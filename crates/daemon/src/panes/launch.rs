//! Starting pane processes: `pane.create`, `pane.resume`, and the tasks of panes restored from the database.
//!
//! A spawn waits until plyd knows the palette (R-R4: the terminal answers colour queries from it before the child's
//! first byte), resolves the program (the login shell for shell panes, the CLI on the login `PATH` otherwise, or
//! `cli_not_found`), checks the CLI's version from its install metadata without running it (C7, `cli_too_old`; an
//! unknown version is logged and allowed), builds argv and env (shell: `<shell> -l`; agents: ply-agents' launch
//! spec), writes the generated files and `launch.json` into `run/panes/<id>/` (mode 0700, files 0600, Ruling R6),
//! sizes a new engine to the last known view, starts the pane's task with the spec (so the agent integration exists
//! before the child's first hook) and only then starts the child, which the task adopts before `pane.added` goes out.
//! A failed spawn leaves no pane behind.
//!
//! `pane.resume` (spec 6.2, 11.3, WP9) relaunches a `lost` pane into the same pane and terminal: an agent pane with a
//! stored session id runs its CLI again from `launch.json` with `--resume <session_id>` (Claude) or `resume
//! <thread_uuid>` (Codex), in the stored directory and with the original `--worktree` option; a pane without a session
//! id (a shell, or an agent that never reported one) reopens as a fresh login shell in its last directory.

use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ply_agents::{LAUNCH_FILE, LaunchRequest, LaunchSpec, adapter};
use ply_proto::control::{ErrorCode, PaneCreateParams};
use ply_proto::pane::{AgentCli, Cli, Pane, PaneId, PaneStatus, Settings};
use ply_term::{Engine, Palette};
use tokio::sync::{mpsc, oneshot};

use crate::branch;
use crate::daemon::{Shared, unix_now};
use crate::panes::pane::{PaneCmd, PaneSeed, Started, fallback_palette, spawn_task};
use crate::panes::registry::{MethodResult, NewPane, internal_for, refuse};
use crate::paths::create_private_dir;
use crate::pty::{Geometry, SpawnSpec, spawn_pane};

/// Longest a spawn waits for the first `theme.set`.
pub const PALETTE_WAIT: Duration = Duration::from_secs(5);

/// Longest a spawn waits for the pane task to adopt the new process.
pub const ADOPT_WAIT: Duration = Duration::from_secs(5);

/// `pane.create`: validates, spawns, announces `pane.added` and returns the pane (see the module docs).
/// Fails with `bad_request`, `not_found`, `tab_full`, `cli_not_found`, `cli_too_old`, `invalid_state`, `spawn_failed`, `shutting_down`.
pub async fn create(shared: &Arc<Shared>, p: PaneCreateParams) -> MethodResult<Pane> {
    if shared.is_stopping() {
        return Err(refuse(ErrorCode::ShuttingDown, "plyd is shutting down"));
    }
    let cwd = Path::new(&p.cwd);
    if !cwd.is_absolute() {
        return Err(refuse(ErrorCode::BadRequest, "cwd must be absolute"));
    }
    if !cwd.is_dir() {
        return Err(refuse(
            ErrorCode::BadRequest,
            format!("{} is not a directory", p.cwd),
        ));
    }
    if p.worktree.is_some() && p.cli != Cli::Claude {
        return Err(refuse(
            ErrorCode::BadRequest,
            "worktree is for claude panes only",
        ));
    }
    if p.prompt.is_some() && p.cli == Cli::Shell {
        return Err(refuse(
            ErrorCode::BadRequest,
            "a shell pane takes no prompt",
        ));
    }
    let palette = wait_palette(shared, None).await?;
    let program = resolve_program(shared, None, p.cli)?;
    let settings = shared.registry().settings();
    let title = default_title(p.cli, &shared.login.shell);
    let status = initial_status(p.cli);
    let new = NewPane {
        workspace_id: p.workspace_id,
        tab_id: p.tab_id,
        cli: p.cli,
        cwd: &p.cwd,
        title: &title,
        status,
    };
    let pane = shared.registry().insert_pane(&new, unix_now())?;
    let options = LaunchOptions {
        program: &program,
        cwd,
        settings: &settings,
        worktree: p.worktree.as_ref().map(|w| w.name.as_str()),
        resume: None,
        prompt: p.prompt.as_deref(),
    };
    let geometry = shared.geometry();
    let prepared = build_launch(shared, pane.id, p.cli, &options).and_then(|spec| {
        new_engine(pane.id, geometry, &palette, &settings).map(|engine| (spec, engine))
    });
    let (spec, engine) = match prepared {
        Ok(parts) => parts,
        Err(e) => {
            abandon(shared, pane.id, None).await;
            return Err(e);
        }
    };
    let handle = spawn_task(
        Arc::clone(shared),
        PaneSeed {
            id: pane.id,
            cli: p.cli,
            default_title: title,
            engine,
            geometry,
            launch: Some(spec.clone()),
            exited: None,
        },
    );
    shared.registry().set_handle(pane.id, handle.clone());
    if let Err(e) = start(shared, pane.id, &spec, geometry, &handle).await {
        abandon(shared, pane.id, Some(&handle)).await;
        return Err(e);
    }
    let pane = {
        let mut reg = shared.registry();
        reg.announce(pane.id, handle);
        reg.entry(pane.id).map_or(pane, |e| e.pane.clone())
    };
    shared.update_power();
    Ok(pane)
}

