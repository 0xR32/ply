//! The LaunchAgent that keeps plyd independent of the app (spec 11.3, Rulings R3 and R38).
//!
//! plyd is the single writer of `~/Library/LaunchAgents/dev.ply.app.plyd.plist`: `plyd install-agent` renders
//! `packaging/plyd.plist.template` (compiled in) with the path of the plyd binary it runs as, rewrites the file only
//! when it changed (booting the old definition out first, unless a plyd holds the instance lock: booting it out would
//! kill every session, so it reports and leaves the agent as is), then runs `launchctl bootstrap gui/<uid> <plist>`
//! (an already loaded agent is fine) and `launchctl kickstart gui/<uid>/dev.ply.app.plyd`. The agent does not run at
//! login (`RunAtLoad` false; the app starts it on demand), launchd restarts it only after a crash (`KeepAlive`
//! with `SuccessfulExit` false), and `ProcessType` is `Standard` because `Background` throttles every pane's I/O.
//! The app calls this subcommand for the cargo-built plyd it found. With `PLY_HOME` set it refuses, since a
//! LaunchAgent would not see `PLY_HOME`; `--dry-run` prints the plan instead. The steps take the `launchctl` runner
//! as a parameter ([`install_with`]), so tests drive them without touching the user's launchd.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Error, Result, io};
use crate::paths::{ENV_PLY_HOME, Paths};

/// The LaunchAgent label, the bundle id plus `.plyd` (Ruling R3).
pub const LABEL: &str = "dev.ply.app.plyd";

const TEMPLATE: &str = include_str!("../../../packaging/plyd.plist.template");

/// The shell command that stops the agent's plyd from outside ply (a clean SIGTERM stop, so launchd does not restart it).
pub const KILL_HINT: &str = "launchctl kill TERM gui/$(id -u)/dev.ply.app.plyd";

/// `launchctl bootstrap` statuses that mean the agent is already loaded: EIO, EEXIST, EALREADY.
const ALREADY_LOADED: [i32; 3] = [5, 17, 37];

/// What `install-agent` writes and runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Install {
    /// `~/Library/LaunchAgents/dev.ply.app.plyd.plist`.
    pub plist: PathBuf,
    /// The rendered plist.
    pub contents: String,
    /// The launchd domain, `gui/<uid>`.
    pub domain: String,
}

impl Install {
    /// The plan for `plyd` under `home` for user `uid`; fails with [`Error::InstallRefused`] for a non-UTF-8 plyd path.
    pub fn new(home: &Path, plyd: &Path, uid: u32) -> Result<Self> {
        Ok(Self {
            plist: plist_path(home),
            contents: render_plist(plyd)?,
            domain: format!("gui/{uid}"),
        })
    }

    /// The `launchctl` argument lists in the order `install-agent` runs them (bootout only when the file changes).
    pub fn commands(&self) -> [Vec<String>; 3] {
        let target = format!("{}/{LABEL}", self.domain);
        [
            vec!["bootout".to_owned(), target.clone()],
            vec![
                "bootstrap".to_owned(),
                self.domain.clone(),
                self.plist.display().to_string(),
            ],
            vec!["kickstart".to_owned(), target],
        ]
    }
}

