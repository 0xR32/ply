//! The user's login shell and the environment every pane process starts with (C5).
//!
//! plyd runs under launchd with a minimal environment, so it resolves two things once at startup: the login shell
//! (`$SHELL`, else the directory record via `dscl`, else the passwd entry via `id -P`, else `/bin/zsh`) and that
//! shell's `PATH` as an interactive login shell sees it (`$SHELL -l -i -c`, [`PATH_PROBE_TIMEOUT`] at most, stdin from
//! `/dev/null`): zsh reads `.zshrc` only when interactive, and that is where installers put `~/.local/bin`, home of the
//! `claude` and `codex` binaries (Ruling R42). A shell that fails or hangs interactively is asked again as a plain
//! login shell (`-l -c`); the log says which one answered. Children start from a cleared environment
//! (`env_clear`) with only [`LoginEnv::base`]: a short allow-list of plyd's own variables (home, user, temp dir,
//! locale, SSH agent), `SHELL`, the login `PATH`, `TERM=xterm-256color` and `COLORTERM=truecolor`, plus the launch
//! spec's additions. `LANG` defaults to `en_US.UTF-8` when plyd has none, because a CLI in the C locale draws no
//! UTF-8. Nothing here reads or writes the CLIs' own configuration (INV-8).

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

/// `PATH` used when the login shell cannot report one.
pub const FALLBACK_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// Shell used when no source names one (the macOS default).
pub const FALLBACK_SHELL: &str = "/bin/zsh";

/// `LANG` given to children when plyd itself has none.
pub const DEFAULT_LANG: &str = "en_US.UTF-8";

/// Longest wait for one probe (`dscl`, `id`).
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Longest wait for one `PATH` probe of the login shell.
pub const PATH_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

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

const PATH_BEGIN: &str = "__PLY_PATH_BEGIN__";
const PATH_END: &str = "__PLY_PATH_END__";

/// The login shell and the base environment of every pane process; resolved once, then read-only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginEnv {
    /// Absolute path of the login shell; shell panes run it as `<shell> -l`.
    pub shell: PathBuf,
    /// Variables every child starts with, `PATH` (the login shell's) included.
    pub base: BTreeMap<String, String>,
}

impl LoginEnv {
    /// Resolves the shell and its login `PATH` from plyd's environment; never fails (each step falls back and logs).
    /// Runs up to three short child processes, each bounded by [`PROBE_TIMEOUT`].
    pub async fn resolve() -> Self {
        let own: BTreeMap<String, String> = std::env::vars().collect();
        let shell = login_shell(&own).await;
        let mut base = base_env(&own, &shell);
        let path = match login_path(&shell, &base).await {
            Some(path) => path,
            None => {
                let path = own
                    .get("PATH")
                    .cloned()
                    .unwrap_or_else(|| FALLBACK_PATH.to_owned());
                tracing::warn!(shell = %shell.display(), %path, "the login shell reported no PATH; using plyd's");
                path
            }
        };
        base.insert("PATH".to_owned(), path);
        Self { shell, base }
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

async fn login_path(shell: &Path, base: &BTreeMap<String, String>) -> Option<String> {
    login_path_within(shell, base, PATH_PROBE_TIMEOUT).await
}

async fn login_path_within(
    shell: &Path,
    base: &BTreeMap<String, String>,
    timeout: Duration,
) -> Option<String> {
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
) -> Option<String> {
    let fish = shell.file_name().is_some_and(|n| n == "fish");
    let script = if fish {
        format!("printf '\\n{PATH_BEGIN}%s{PATH_END}\\n' (string join : $PATH)")
    } else {
        format!("printf '\\n{PATH_BEGIN}%s{PATH_END}\\n' \"$PATH\"")
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
    marked_path(&out)
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
        let path = login_path_within(&shell, &base, Duration::from_secs(5)).await;
        assert_eq!(path.as_deref(), Some("/example/interactive:/bin"));
    }

    #[tokio::test]
    async fn a_hanging_interactive_shell_falls_back_to_a_login_shell() {
        let body = format!(
            "case \"$*\" in *-i*) exec sleep 30 ;; esac\nPATH=/example/login:/bin\nexport PATH\n{RUN_LAST_ARG}"
        );
        let (shell, base) = fake_shell("hanging", &body);
        let started = std::time::Instant::now();
        let path = login_path_within(&shell, &base, Duration::from_millis(500)).await;
        assert_eq!(path.as_deref(), Some("/example/login:/bin"));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the hang is cut at the timeout"
        );
    }

    #[test]
    fn the_path_is_read_between_the_markers() {
        let out = format!("motd\n{PATH_BEGIN}/a:/b{PATH_END}\ntrailing");
        assert_eq!(marked_path(&out).as_deref(), Some("/a:/b"));
        assert_eq!(marked_path(&format!("{PATH_BEGIN}{PATH_END}")), None);
        assert_eq!(marked_path("no markers"), None);
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
        let path = login_path(Path::new("/bin/sh"), &base).await.unwrap();
        assert!(path.split(':').any(|d| d == "/bin"), "{path}");
    }
}