/// Removes a pane whose process never started: its task, its record and its run directory.
async fn abandon(shared: &Shared, pane_id: PaneId, handle: Option<&mpsc::Sender<PaneCmd>>) {
    if let Some(handle) = handle
        && handle.send(PaneCmd::Stop).await.is_err()
    {
        tracing::debug!(
            pane_id,
            "the task of the pane that failed to start had stopped"
        );
    }
    shared.registry().discard_pane(pane_id);
    remove_pane_dir(pane_id, &shared.paths.pane_dir(pane_id));
}

/// Spawns `spec` and waits until the pane task adopted it (and published the pane's state).
async fn start(
    shared: &Shared,
    pane_id: PaneId,
    spec: &LaunchSpec,
    geometry: Geometry,
    handle: &mpsc::Sender<PaneCmd>,
) -> MethodResult<()> {
    let (adopted, done) = oneshot::channel();
    let mut started = spawn(shared, pane_id, spec, geometry)?;
    started.adopted = Some(adopted);
    let gone = || {
        tracing::error!(
            pane_id,
            "the pane task is gone; the new process is orphaned"
        );
        refuse(ErrorCode::Internal, "the pane task is gone")
    };
    if handle
        .send(PaneCmd::Start(Box::new(started)))
        .await
        .is_err()
    {
        return Err(gone());
    }
    match tokio::time::timeout(ADOPT_WAIT, done).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) => Err(gone()),
        Err(_) => {
            tracing::warn!(
                pane_id,
                "the pane task took over {ADOPT_WAIT:?} to adopt its process"
            );
            Ok(())
        }
    }
}

/// `pane.resume`: relaunches a `lost` pane from its `launch.json`, resuming the CLI session when one was reported.
/// Fails with `not_found`, `invalid_state` (the pane is not lost, or no palette), `cli_not_found`, `spawn_failed`.
pub async fn resume(shared: &Arc<Shared>, pane_id: PaneId) -> MethodResult<Pane> {
    if shared.is_stopping() {
        return Err(refuse(ErrorCode::ShuttingDown, "plyd is shutting down"));
    }
    let handle = {
        let reg = shared.registry();
        let entry = reg.require(pane_id)?;
        entry.handle.clone()
    };
    let Some(handle) = handle else {
        return Err(refuse(ErrorCode::InvalidState, "the pane has no task"));
    };
    let pane = shared
        .registry()
        .begin_resume(pane_id, PaneStatus::Starting, unix_now())?;
    tracing::info!(pane_id, session = ?pane.session_ref, "resuming the pane");
    let resumed = relaunch(shared, &pane, &handle).await;
    let close = shared
        .registry()
        .end_resume(pane_id, resumed.is_ok(), unix_now());
    if close {
        tracing::info!(
            pane_id,
            "the resume failed after a close was asked for; closing the pane"
        );
        let closed = shared.registry().close_pane(pane_id, unix_now());
        match closed {
            Ok(_) => {
                if handle.send(PaneCmd::Stop).await.is_err() {
                    tracing::debug!(pane_id, "the pane task had stopped");
                }
                remove_pane_dir(pane_id, &shared.paths.pane_dir(pane_id));
            }
            Err(e) => {
                tracing::warn!(pane_id, error = %e.msg, "cannot close the pane after its failed resume");
            }
        }
    }
    shared.update_power();
    resumed?;
    let reg = shared.registry();
    Ok(reg.entry(pane_id).map_or(pane, |e| e.pane.clone()))
}

