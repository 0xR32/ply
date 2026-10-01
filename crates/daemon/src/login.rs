//! The user's login shell and the environment every pane process starts with (C5).
//!
//! plyd runs under launchd with a minimal environment, so it resolves two things once at startup: the login shell
//! (`$SHELL`, else the directory record via `dscl`, else the passwd entry via `id -P`, else `/bin/zsh`) and that
//! shell's `PATH` as an interactive login shell sees it (`$SHELL -l -i -c`, [`PATH_PROBE_TIMEOUT`] at most, stdin from
//! `/dev/null`): zsh reads `.zshrc` only when interactive, and that is where installers put `~/.local/bin`, home of the
//! `claude` and `codex` binaries (Ruling R42). A shell that fails or hangs interactively is asked again as a plain
//! login shell (`-l -c`); the log says which one answered. The same probe reports the [`CAPTURED`] variables the shell
//! exports (Ruling R52: the CLIs' homes, proxies, CA certificates, locale), and nothing else, so no API key an rc
//! file sets ever reaches plyd; when no shell answers, plyd's own values of them are used. That fallback is held only
//! until the shell answers: [`Login`] asks it again in the background with [`LATE_PROBE_TIMEOUT`], since a shell
//! started in the first minute after login can miss the startup budget, and a CLI launch waits for that answer
//! ([`Login::settled`]) rather than look for the CLI on launchd's `PATH`. Children start from a
//! cleared environment (`env_clear`) with only [`LoginEnv::base`]: a short allow-list of plyd's own variables (home,
//! user, temp dir, locale, SSH agent), `SHELL`, the login `PATH`, the captured variables, `TERM=xterm-256color` and
//! `COLORTERM=truecolor`, plus the launch spec's additions. `LANG` defaults to `en_US.UTF-8` when neither plyd nor
//! the shell has one, because a CLI in the C locale draws no UTF-8. Nothing here reads or writes the CLIs' own
//! configuration (INV-8).

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::process::Command;
use tokio::sync::{Mutex, watch};

/// `PATH` used when the login shell cannot report one.
pub const FALLBACK_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// Shell used when no source names one (the macOS default).
pub const FALLBACK_SHELL: &str = "/bin/zsh";

/// `LANG` given to children when plyd itself has none.
pub const DEFAULT_LANG: &str = "en_US.UTF-8";

/// Longest wait for one probe (`dscl`, `id`).
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Longest wait for one `PATH` probe of the login shell at startup, which holds back plyd's sockets meanwhile.
pub const PATH_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Longest wait for one `PATH` probe asked again after the startup probes went unanswered.
pub const LATE_PROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// How long after an unanswered late probe the shell is left alone before a CLI launch asks it again.
pub const LATE_PROBE_INTERVAL: Duration = Duration::from_secs(60);

const PASSTHROUGH: &[&str] = &[
    "HOME",
    "USER",
    "LOGNAME",
    "TMPDIR",
    "LANG",
    "LC_ALL",
    "LC_COLLATE",
    "LC_CTYPE",
    "LC_MESSAGES",
    "LC_MONETARY",
    "LC_NUMERIC",
    "LC_TIME",
    "SSH_AUTH_SOCK",
    "__CF_USER_TEXT_ENCODING",
];

/// The variables the login-shell probe reports when the shell exports them, and passes to every pane (Ruling R52).
pub const CAPTURED: &[&str] = &[
    "CODEX_HOME",
    "CLAUDE_CONFIG_DIR",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "all_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NODE_EXTRA_CA_CERTS",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
];

const PATH_BEGIN: &str = "__PLY_PATH_BEGIN__";
const PATH_END: &str = "__PLY_PATH_END__";
const VAR_BEGIN: &str = "__PLY_VAR__";
const VAR_END: &str = "__PLY_VAR_END__";

/// What the login-shell probe reported: its `PATH` and the [`CAPTURED`] variables it exports.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ShellReport {
    path: String,
    vars: BTreeMap<String, String>,
}