/// Where the plist lives for the user whose home is `home`.
pub fn plist_path(home: &Path) -> PathBuf {
    home.join("Library")
        .join("LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

/// The template with the label and the XML-escaped plyd path filled in; fails with [`Error::InstallRefused`] for a non-UTF-8 path.
pub fn render_plist(plyd: &Path) -> Result<String> {
    let Some(path) = plyd.to_str() else {
        return Err(Error::InstallRefused(format!(
            "the plyd path {} is not UTF-8",
            plyd.display()
        )));
    };
    let escaped = path
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    Ok(TEMPLATE
        .replace("{{LABEL}}", LABEL)
        .replace("{{PLYD}}", &escaped))
}

/// Installs and starts the agent for the running plyd (module docs), or with `dry_run` only describes it; returns a report.
/// Fails with [`Error::InstallRefused`] under `PLY_HOME`, [`Error::NoHome`], [`Error::Io`] or [`Error::Launchctl`].
pub fn install_agent(dry_run: bool) -> Result<String> {
    if std::env::var_os(ENV_PLY_HOME).is_some_and(|v| !v.is_empty()) {
        return Err(Error::InstallRefused(format!(
            "{ENV_PLY_HOME} is set, and a LaunchAgent would not see it; run `plyd --foreground` instead"
        )));
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|h| h.is_absolute())
        .ok_or_else(|| Error::NoHome("HOME is not set to an absolute path".to_owned()))?;
    let exe = std::env::current_exe().map_err(io("cannot locate", "the plyd executable"))?;
    let plyd = fs::canonicalize(&exe).map_err(io("cannot resolve", &exe))?;
    let plan = Install::new(&home, &plyd, rustix::process::getuid().as_raw())?;
    install_with(&plan, &home, &plyd, dry_run, &mut launchctl)
}

/// A `launchctl` runner: takes the argument list, returns the exit status and the trimmed stderr.
pub type Launchctl<'a> = dyn FnMut(&[String]) -> Result<(i32, String)> + 'a;

/// The steps of [`install_agent`] for `plan` (the plyd lock under `home`), with `launchctl` run through `run`; errors as [`install_agent`].
pub fn install_with(
    plan: &Install,
    home: &Path,
    plyd: &Path,
    dry_run: bool,
    run: &mut Launchctl<'_>,
) -> Result<String> {
    let [bootout, bootstrap, kickstart] = plan.commands();
    let mut report = String::new();
    if dry_run {
        let _ = writeln!(
            report,
            "would write {}:\n{}",
            plan.plist.display(),
            plan.contents
        );
        for args in [&bootstrap, &kickstart] {
            let _ = writeln!(report, "would run: launchctl {}", args.join(" "));
        }
        return Ok(report);
    }
    let current = match fs::read_to_string(&plan.plist) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(io("cannot read", &plan.plist)(e)),
    };
    if current.as_deref() != Some(plan.contents.as_str()) {
        let lock = Paths::resolve(None, Some(home.as_os_str()), None)?.lock();
        if current.is_some()
            && let Some(pid) = crate::lock::running(&lock)?
        {
            let pid = pid.map_or_else(|| "?".to_owned(), |p| p.to_string());
            tracing::warn!(pid, plist = %plan.plist.display(), "plyd is running; not replacing its LaunchAgent");
            let _ = writeln!(
                report,
                "plyd (pid {pid}) is running; {} was left unchanged so its sessions keep running. To switch to {}, run \"Restart plyd\" in ply's command palette (⌘K; running sessions come back lost, to be resumed) or \"Quit ply and stop sessions\", or stop plyd with `{KILL_HINT}`; ply installs this plyd the next time it starts one",
                plan.plist.display(),
                plyd.display()
            );
            return Ok(report);
        }
        if current.is_some() {
            let (code, stderr) = run(&bootout)?;
            if code != 0 {
                tracing::debug!(code, %stderr, "bootout of the old agent failed (it was not loaded)");
            }
        }
        if let Some(dir) = plan.plist.parent() {
            fs::create_dir_all(dir).map_err(io("cannot create", dir))?;
        }
        fs::write(&plan.plist, &plan.contents).map_err(io("cannot write", &plan.plist))?;
        let _ = writeln!(report, "installed {}", plan.plist.display());
    }
    let (code, stderr) = run(&bootstrap)?;
    if code != 0 && !ALREADY_LOADED.contains(&code) {
        return Err(Error::Launchctl {
            args: bootstrap.join(" "),
            code,
            stderr,
        });
    }
    let (code, stderr) = run(&kickstart)?;
    if code != 0 {
        return Err(Error::Launchctl {
            args: kickstart.join(" "),
            code,
            stderr,
        });
    }
    let _ = writeln!(report, "started {LABEL}");
    Ok(report)
}

/// Runs `/bin/launchctl` with `args`; fails with [`Error::Io`] when it cannot be started.
fn launchctl(args: &[String]) -> Result<(i32, String)> {
    let out = Command::new("/bin/launchctl")
        .args(args)
        .output()
        .map_err(io("cannot run", "/bin/launchctl"))?;
    Ok((
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).trim().to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plist_restarts_plyd_only_after_a_crash_and_never_throttles_it() {
        let plist = render_plist(Path::new("/Users/example/ply/target/release/plyd")).unwrap();
        assert!(plist.contains("<string>dev.ply.app.plyd</string>"));
        assert!(plist.contains("<string>/Users/example/ply/target/release/plyd</string>"));
        assert!(plist.contains("<key>SuccessfulExit</key>\n    <false/>"));
        assert!(plist.contains("<key>ProcessType</key>\n  <string>Standard</string>"));
        assert!(plist.contains("<key>RunAtLoad</key>\n  <false/>"));
        assert!(!plist.contains("{{"), "every placeholder is filled");
        assert!(
            render_plist(Path::new("/a&b/plyd"))
                .unwrap()
                .contains("<string>/a&amp;b/plyd</string>")
        );
    }

    #[test]
    fn the_plan_targets_the_user_gui_domain() {
        let plan = Install::new(Path::new("/Users/example"), Path::new("/opt/plyd"), 501).unwrap();
        assert_eq!(
            plan.plist,
            Path::new("/Users/example/Library/LaunchAgents/dev.ply.app.plyd.plist")
        );
        let [bootout, bootstrap, kickstart] = plan.commands();
        assert_eq!(bootout, ["bootout", "gui/501/dev.ply.app.plyd"]);
        assert_eq!(
            bootstrap,
            [
                "bootstrap",
                "gui/501",
                "/Users/example/Library/LaunchAgents/dev.ply.app.plyd.plist"
            ]
        );
        assert_eq!(kickstart, ["kickstart", "gui/501/dev.ply.app.plyd"]);
    }

    /// Records every `launchctl` call and answers each with `code`.
    fn recorder(
        calls: &mut Vec<String>,
        code: i32,
    ) -> impl FnMut(&[String]) -> Result<(i32, String)> + '_ {
        move |args| {
            calls.push(args.join(" "));
            Ok((code, String::new()))
        }
    }

    #[test]
    fn install_writes_the_plist_once_and_boots_out_only_a_changed_one() {
        let home = std::env::temp_dir().join(format!("ply-launchd-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(&home).unwrap();
        let plan = Install::new(&home, Path::new("/opt/plyd"), 501).unwrap();
        let mut calls = Vec::new();

        let dry = install_with(
            &plan,
            &home,
            Path::new("/opt/plyd"),
            true,
            &mut recorder(&mut calls, 0),
        )
        .unwrap();
        assert!(
            dry.contains("would run: launchctl kickstart gui/501/dev.ply.app.plyd"),
            "{dry}"
        );
        assert!(calls.is_empty(), "a dry run runs nothing");
        assert!(!plan.plist.exists(), "a dry run writes nothing");

        install_with(
            &plan,
            &home,
            Path::new("/opt/plyd"),
            false,
            &mut recorder(&mut calls, 0),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&plan.plist).unwrap(), plan.contents);
        let bootstrap = format!("bootstrap gui/501 {}", plan.plist.display());
        assert_eq!(
            calls,
            [bootstrap.as_str(), "kickstart gui/501/dev.ply.app.plyd"]
        );

        calls.clear();
        install_with(
            &plan,
            &home,
            Path::new("/opt/plyd"),
            false,
            &mut recorder(&mut calls, 37),
        )
        .unwrap_err();
        assert_eq!(
            calls.len(),
            2,
            "an already loaded agent (EALREADY) is fine, then kickstart failed"
        );

        let moved = Install::new(&home, Path::new("/opt/other/plyd"), 501).unwrap();
        calls.clear();
        install_with(
            &moved,
            &home,
            Path::new("/opt/other/plyd"),
            false,
            &mut recorder(&mut calls, 0),
        )
        .unwrap();
        assert_eq!(
            calls[0], "bootout gui/501/dev.ply.app.plyd",
            "a changed plist replaces the old agent"
        );
        assert_eq!(fs::read_to_string(&moved.plist).unwrap(), moved.contents);

        let lock = Paths::resolve(None, Some(home.as_os_str()), None)
            .unwrap()
            .lock();
        fs::create_dir_all(lock.parent().unwrap()).unwrap();
        let held = crate::lock::InstanceLock::acquire(&lock).unwrap();
        calls.clear();
        let report = install_with(
            &plan,
            &home,
            Path::new("/opt/plyd"),
            false,
            &mut recorder(&mut calls, 0),
        )
        .unwrap();
        assert!(report.contains("is running"), "{report}");
        assert!(
            report.contains("\"Restart plyd\"")
                && report.contains("\"Quit ply and stop sessions\"")
                && report.contains(KILL_HINT),
            "the report names the palette commands and the launchctl fallback: {report}"
        );
        assert!(
            calls.is_empty(),
            "a running plyd's agent is never booted out"
        );
        assert_eq!(
            fs::read_to_string(&moved.plist).unwrap(),
            moved.contents,
            "left unchanged"
        );
        drop(held);
        fs::remove_dir_all(&home).unwrap();
    }
}
