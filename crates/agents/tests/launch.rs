//! C7 launch specs (spec 6.1/6.2, Rulings R6, R15, R26, R14): argv, env and generated files of both adapters, the
//! launch.json round trip, and the version check read from install metadata without running a CLI.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ply_agents::claude::settings::{HOOK_EVENTS, SETTINGS_FILE, claude_settings_json};
use ply_agents::codex::{notify_override, toml_string};
use ply_agents::{CliVersion, Error, LAUNCH_FILE, LaunchRequest, LaunchSpec, adapter};
use ply_proto::pane::{AgentCli, Cli, Settings};
use serde_json::Value;

const HOOK: &str = "/Applications/Ply.app/Contents/MacOS/ply-hook";
const SOCK: &str = "/Users/example/Library/Application Support/ply/run/hook.sock";
const PANE_DIR: &str = "/Users/example/Library/Application Support/ply/run/panes/7";

fn request<'a>(cli: AgentCli, settings: &'a Settings) -> LaunchRequest<'a> {
    LaunchRequest {
        pane_id: 7,
        program: Path::new(match cli {
            AgentCli::Claude => "/Users/example/.local/bin/claude",
            AgentCli::Codex => "/Users/example/.local/bin/codex",
        }),
        cwd: Path::new("/Users/example/project"),
        hook_program: Path::new(HOOK),
        hook_socket: Path::new(SOCK),
        pane_dir: Path::new(PANE_DIR),
        settings,
        worktree: None,
        resume: None,
        prompt: None,
        status_line: None,
    }
}

fn settings_path() -> String {
    format!("{PANE_DIR}/{SETTINGS_FILE}")
}

#[test]
fn claude_argv_env_and_settings_file_follow_spec_6_1() {
    let settings = Settings::default();
    let launch = adapter(AgentCli::Claude)
        .launch(&request(AgentCli::Claude, &settings))
        .unwrap();
    assert_eq!(launch.spec.cli, Cli::Claude);
    assert_eq!(
        launch.spec.argv,
        [
            "/Users/example/.local/bin/claude",
            "--settings",
            &settings_path()
        ]
    );
    assert_eq!(launch.spec.cwd, "/Users/example/project");
    let env: Vec<(&str, &str)> = launch
        .spec
        .env
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_eq!(
        env,
        [
            ("CLAUDE_CODE_FORCE_SYNC_OUTPUT", "1"),
            ("COLORTERM", "truecolor"),
            ("PLY_HOOK_SOCK", SOCK),
            ("PLY_PANE_ID", "7"),
            ("TERM", "xterm-256color"),
        ]
    );
    assert_eq!(launch.files.len(), 1);
    assert_eq!(launch.files[0].path, PathBuf::from(settings_path()));
    assert_eq!(
        launch.files[0].contents,
        claude_settings_json(Path::new(HOOK), true, None).unwrap()
    );
}

#[test]
fn claude_worktree_resume_and_prompt_are_passed_through() {
    let settings = Settings::default();
    let mut req = request(AgentCli::Claude, &settings);
    req.worktree = Some("feat-x");
    req.resume = Some("00000000-0000-4000-8000-000000000000");
    req.prompt = Some("fix the build");
    let spec = adapter(AgentCli::Claude).launch(&req).unwrap().spec;
    assert_eq!(
        spec.argv[3..],
        [
            "--worktree",
            "feat-x",
            "--resume",
            "00000000-0000-4000-8000-000000000000",
            "fix the build"
        ]
    );
    assert_eq!(spec.worktree.as_deref(), Some("feat-x"));
    assert_eq!(
        spec.resume.as_deref(),
        Some("00000000-0000-4000-8000-000000000000")
    );
}

#[test]
fn values_that_would_read_as_options_are_refused() {
    let settings = Settings::default();
    for (worktree, resume) in [
        (Some("-rf"), None),
        (Some(""), None),
        (None, Some("--help")),
    ] {
        let mut req = request(AgentCli::Claude, &settings);
        req.worktree = worktree;
        req.resume = resume;
        let err = adapter(AgentCli::Claude).launch(&req).unwrap_err();
        assert!(matches!(err, Error::InvalidLaunch(_)), "{err}");
    }
    let mut req = request(AgentCli::Claude, &settings);
    req.prompt = Some("--version?");
    let argv = adapter(AgentCli::Claude).launch(&req).unwrap().spec.argv;
    assert_eq!(argv[3..], ["--", "--version?"]);
}