async fn relaunch(
    shared: &Arc<Shared>,
    pane: &Pane,
    handle: &tokio::sync::mpsc::Sender<PaneCmd>,
) -> MethodResult<()> {
    let pane_id = pane.id;
    let dir = shared.paths.pane_dir(pane_id);
    let stored = std::fs::read(dir.join(LAUNCH_FILE))
        .map_err(|e| e.to_string())
        .and_then(|bytes| LaunchSpec::from_json(&bytes).map_err(|e| e.to_string()));
    let stored = match stored {
        Ok(spec) => spec,
        Err(e) => {
            tracing::warn!(pane_id, error = %e, "no usable launch.json to resume from");
            return Err(refuse(
                ErrorCode::SpawnFailed,
                format!("no stored launch spec for pane {pane_id}: {e}"),
            ));
        }
    };
    wait_palette(shared, Some(pane_id)).await?;
    let settings = shared.registry().settings();
    let session = pane
        .session_ref
        .as_deref()
        .filter(|_| stored.cli != Cli::Shell);
    let spec = match session {
        Some(session) => {
            let program = resolve_program(shared, Some(pane_id), stored.cli)?;
            let options = LaunchOptions {
                program: &program,
                cwd: Path::new(&stored.cwd),
                settings: &settings,
                worktree: stored.worktree.as_deref(),
                resume: Some(session),
                prompt: None,
            };
            build_launch(shared, pane_id, stored.cli, &options)?
        }
        None => {
            let cwd = [pane.cwd.as_str(), stored.cwd.as_str()]
                .into_iter()
                .map(Path::new)
                .find(|dir| dir.is_dir())
                .ok_or_else(|| {
                    tracing::warn!(pane_id, cwd = %pane.cwd, "no directory left to reopen the pane in");
                    refuse(
                        ErrorCode::SpawnFailed,
                        format!("{} no longer exists", pane.cwd),
                    )
                })?;
            let options = LaunchOptions {
                program: &shared.login.shell,
                cwd,
                settings: &settings,
                worktree: None,
                resume: None,
                prompt: None,
            };
            build_launch(shared, pane_id, Cli::Shell, &options)?
        }
    };
    if spec.cli != pane.cli {
        tracing::info!(pane_id, from = ?pane.cli, "no session id to resume; reopening the pane as a shell");
        let title = default_title(spec.cli, &shared.login.shell);
        shared.registry().set_cli(pane_id, spec.cli, &title);
    }
    shared.set_status(pane_id, initial_status(spec.cli), None);
    if handle
        .send(PaneCmd::Launch(Box::new(spec.clone())))
        .await
        .is_err()
    {
        tracing::error!(pane_id, "the pane task is gone; cannot resume");
        return Err(refuse(ErrorCode::Internal, "the pane task is gone"));
    }
    if let Err(e) = start(shared, pane_id, &spec, shared.geometry(), handle).await {
        if handle.send(PaneCmd::LaunchFailed).await.is_err() {
            tracing::debug!(pane_id, "the pane task had stopped");
        }
        return Err(e);
    }
    Ok(())
}

/// Gives every pane loaded from the database (all `lost` or `exited`) a task with an empty terminal, so a client
/// can attach to it and see its status.
pub fn restore(shared: &Arc<Shared>) {
    let (panes, settings, palette) = {
        let reg = shared.registry();
        let all: Vec<Pane> = reg
            .workspaces()
            .iter()
            .filter_map(|w| reg.panes_of(w.id).ok())
            .flatten()
            .collect();
        (all, reg.settings(), reg.palette().map(Palette::from_theme))
    };
    let palette = palette.unwrap_or_else(fallback_palette);
    let geometry = shared.geometry();
    for pane in panes {
        let engine = match new_engine(pane.id, geometry, &palette, &settings) {
            Ok(engine) => engine,
            Err(e) => {
                tracing::error!(pane_id = pane.id, error = %e.msg, "cannot create a terminal for a restored pane");
                continue;
            }
        };
        let handle = spawn_task(
            Arc::clone(shared),
            PaneSeed {
                id: pane.id,
                cli: pane.cli,
                default_title: default_title(pane.cli, &shared.login.shell),
                engine,
                geometry,
                launch: None,
                exited: (pane.status == PaneStatus::Exited).then_some(pane.exit_code.unwrap_or(0)),
            },
        );
        shared.registry().set_handle(pane.id, handle);
        branch::lookup(shared, pane.id, pane.cwd);
    }
}