/// The login shell and the base environment of every pane process; read-only, replaced whole by [`Login`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginEnv {
    /// Absolute path of the login shell; shell panes run it as `<shell> -l`.
    pub shell: PathBuf,
    /// Variables every child starts with, `PATH` (the login shell's) included.
    pub base: BTreeMap<String, String>,
    /// Whether the login shell reported `PATH`; `false` while plyd's own `PATH` and variables stand in.
    pub from_shell: bool,
    probe_env: BTreeMap<String, String>,
}

impl LoginEnv {
    /// Resolves the shell and its login `PATH` from plyd's environment; never fails (each step falls back and logs).
    /// Runs up to four short child processes: `dscl` and `id` bounded by [`PROBE_TIMEOUT`], the shell by [`PATH_PROBE_TIMEOUT`].
    pub async fn resolve() -> Self {
        let own: BTreeMap<String, String> = std::env::vars().collect();
        let shell = login_shell(&own).await;
        let mut probe_env = base_env(&own, &shell);
        probe_env.extend(captured(&own));
        let fallback = own
            .get("PATH")
            .cloned()
            .unwrap_or_else(|| FALLBACK_PATH.to_owned());
        Self::probed(shell, probe_env, fallback, PATH_PROBE_TIMEOUT).await
    }

    /// Asks `shell` for its `PATH`, each mode bounded by `timeout`; without an answer `fallback` stands in.
    async fn probed(
        shell: PathBuf,
        probe_env: BTreeMap<String, String>,
        fallback: String,
        timeout: Duration,
    ) -> Self {
        match login_path_within(&shell, &probe_env, timeout).await {
            Some(report) => Self::from_report(shell, probe_env, report, true),
            None => {
                tracing::warn!(shell = %shell.display(), path = %fallback, "the login shell reported no PATH; using plyd's PATH and variables until it answers");
                let report = ShellReport {
                    path: fallback,
                    vars: BTreeMap::new(),
                };
                Self::from_report(shell, probe_env, report, false)
            }
        }
    }

    /// Asks the shell once more with `timeout` per mode; `None` when it still does not answer.
    async fn ask_again(&self, timeout: Duration) -> Option<Self> {
        let report = login_path_within(&self.shell, &self.probe_env, timeout).await?;
        Some(Self::from_report(
            self.shell.clone(),
            self.probe_env.clone(),
            report,
            true,
        ))
    }

    fn from_report(
        shell: PathBuf,
        probe_env: BTreeMap<String, String>,
        report: ShellReport,
        from_shell: bool,
    ) -> Self {
        tracing::info!(variables = ?report.vars.keys().collect::<Vec<_>>(), "variables taken from the login shell");
        let mut base = probe_env.clone();
        base.extend(report.vars);
        base.insert("PATH".to_owned(), report.path);
        Self {
            shell,
            base,
            from_shell,
            probe_env,
        }
    }

    /// The login `PATH`.
    pub fn path(&self) -> &str {
        self.base.get("PATH").map_or(FALLBACK_PATH, String::as_str)
    }

    /// The complete environment of a child: [`LoginEnv::base`] overlaid with `additions` (the launch spec's variables).
    pub fn env_for(&self, additions: &BTreeMap<String, String>) -> BTreeMap<String, String> {
        let mut env = self.base.clone();
        env.extend(additions.iter().map(|(k, v)| (k.clone(), v.clone())));
        env
    }

    /// The first executable file named `name` on the login `PATH`; `None` when there is none (`cli_not_found`).
    pub fn which(&self, name: &str) -> Option<PathBuf> {
        which_in(self.path(), name)
    }
}

/// The login environment children start with: the one resolved at startup, replaced once a late probe answers.
#[derive(Debug)]
pub struct Login {
    env: watch::Sender<Arc<LoginEnv>>,
    late_probe: Mutex<Option<Instant>>,
    late_timeout: Duration,
}