#[test]
fn claude_settings_contain_only_hooks_the_status_line_and_the_theme() {
    let with_theme: Value =
        serde_json::from_str(&claude_settings_json(Path::new(HOOK), true, None).unwrap()).unwrap();
    let keys: Vec<&String> = with_theme.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["hooks", "statusLine", "theme"]);
    assert_eq!(with_theme["theme"], "dark-ansi");
    let without: Value =
        serde_json::from_str(&claude_settings_json(Path::new(HOOK), false, None).unwrap()).unwrap();
    assert_eq!(
        without.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["hooks", "statusLine"]
    );

    let hooks = with_theme["hooks"].as_object().unwrap();
    let registered: BTreeSet<&str> = hooks.keys().map(String::as_str).collect();
    assert_eq!(registered, HOOK_EVENTS.into_iter().collect());
    assert_eq!(registered.len(), 12);
    for event in hooks.keys() {
        assert!(
            !event.starts_with("Worktree"),
            "{event} must never be registered (R15)"
        );
        let groups = hooks[event].as_array().unwrap();
        assert_eq!(groups.len(), 1);
        assert!(groups[0].get("matcher").is_none());
        let handlers = groups[0]["hooks"].as_array().unwrap();
        assert_eq!(handlers.len(), 1);
        assert_eq!(handlers[0]["type"], "command");
        assert_eq!(handlers[0]["command"], format!("'{HOOK}' claude {event}"));
    }
}

#[test]
fn the_status_line_reports_to_plyd_then_runs_the_users_own_with_their_other_keys() {
    let status = |user: Option<Value>| -> Value {
        let json = claude_settings_json(Path::new(HOOK), false, user.as_ref()).unwrap();
        serde_json::from_str::<Value>(&json).unwrap()["statusLine"].clone()
    };
    assert_eq!(
        status(None),
        serde_json::json!({"type": "command", "command": format!("'{HOOK}' statusline")})
    );
    let own = serde_json::json!({
        "type": "command",
        "command": "~/.claude/it's line.sh --x",
        "refreshInterval": 2,
        "padding": 0
    });
    assert_eq!(
        status(Some(own)),
        serde_json::json!({
            "type": "command",
            "command": format!(r"'{HOOK}' statusline '~/.claude/it'\''s line.sh --x'"),
            "refreshInterval": 2,
            "padding": 0
        })
    );
    for ignored in [
        serde_json::json!({"type": "static", "command": "echo no"}),
        serde_json::json!({"type": "command", "command": "   "}),
        serde_json::json!("echo"),
    ] {
        assert_eq!(
            status(Some(ignored.clone()))["command"],
            format!("'{HOOK}' statusline"),
            "{ignored}"
        );
    }
}

#[test]
fn hook_program_paths_are_shell_quoted() {
    let json = claude_settings_json(
        Path::new("/Users/example/My Apps/it's/ply-hook"),
        false,
        None,
    )
    .unwrap();
    let settings: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        settings["hooks"]["Stop"][0]["hooks"][0]["command"],
        r"'/Users/example/My Apps/it'\''s/ply-hook' claude Stop"
    );
}