/// Reopens as fresh shells the `lost` panes with no session to resume (shells, agent panes whose CLI reported no id; R50, spec 11.3), leaving the rest for `pane.resume`; run after binding, since a first start may wait for `theme.set`.
pub async fn reopen_sessionless(shared: Arc<Shared>) {
    let panes: Vec<PaneId> = {
        let reg = shared.registry();
        reg.workspaces()
            .iter()
            .filter_map(|w| reg.panes_of(w.id).ok())
            .flatten()
            .filter(|p| {
                p.status == PaneStatus::Lost && (p.cli == Cli::Shell || p.session_ref.is_none())
            })
            .map(|p| p.id)
            .collect()
    };
    for pane_id in panes {
        match resume(&shared, pane_id).await {
            Ok(pane) => {
                tracing::info!(pane_id, cli = ?pane.cli, "reopened a pane without a session as a fresh shell");
            }
            Err(e) => {
                tracing::warn!(pane_id, error = %e.msg, "cannot reopen the pane; it stays lost");
            }
        }
    }
}

/// Deletes `run/panes/<id>/` of a closed pane; a failure is only logged.
pub fn remove_pane_dir(pane_id: PaneId, dir: &Path) {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            tracing::warn!(pane_id, dir = %dir.display(), error = %e, "cannot remove the pane's run directory")
        }
    }
}

struct LaunchOptions<'a> {
    program: &'a Path,
    cwd: &'a Path,
    settings: &'a Settings,
    worktree: Option<&'a str>,
    resume: Option<&'a str>,
    prompt: Option<&'a str>,
}

/// The palette `theme.set` gave, waiting [`PALETTE_WAIT`] for one; `pane_id` is the pane being resumed, if any, for the log.
async fn wait_palette(shared: &Shared, pane_id: Option<PaneId>) -> MethodResult<Palette> {
    let mut rx = shared.palette.subscribe();
    let waited = tokio::time::timeout(PALETTE_WAIT, rx.wait_for(Option::is_some)).await;
    match waited {
        Ok(Ok(palette)) => palette
            .clone()
            .ok_or_else(|| refuse(ErrorCode::Internal, "the palette vanished")),
        Ok(Err(_)) | Err(_) => {
            tracing::warn!(
                ?pane_id,
                "a spawn waited {PALETTE_WAIT:?} for theme.set in vain"
            );
            Err(refuse(
                ErrorCode::InvalidState,
                "plyd has no terminal palette yet; send theme.set first",
            ))
        }
    }
}

fn agent_cli(cli: Cli) -> Option<AgentCli> {
    match cli {
        Cli::Claude => Some(AgentCli::Claude),
        Cli::Codex => Some(AgentCli::Codex),
        Cli::Shell => None,
    }
}

/// The program for `cli` from the login PATH, version-checked; `pane_id` is the pane being resumed, if any, for the log.
fn resolve_program(shared: &Shared, pane_id: Option<PaneId>, cli: Cli) -> MethodResult<PathBuf> {
    let Some(agent) = agent_cli(cli) else {
        return Ok(shared.login.shell.clone());
    };
    let name = ply_agents::cli_name(agent);
    let Some(program) = shared.login.which(name) else {
        tracing::warn!(
            ?pane_id,
            cli = name,
            "the CLI is not on the login shell's PATH"
        );
        return Err(refuse(
            ErrorCode::CliNotFound,
            format!("{name} is not on the login shell's PATH"),
        ));
    };
    let adapter = adapter(agent);
    match adapter.installed_version(&program) {
        Ok(version) => {
            if let Err(e) = adapter.check_version(&version) {
                tracing::warn!(?pane_id, cli = name, %version, error = %e, "refusing a CLI below the supported minimum");
                return Err(refuse(ErrorCode::CliTooOld, e.to_string()));
            }
            tracing::debug!(?pane_id, cli = name, %version, "CLI version accepted");
        }
        Err(e) => {
            tracing::warn!(?pane_id, cli = name, program = %program.display(), error = %e, "CLI version unknown; launching anyway");
        }
    }
    Ok(program)
}