impl Login {
    /// Holds `env`; a late probe, when one is needed, waits [`LATE_PROBE_TIMEOUT`] per mode.
    pub fn new(env: LoginEnv) -> Self {
        Self::with_late_timeout(env, LATE_PROBE_TIMEOUT)
    }

    fn with_late_timeout(env: LoginEnv, late_timeout: Duration) -> Self {
        Self {
            env: watch::Sender::new(Arc::new(env)),
            late_probe: Mutex::new(None),
            late_timeout,
        }
    }

    /// The environment as known now; never waits.
    pub fn get(&self) -> Arc<LoginEnv> {
        Arc::clone(&self.env.borrow())
    }

    /// The environment once the shell has answered or missed a late probe (at most one per [`LATE_PROBE_INTERVAL`], shared by concurrent callers); waits up to twice [`LATE_PROBE_TIMEOUT`], never fails.
    pub async fn settled(&self) -> Arc<LoginEnv> {
        let current = self.get();
        if current.from_shell {
            return current;
        }
        let mut missed = self.late_probe.lock().await;
        let current = self.get();
        if current.from_shell || missed.is_some_and(|at| at.elapsed() < LATE_PROBE_INTERVAL) {
            return current;
        }
        match current.ask_again(self.late_timeout).await {
            Some(env) => {
                tracing::info!(path = %env.path(), "the login shell answered a later probe; children start with its PATH from now on");
                let env = Arc::new(env);
                self.env.send_replace(Arc::clone(&env));
                *missed = None;
                env
            }
            None => {
                tracing::warn!(shell = %current.shell.display(), "the login shell did not answer the later probe either; keeping plyd's PATH");
                *missed = Some(Instant::now());
                current
            }
        }
    }
}