#[test]
fn codex_argv_carries_the_per_invocation_overrides() {
    let settings = Settings::default();
    let launch = adapter(AgentCli::Codex)
        .launch(&request(AgentCli::Codex, &settings))
        .unwrap();
    assert_eq!(launch.spec.cli, Cli::Codex);
    assert_eq!(
        launch.spec.argv,
        [
            "/Users/example/.local/bin/codex",
            "-c",
            &format!(r#"notify=["{HOOK}","codex"]"#),
            "-c",
            r#"tui.notification_method="osc9""#,
            "-c",
            r#"tui.notification_condition="always""#,
            "-c",
            "tools.update_plan.enabled=true",
        ]
    );
    assert!(launch.files.is_empty());
    let keys: Vec<&str> = launch.spec.env.keys().map(String::as_str).collect();
    assert_eq!(keys, ["COLORTERM", "PLY_HOOK_SOCK", "PLY_PANE_ID", "TERM"]);
}

#[test]
fn codex_plan_tool_setting_drops_the_override() {
    let settings = Settings {
        codex_plan_tool: false,
        ..Settings::default()
    };
    let argv = adapter(AgentCli::Codex)
        .launch(&request(AgentCli::Codex, &settings))
        .unwrap()
        .spec
        .argv;
    assert!(!argv.iter().any(|a| a.contains("update_plan")));
    assert_eq!(argv.iter().filter(|a| *a == "-c").count(), 3);
}

#[test]
fn codex_resume_and_prompt_follow_the_overrides() {
    let settings = Settings::default();
    let mut req = request(AgentCli::Codex, &settings);
    req.resume = Some("00000000-0000-7000-8000-000000000001");
    req.prompt = Some("continue");
    let spec = adapter(AgentCli::Codex).launch(&req).unwrap().spec;
    assert_eq!(
        spec.argv[spec.argv.len() - 3..],
        ["resume", "00000000-0000-7000-8000-000000000001", "continue"]
    );
    assert_eq!(
        spec.resume.as_deref(),
        Some("00000000-0000-7000-8000-000000000001")
    );

    req.resume = Some("not-a-thread");
    assert!(matches!(
        adapter(AgentCli::Codex).launch(&req),
        Err(Error::InvalidLaunch(_))
    ));
    req.resume = None;
    req.worktree = Some("feat-x");
    assert!(matches!(
        adapter(AgentCli::Codex).launch(&req),
        Err(Error::InvalidLaunch(_))
    ));
}

#[test]
fn notify_value_is_a_toml_array_of_escaped_strings() {
    assert_eq!(toml_string(r#"a "b" \c"#), r#""a \"b\" \\c""#);
    assert_eq!(toml_string("tab\there"), r#""tab\u0009here""#);
    assert_eq!(
        notify_override("/Users/example/My \"Apps\"/ply-hook"),
        r#"notify=["/Users/example/My \"Apps\"/ply-hook","codex"]"#
    );
}

#[test]
fn non_utf8_paths_are_refused() {
    let settings = Settings::default();
    let bad = PathBuf::from(OsStr::from_bytes(b"/Users/example/\xff/codex"));
    let mut req = request(AgentCli::Codex, &settings);
    req.program = &bad;
    assert!(matches!(
        adapter(AgentCli::Codex).launch(&req),
        Err(Error::NonUtf8Path(_))
    ));
}

#[test]
fn launch_spec_round_trips_as_launch_json() {
    let settings = Settings::default();
    let mut req = request(AgentCli::Claude, &settings);
    req.worktree = Some("feat-x");
    let spec = adapter(AgentCli::Claude).launch(&req).unwrap().spec;
    let json = spec.to_json().unwrap();
    assert_eq!(LaunchSpec::from_json(json.as_bytes()).unwrap(), spec);
    assert_eq!(LAUNCH_FILE, "launch.json");

    let mut doc: Value = serde_json::from_str(&json).unwrap();
    doc["extra"] = Value::Bool(true);
    assert!(LaunchSpec::from_json(doc.to_string().as_bytes()).is_err());
    let empty = r#"{"cli":"codex","argv":[],"env":{},"cwd":"/Users/example"}"#;
    assert!(matches!(
        LaunchSpec::from_json(empty.as_bytes()),
        Err(Error::InvalidLaunch(_))
    ));
}

#[test]
fn adapters_declare_minimums_probes_and_quiet_timeouts() {
    let claude = adapter(AgentCli::Claude);
    let codex = adapter(AgentCli::Codex);
    assert_eq!(claude.min_version(), CliVersion::new(2, 1, 282));
    assert_eq!(codex.min_version(), CliVersion::new(0, 156, 1));
    assert_eq!(claude.version_probe_args(), ["--version"]);
    assert_eq!(codex.version_probe_args(), ["--version"]);
    assert_eq!(claude.quiet_timeout(), Some(Duration::from_secs(5)));
    assert_eq!(codex.quiet_timeout(), None);

    assert!(codex.check_version(&"0.156.1".parse().unwrap()).is_ok());
    assert!(codex.check_version(&"0.157.0".parse().unwrap()).is_ok());
    let err = codex
        .check_version(&"0.155.9".parse().unwrap())
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "codex 0.155.9 is older than the supported minimum 0.156.1"
    );
    assert!(matches!(
        claude.check_version(&"2.1.281".parse().unwrap()),
        Err(Error::CliTooOld { .. })
    ));
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("launch")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, contents: &str) -> PathBuf {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
    path.to_path_buf()
}

fn link(target: &Path, at: &Path) -> PathBuf {
    std::fs::create_dir_all(at.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(target, at).unwrap();
    at.to_path_buf()
}

fn version_of(cli: AgentCli, exe: &Path) -> String {
    adapter(cli).installed_version(exe).unwrap().to_string()
}

#[test]
fn claude_version_comes_from_the_install_layout() {
    let root = scratch("claude");
    let native = write(&root.join("share/claude/versions/2.1.282"), "binary");
    let on_path = link(&native, &root.join("bin/claude"));
    assert_eq!(version_of(AgentCli::Claude, &on_path), "2.1.282");

    let pkg = root.join("lib/node_modules/@anthropic-ai/claude-code");
    write(
        &pkg.join("package.json"),
        r#"{"name":"@anthropic-ai/claude-code","version":"2.1.290"}"#,
    );
    let cli_js = write(&pkg.join("cli.js"), "#!/usr/bin/env node");
    assert_eq!(
        version_of(
            AgentCli::Claude,
            &link(&cli_js, &root.join("npm-bin/claude"))
        ),
        "2.1.290"
    );

    let cask = write(&root.join("Caskroom/claude-code/2.1.300/claude"), "binary");
    assert_eq!(version_of(AgentCli::Claude, &cask), "2.1.300");

    let other = write(&root.join("opt/claude"), "binary");
    let err = adapter(AgentCli::Claude)
        .installed_version(&other)
        .unwrap_err();
    assert!(matches!(err, Error::VersionUnknown { .. }), "{err}");
    let missing = adapter(AgentCli::Claude)
        .installed_version(&root.join("absent"))
        .unwrap_err();
    assert!(matches!(missing, Error::Io { .. }), "{missing}");
}

#[test]
fn codex_version_comes_from_the_package_metadata() {
    let root = scratch("codex");
    let release = root.join("codex-home/packages/standalone/releases/0.156.1-aarch64-apple-darwin");
    write(
        &release.join("codex-package.json"),
        r#"{"layoutVersion":1,"version":"0.156.1","target":"aarch64-apple-darwin","entrypoint":"bin/codex"}"#,
    );
    let exe = write(&release.join("bin/codex"), "binary");
    let current = link(
        &release,
        &root.join("codex-home/packages/standalone/current"),
    );
    let on_path = link(&current.join("bin/codex"), &root.join("bin/codex"));
    assert_eq!(version_of(AgentCli::Codex, &on_path), "0.156.1");
    assert_eq!(version_of(AgentCli::Codex, &exe), "0.156.1");

    let pkg = root.join("lib/node_modules/@openai/codex");
    write(
        &pkg.join("package.json"),
        r#"{"name":"@openai/codex","version":"0.158.0"}"#,
    );
    let shim = write(&pkg.join("bin/codex.js"), "#!/usr/bin/env node");
    assert_eq!(
        version_of(AgentCli::Codex, &link(&shim, &root.join("npm-bin/codex"))),
        "0.158.0"
    );

    let other_pkg = root.join("lib/node_modules/not-codex");
    write(
        &other_pkg.join("package.json"),
        r#"{"name":"not-codex","version":"9.9.9"}"#,
    );
    let impostor = write(&other_pkg.join("bin/codex"), "binary");
    assert!(matches!(
        adapter(AgentCli::Codex).installed_version(&impostor),
        Err(Error::VersionUnknown { .. })
    ));

    let cask = write(
        &root.join("Caskroom/codex/0.157.0/codex-aarch64-apple-darwin"),
        "binary",
    );
    assert_eq!(version_of(AgentCli::Codex, &cask), "0.157.0");
    let cellar = write(&root.join("Cellar/codex/0.156.2/bin/codex"), "binary");
    assert_eq!(version_of(AgentCli::Codex, &cellar), "0.156.2");

    let broken = root.join("broken");
    write(&broken.join("codex-package.json"), "{not json");
    let exe = write(&broken.join("bin/codex"), "binary");
    assert!(matches!(
        adapter(AgentCli::Codex).installed_version(&exe),
        Err(Error::Json { .. })
    ));
}

#[test]
fn a_prompt_is_acknowledged_by_userpromptsubmit_in_claude_and_task_started_in_codex() {
    use ply_agents::{StatusSignal, adapter};
    use ply_proto::pane::AgentCli;
    let claude = adapter(AgentCli::Claude);
    let codex = adapter(AgentCli::Codex);
    assert!(claude.acknowledges_prompt(&StatusSignal::PromptSubmitted));
    assert!(!claude.acknowledges_prompt(&StatusSignal::TurnStarted));
    assert!(codex.acknowledges_prompt(&StatusSignal::TurnStarted));
    assert!(
        !codex.acknowledges_prompt(&StatusSignal::PromptSubmitted),
        "Codex's Enter is only a guess until the rollout confirms a turn (R48)"
    );
    for other in [
        StatusSignal::Ready,
        StatusSignal::TurnComplete,
        StatusSignal::KeyTyped,
        StatusSignal::NoTurnStarted,
    ] {
        assert!(!claude.acknowledges_prompt(&other) && !codex.acknowledges_prompt(&other));
    }
}

#[test]
fn claude_reads_an_image_for_a_pasted_piece_ending_in_an_image_extension_and_codex_never_delays() {
    let claude = adapter(AgentCli::Claude);
    let codex = adapter(AgentCli::Codex);
    for text in [
        "/Users/example/Desktop/Screenshot\\ 2026-09-27\\ at\\ 10.15.32.png",
        "Fix the layout shown here\n/Users/example/Desktop/shot.png",
        "fix this /Users/example/shot.PNG",
        "'/Users/example/My Shots/a.jpeg'",
        "\"/Users/example/b.jpg\"",
        "/tmp/a.gif /tmp/b.webp",
        "/tmp/one.png\n\nthen compare it with the design",
        "optimize logo.png",
        "/tmp/a.pn\\g",
        "see x.png C:\\notes",
        "'/tmp/unbalanced.png",
    ] {
        assert!(claude.paste_reads_images(text), "{text:?}");
        assert!(!codex.paste_reads_images(text), "{text:?}");
    }
    for text in [
        "",
        "/review-pr #1",
        "look at /tmp/a.png please",
        "/tmp/notes.pdf",
        "/tmp/icon.svg",
        "/tmp/a.png.txt",
    ] {
        assert!(!claude.paste_reads_images(text), "{text:?}");
    }
}

#[test]
fn only_a_signal_of_the_running_process_at_its_prompt_opens_the_task_queue() {
    use ply_agents::StatusSignal;
    use ply_proto::pane::AgentCli;
    let claude = adapter(AgentCli::Claude);
    let codex = adapter(AgentCli::Codex);
    for signal in [
        StatusSignal::Ready,
        StatusSignal::PromptSubmitted,
        StatusSignal::TurnComplete,
    ] {
        assert!(claude.shows_prompt(&signal), "{signal:?}");
    }
    for signal in [StatusSignal::TurnStarted, StatusSignal::TurnComplete] {
        assert!(codex.shows_prompt(&signal), "{signal:?}");
    }
    assert!(
        !codex.shows_prompt(&StatusSignal::Ready),
        "Codex is ready from its first byte, which a trust, hooks or update screen prints too"
    );
    assert!(
        !codex.shows_prompt(&StatusSignal::PromptSubmitted),
        "an Enter is only a guess (R48)"
    );
    for other in [
        StatusSignal::KeyTyped,
        StatusSignal::NoTurnStarted,
        StatusSignal::QuietTimeout,
    ] {
        assert!(!claude.shows_prompt(&other) && !codex.shows_prompt(&other));
    }
}
