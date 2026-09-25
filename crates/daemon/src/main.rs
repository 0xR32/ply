//! `plyd`, ply's background daemon: the command line over the `ply_daemon` library (spec 3.1, 11.3, WP4).
//!
//! `plyd [--foreground] [--run-dir <dir>]` serves panes until `daemon.shutdown`, SIGTERM, SIGINT or SIGHUP. launchd
//! starts it without flags; `--foreground` also logs to stderr for a terminal, and `--run-dir` moves the sockets
//! (tests). `plyd install-agent [--dry-run]` installs and starts the LaunchAgent (Ruling R38). Logs go to
//! `plyd.YYYY-MM-DD.log` in the log directory, kept 14 days, at the level in `PLY_LOG` (default `info`).
//!
//! A second plyd for the same data directory prints why it refuses and exits with status 0, so launchd's
//! restart-on-failure never loops on it. Any other startup failure (a newer database schema, an unbindable socket)
//! exits with status 1. plyd never renders and never links gpui (spec 3.2).

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Context;
use ply_daemon::daemon::{Options, run};
use ply_daemon::lock::InstanceLock;
use ply_daemon::paths::Paths;
use ply_daemon::{Error, launchd, power};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::Layer;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const USAGE: &str = "usage: plyd [--foreground] [--run-dir <dir>]
       plyd install-agent [--dry-run]
       plyd --version | --help
";

const LOG_DAYS: usize = 14;

enum Mode {
    Serve {
        foreground: bool,
        run_dir: Option<PathBuf>,
    },
    InstallAgent {
        dry_run: bool,
    },
    Version,
    Help,
}

fn parse(args: &[String]) -> Result<Mode, String> {
    match args.first().map(String::as_str) {
        Some("install-agent") => match &args[1..] {
            [] => Ok(Mode::InstallAgent { dry_run: false }),
            [flag] if flag == "--dry-run" => Ok(Mode::InstallAgent { dry_run: true }),
            other => Err(format!("unexpected install-agent arguments {other:?}")),
        },
        Some("--version") if args.len() == 1 => Ok(Mode::Version),
        Some("--help" | "-h") if args.len() == 1 => Ok(Mode::Help),
        _ => {
            let mut foreground = false;
            let mut run_dir = None;
            let mut it = args.iter();
            while let Some(arg) = it.next() {
                match arg.as_str() {
                    "--foreground" => foreground = true,
                    "--run-dir" => match it.next() {
                        Some(dir) if Path::new(dir).is_absolute() => {
                            run_dir = Some(PathBuf::from(dir));
                        }
                        Some(dir) => return Err(format!("--run-dir must be absolute, got {dir}")),
                        None => return Err("--run-dir needs a directory".to_owned()),
                    },
                    other => return Err(format!("unknown argument {other}")),
                }
            }
            Ok(Mode::Serve {
                foreground,
                run_dir,
            })
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse(&args) {
        Err(msg) => {
            eprintln!("plyd: {msg}\n{USAGE}");
            ExitCode::from(2)
        }
        Ok(Mode::Version) => {
            println!("plyd {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Ok(Mode::Help) => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Ok(Mode::InstallAgent { dry_run }) => match launchd::install_agent(dry_run) {
            Ok(report) => {
                print!("{report}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("plyd install-agent: {e}");
                ExitCode::FAILURE
            }
        },
        Ok(Mode::Serve {
            foreground,
            run_dir,
        }) => match serve(foreground, run_dir) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => match e.downcast_ref::<Error>() {
                Some(already @ Error::AlreadyRunning { .. }) => {
                    eprintln!("plyd: {already}; not starting a second one");
                    tracing::warn!(error = %already, "refused to start a second plyd");
                    ExitCode::SUCCESS
                }
                _ => {
                    eprintln!("plyd: {e:#}");
                    tracing::error!(error = format!("{e:#}"), "plyd failed");
                    ExitCode::FAILURE
                }
            },
        },
    }
}

fn serve(foreground: bool, run_dir: Option<PathBuf>) -> anyhow::Result<()> {
    let paths = Paths::from_env(run_dir)?;
    std::fs::create_dir_all(&paths.data_dir)
        .with_context(|| format!("cannot create {}", paths.data_dir.display()))?;
    init_logging(&paths.log_dir, foreground)?;
    let _lock = InstanceLock::acquire(&paths.lock())?;
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        data_dir = %paths.data_dir.display(),
        sandboxed = paths.sandboxed,
        "plyd starting"
    );
    let exe = std::env::current_exe().context("cannot locate the plyd executable")?;
    let hook_program = exe
        .parent()
        .map(|dir| dir.join("ply-hook"))
        .unwrap_or_default();
    if !hook_program.is_file() {
        tracing::warn!(hook = %hook_program.display(), "ply-hook is not beside plyd; agent hooks will not reach plyd");
    }
    let keep_awake = (!paths.sandboxed).then(power::caffeinate_command);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("plyd")
        .build()
        .context("cannot start the async runtime")?;
    runtime.block_on(run(Options {
        paths,
        hook_program,
        keep_awake,
    }))?;
    Ok(())
}

fn init_logging(log_dir: &Path, foreground: bool) -> anyhow::Result<()> {
    std::fs::create_dir_all(log_dir)
        .with_context(|| format!("cannot create {}", log_dir.display()))?;
    let level = std::env::var("PLY_LOG")
        .ok()
        .and_then(|v| v.parse::<LevelFilter>().ok())
        .unwrap_or(LevelFilter::INFO);
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("plyd")
        .filename_suffix("log")
        .max_log_files(LOG_DAYS)
        .build(log_dir)
        .context("cannot open the log file")?;
    let file = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(appender)
        .with_filter(level);
    let stderr = foreground.then(|| {
        tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_filter(level)
    });
    tracing_subscriber::registry()
        .with(file)
        .with(stderr)
        .try_init()
        .context("cannot install the logger")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn the_command_line_selects_the_mode() {
        assert!(matches!(
            parse(&args(&[])),
            Ok(Mode::Serve {
                foreground: false,
                run_dir: None
            })
        ));
        assert!(matches!(
            parse(&args(&["--foreground", "--run-dir", "/tmp/r"])),
            Ok(Mode::Serve { foreground: true, run_dir: Some(d) }) if d == Path::new("/tmp/r")
        ));
        assert!(matches!(
            parse(&args(&["install-agent", "--dry-run"])),
            Ok(Mode::InstallAgent { dry_run: true })
        ));
        assert!(parse(&args(&["--run-dir", "rel"])).is_err());
        assert!(parse(&args(&["--bogus"])).is_err());
    }
}