/// The first executable regular file `name` in the `:`-separated `path`; relative entries are skipped.
pub fn which_in(path: &str, name: &str) -> Option<PathBuf> {
    path.split(':')
        .map(Path::new)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The [`CAPTURED`] variables of `env`, and no others.
fn captured(env: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    CAPTURED
        .iter()
        .filter_map(|k| env.get(*k).map(|v| ((*k).to_owned(), v.clone())))
        .collect()
}

fn base_env(own: &BTreeMap<String, String>, shell: &Path) -> BTreeMap<String, String> {
    let mut env: BTreeMap<String, String> = PASSTHROUGH
        .iter()
        .filter_map(|k| own.get(*k).map(|v| ((*k).to_owned(), v.clone())))
        .collect();
    env.entry("LANG".to_owned())
        .or_insert_with(|| DEFAULT_LANG.to_owned());
    env.insert("SHELL".to_owned(), shell.display().to_string());
    env.insert("TERM".to_owned(), ply_agents::TERM.to_owned());
    env.insert("COLORTERM".to_owned(), ply_agents::COLORTERM.to_owned());
    env
}

async fn login_shell(own: &BTreeMap<String, String>) -> PathBuf {
    if let Some(shell) = own.get("SHELL").map(PathBuf::from)
        && shell.is_absolute()
        && is_executable(&shell)
    {
        return shell;
    }
    let user = match own.get("USER") {
        Some(user) => Some(user.clone()),
        None => probe("/usr/bin/id", &["-un"])
            .await
            .map(|out| out.trim().to_owned()),
    };
    if let Some(user) = user.filter(|u| !u.is_empty() && !u.contains('/')) {
        let record = format!("/Users/{user}");
        if let Some(out) = probe("/usr/bin/dscl", &[".", "-read", &record, "UserShell"]).await
            && let Some(shell) = out
                .lines()
                .find_map(|l| l.strip_prefix("UserShell:"))
                .map(|s| PathBuf::from(s.trim()))
                .filter(|s| is_executable(s))
        {
            return shell;
        }
    }
    if let Some(out) = probe("/usr/bin/id", &["-P"]).await
        && let Some(shell) = out
            .trim()
            .rsplit(':')
            .next()
            .map(PathBuf::from)
            .filter(|s| s.is_absolute() && is_executable(s))
    {
        return shell;
    }
    tracing::warn!(
        fallback = FALLBACK_SHELL,
        "no login shell found in $SHELL, dscl or id"
    );
    PathBuf::from(FALLBACK_SHELL)
}

async fn login_path_within(
    shell: &Path,
    base: &BTreeMap<String, String>,
    timeout: Duration,
) -> Option<ShellReport> {
    for (flags, mode) in [
        (&["-l", "-i", "-c"][..], "interactive login"),
        (&["-l", "-c"][..], "login"),
    ] {
        match shell_path(shell, base, flags, timeout).await {
            Some(path) => {
                tracing::info!(shell = %shell.display(), mode, "PATH taken from the {mode} shell");
                return Some(path);
            }
            None => {
                tracing::debug!(shell = %shell.display(), mode, "the {mode} shell reported no PATH");
            }
        }
    }
    None
}

async fn shell_path(
    shell: &Path,
    base: &BTreeMap<String, String>,
    flags: &[&str],
    timeout: Duration,
) -> Option<ShellReport> {
    let fish = shell.file_name().is_some_and(|n| n == "fish");
    let names = CAPTURED.join(" ");
    // printenv answers only for exported variables, which are what the CLIs would see in a terminal.
    let script = if fish {
        format!(
            "printf '\\n{PATH_BEGIN}%s{PATH_END}\\n' (string join : $PATH); for v in {names}; if set val (/usr/bin/printenv $v); printf '\\n{VAR_BEGIN}%s=%s{VAR_END}\\n' $v \"$val\"; end; end"
        )
    } else {
        format!(
            "printf '\\n{PATH_BEGIN}%s{PATH_END}\\n' \"$PATH\"; for v in {names}; do if val=$(/usr/bin/printenv \"$v\"); then printf '\\n{VAR_BEGIN}%s=%s{VAR_END}\\n' \"$v\" \"$val\"; fi; done"
        )
    };
    let mut cmd = Command::new(shell);
    cmd.args(flags)
        .arg(&script)
        .env_clear()
        .envs(base)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Some(home) = base.get("HOME") {
        cmd.current_dir(home);
    }
    let out = run_within(cmd, shell, timeout).await?;
    report(&out)
}

/// The `PATH` between the last begin marker and its end marker, and the [`CAPTURED`] variables marked after it.
fn report(out: &str) -> Option<ShellReport> {
    let path = marked_path(out)?;
    let (_, after) = out.rsplit_once(PATH_BEGIN)?;
    let vars = after
        .split(VAR_BEGIN)
        .skip(1)
        .filter_map(|marked| marked.split_once(VAR_END))
        .filter_map(|(pair, _)| pair.split_once('='))
        .filter(|(name, _)| CAPTURED.contains(name))
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect();
    Some(ShellReport { path, vars })
}

/// The text between the last begin marker and the end marker after it; `.zshrc` output around it is ignored.
fn marked_path(out: &str) -> Option<String> {
    let (_, rest) = out.rsplit_once(PATH_BEGIN)?;
    let (path, _) = rest.split_once(PATH_END)?;
    (!path.is_empty()).then(|| path.to_owned())
}

async fn probe(program: &str, args: &[&str]) -> Option<String> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    run(cmd, Path::new(program)).await
}

async fn run(cmd: Command, program: &Path) -> Option<String> {
    run_within(cmd, program, PROBE_TIMEOUT).await
}