fn default_title(cli: Cli, shell: &Path) -> String {
    match cli {
        Cli::Claude => "claude".to_owned(),
        Cli::Codex => "codex".to_owned(),
        Cli::Shell => shell
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("shell")
            .to_owned(),
    }
}

fn initial_status(cli: Cli) -> PaneStatus {
    match cli {
        Cli::Shell => PaneStatus::Idle,
        Cli::Claude | Cli::Codex => PaneStatus::Starting,
    }
}

fn build_launch(
    shared: &Shared,
    pane_id: PaneId,
    cli: Cli,
    o: &LaunchOptions<'_>,
) -> MethodResult<LaunchSpec> {
    let dir = shared.paths.pane_dir(pane_id);
    create_private_dir(&dir)
        .map_err(|e| internal_for(pane_id, "cannot create the pane directory", &e))?;
    let (spec, files) = match agent_cli(cli) {
        None => (
            LaunchSpec {
                cli,
                argv: vec![o.program.display().to_string(), "-l".to_owned()],
                env: BTreeMap::from([
                    ("TERM".to_owned(), ply_agents::TERM.to_owned()),
                    ("COLORTERM".to_owned(), ply_agents::COLORTERM.to_owned()),
                ]),
                cwd: o.cwd.display().to_string(),
                worktree: None,
                resume: None,
            },
            Vec::new(),
        ),
        Some(agent) => {
            let hook_socket = shared.paths.hook_socket();
            let request = LaunchRequest {
                pane_id,
                program: o.program,
                cwd: o.cwd,
                hook_program: &shared.hook_program,
                hook_socket: &hook_socket,
                pane_dir: &dir,
                settings: o.settings,
                worktree: o.worktree,
                resume: o.resume,
                prompt: o.prompt,
            };
            let launch = adapter(agent).launch(&request).map_err(|e| match e {
                ply_agents::Error::InvalidLaunch(_) | ply_agents::Error::NonUtf8Path(_) => {
                    tracing::warn!(pane_id, error = %e, "launch request refused");
                    refuse(ErrorCode::BadRequest, e.to_string())
                }
                other => {
                    tracing::error!(pane_id, error = %other, "cannot build the launch spec");
                    refuse(ErrorCode::SpawnFailed, other.to_string())
                }
            })?;
            (launch.spec, launch.files)
        }
    };
    for file in &files {
        write_private(pane_id, &file.path, file.contents.as_bytes())?;
    }
    let json = spec.to_json().map_err(|e| {
        internal_for(
            pane_id,
            "cannot serialise launch.json",
            &crate::Error::Agents(e),
        )
    })?;
    write_private(pane_id, &dir.join(LAUNCH_FILE), json.as_bytes())?;
    Ok(spec)
}

fn write_private(pane_id: PaneId, path: &Path, bytes: &[u8]) -> MethodResult<()> {
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .and_then(|mut f| f.write_all(bytes))
        .map_err(|source| {
            internal_for(
                pane_id,
                "cannot write a pane file",
                &crate::Error::Io {
                    what: "cannot write",
                    path: path.to_path_buf(),
                    source,
                },
            )
        })
}

fn new_engine(
    pane_id: PaneId,
    g: Geometry,
    palette: &Palette,
    settings: &Settings,
) -> MethodResult<Engine> {
    let mut engine = Engine::new(pane_id, g.cols, g.rows, settings.scrollback_lines, palette)
        .map_err(|e| internal_for(pane_id, "cannot create the terminal", &e.into()))?;
    engine
        .resize(g.cols, g.rows, g.cell_width_px, g.cell_height_px)
        .map_err(|e| internal_for(pane_id, "cannot size the terminal", &e.into()))?;
    engine.set_option_as_meta(settings.option_as_meta);
    Ok(engine)
}

fn spawn(
    shared: &Shared,
    pane_id: PaneId,
    spec: &LaunchSpec,
    geometry: Geometry,
) -> MethodResult<Started> {
    let env = shared.login.env_for(&spec.env);
    let request = SpawnSpec {
        pane_id,
        argv: &spec.argv,
        env: &env,
        cwd: Path::new(&spec.cwd),
        geometry,
    };
    match spawn_pane(&request) {
        Ok((process, channels)) => Ok(Started {
            process,
            channels,
            adopted: None,
        }),
        Err(e) => {
            tracing::warn!(pane_id, error = %e, "cannot start the pane's process");
            Err(refuse(ErrorCode::SpawnFailed, e.to_string()))
        }
    }
}