async fn run_within(mut cmd: Command, program: &Path, timeout: Duration) -> Option<String> {
    match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(Ok(out)) if out.status.success() => {
            Some(String::from_utf8_lossy(&out.stdout).into_owned())
        }
        Ok(Ok(out)) => {
            tracing::debug!(program = %program.display(), status = %out.status, "probe exited unsuccessfully");
            None
        }
        Ok(Err(e)) => {
            tracing::warn!(program = %program.display(), error = %e, "probe could not run");
            None
        }
        Err(_) => {
            tracing::warn!(program = %program.display(), timeout_ms = timeout.as_millis(), "probe timed out");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_base_env_keeps_only_the_allow_list_and_sets_the_terminal() {
        let own = BTreeMap::from([
            ("HOME".to_owned(), "/Users/example".to_owned()),
            ("SECRET_TOKEN".to_owned(), "x".to_owned()),
            ("PATH".to_owned(), "/opt/example/bin".to_owned()),
        ]);
        let env = base_env(&own, Path::new("/bin/zsh"));
        assert_eq!(env.get("HOME").map(String::as_str), Some("/Users/example"));
        assert_eq!(env.get("LANG").map(String::as_str), Some(DEFAULT_LANG));
        assert_eq!(env.get("TERM").map(String::as_str), Some("xterm-256color"));
        assert_eq!(env.get("COLORTERM").map(String::as_str), Some("truecolor"));
        assert_eq!(env.get("SHELL").map(String::as_str), Some("/bin/zsh"));
        assert!(!env.contains_key("SECRET_TOKEN"));
        assert!(!env.contains_key("PATH"), "PATH comes from the login shell");
    }

    #[test]
    fn which_finds_executables_only() {
        assert_eq!(
            which_in("relative:/bin", "sh"),
            Some(PathBuf::from("/bin/sh"))
        );
        assert_eq!(which_in("/etc", "hosts"), None, "not executable");
        assert_eq!(which_in("/bin", "no-such-program-example"), None);
    }

    fn fake_shell(name: &str, body: &str) -> (PathBuf, BTreeMap<String, String>) {
        let dir = std::env::temp_dir().join(format!("ply-login-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let shell = dir.join("fake-shell");
        std::fs::write(&shell, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
        let own = BTreeMap::from([("HOME".to_owned(), dir.display().to_string())]);
        let base = base_env(&own, &shell);
        (shell, base)
    }

    const RUN_LAST_ARG: &str = r#"for a in "$@"; do last=$a; done; exec /bin/sh -c "$last""#;

    #[tokio::test]
    async fn the_path_comes_from_an_interactive_login_shell_first() {
        let body = format!(
            "case \"$*\" in *-i*) echo 'rc noise'; PATH=/example/interactive:/bin ;; *) PATH=/example/login:/bin ;; esac\nexport PATH\n{RUN_LAST_ARG}"
        );
        let (shell, base) = fake_shell("interactive", &body);
        let path = login_path_within(&shell, &base, Duration::from_secs(5))
            .await
            .map(|r| r.path);
        assert_eq!(path.as_deref(), Some("/example/interactive:/bin"));
    }

    #[tokio::test]
    async fn the_probe_passes_on_only_the_captured_variables_the_shell_exports() {
        let body = format!(
            "export CODEX_HOME=/example/codex-home https_proxy=http://proxy.example:3128 OPENAI_API_KEY=example-secret\nNO_PROXY=not-exported\nPATH=/example/bin:/bin\nexport PATH\n{RUN_LAST_ARG}"
        );
        let (shell, base) = fake_shell("captured", &body);
        let report = login_path_within(&shell, &base, Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(report.path, "/example/bin:/bin");
        assert_eq!(
            report.vars.get("CODEX_HOME").map(String::as_str),
            Some("/example/codex-home")
        );
        assert_eq!(
            report.vars.get("https_proxy").map(String::as_str),
            Some("http://proxy.example:3128")
        );
        assert_eq!(
            report.vars.get("LANG").map(String::as_str),
            Some(DEFAULT_LANG),
            "the locale plyd gave the probe comes back"
        );
        assert!(!report.vars.contains_key("OPENAI_API_KEY"), "never a key");
        assert!(
            !report.vars.contains_key("NO_PROXY"),
            "a shell variable that is not exported"
        );
        assert!(report.vars.keys().all(|k| CAPTURED.contains(&k.as_str())));
    }

    #[tokio::test]
    async fn a_hanging_interactive_shell_falls_back_to_a_login_shell() {
        let body = format!(
            "case \"$*\" in *-i*) exec sleep 30 ;; esac\nPATH=/example/login:/bin\nexport PATH\n{RUN_LAST_ARG}"
        );
        let (shell, base) = fake_shell("hanging", &body);
        let started = std::time::Instant::now();
        let path = login_path_within(&shell, &base, Duration::from_millis(500))
            .await
            .map(|r| r.path);
        assert_eq!(path.as_deref(), Some("/example/login:/bin"));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the hang is cut at the timeout"
        );
    }

    #[tokio::test]
    async fn a_late_answer_replaces_the_fallback_for_every_later_launch() {
        let body = format!(
            "n=$(cat \"$HOME/slow\" 2>/dev/null || echo 0)\nif [ \"$n\" -gt 0 ]; then echo $((n - 1)) > \"$HOME/slow\"; exec sleep 30; fi\nPATH=/example/late:/bin\nexport PATH\n{RUN_LAST_ARG}"
        );
        let (shell, base) = fake_shell("late", &body);
        std::fs::write(Path::new(&base["HOME"]).join("slow"), "2").unwrap();
        let fallback = "/usr/bin:/bin".to_owned();
        let env = LoginEnv::probed(shell, base, fallback, Duration::from_millis(300)).await;
        assert!(!env.from_shell, "both startup probes hung");
        assert_eq!(env.path(), "/usr/bin:/bin");
        let login = Login::with_late_timeout(env, Duration::from_secs(5));
        let settled = login.settled().await;
        assert!(settled.from_shell);
        assert_eq!(settled.path(), "/example/late:/bin");
        assert_eq!(login.get().path(), "/example/late:/bin");
    }

    #[tokio::test]
    async fn a_missed_late_probe_is_not_repeated_within_the_interval() {
        let (shell, base) = fake_shell("mute", "exec sleep 30");
        let fallback = "/usr/bin:/bin".to_owned();
        let env = LoginEnv::probed(shell, base, fallback, Duration::from_millis(200)).await;
        let login = Login::with_late_timeout(env, Duration::from_millis(200));
        assert!(!login.settled().await.from_shell);
        let started = Instant::now();
        assert_eq!(login.settled().await.path(), "/usr/bin:/bin");
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "no second probe within LATE_PROBE_INTERVAL"
        );
    }

    #[test]
    fn the_path_is_read_between_the_markers() {
        let out = format!("motd\n{PATH_BEGIN}/a:/b{PATH_END}\ntrailing");
        assert_eq!(marked_path(&out).as_deref(), Some("/a:/b"));
        assert_eq!(marked_path(&format!("{PATH_BEGIN}{PATH_END}")), None);
        assert_eq!(marked_path("no markers"), None);
        let out = format!(
            "{VAR_BEGIN}CODEX_HOME=/noise{VAR_END}\n{PATH_BEGIN}/a{PATH_END}\n{VAR_BEGIN}CLAUDE_CONFIG_DIR=/a=b{VAR_END}\n{VAR_BEGIN}AWS_SECRET_ACCESS_KEY=x{VAR_END}\n{VAR_BEGIN}broken\n"
        );
        let report = report(&out).unwrap();
        assert_eq!(
            report.vars,
            BTreeMap::from([("CLAUDE_CONFIG_DIR".to_owned(), "/a=b".to_owned())]),
            "only captured names after the PATH, split at the first ="
        );
    }

    #[tokio::test]
    async fn a_posix_login_shell_reports_its_path() {
        let base = base_env(
            &BTreeMap::from([(
                "HOME".to_owned(),
                std::env::temp_dir().display().to_string(),
            )]),
            Path::new("/bin/sh"),
        );
        let path = login_path_within(Path::new("/bin/sh"), &base, PATH_PROBE_TIMEOUT)
            .await
            .unwrap()
            .path;
        assert!(path.split(':').any(|d| d == "/bin"), "{path}");
    }
}
