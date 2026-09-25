//! plyd end to end over its real sockets (spec WP4 verification): create, attach, detach and reattach with the same
//! screen; the forced Snapshot for a slow client; OSC 11 answered from ply's palette; a second plyd refused; pane
//! close, exit, restart and resume; an agent CLI launched from the login PATH; the child environment; the C1/C2
//! handshake checks; the LaunchAgent dry run.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::{Duration, Instant};

use common::{Control, Data, Sandbox, eventually, read_when_ready};
use ply_proto::control::{ErrorCode, Event};
use ply_proto::data::{Frame, RefuseReason};
use ply_proto::pane::PaneStatus;
use serde_json::json;

const WAIT: Duration = Duration::from_secs(10);

#[test]
fn a_shell_survives_detach_and_reattach_with_the_same_screen() {
    let sb = Sandbox::new("life");
    let _plyd = sb.start();
    let (mut c, ws) = sb.control();
    let pane = sb.shell(&mut c, ws);
    let mut d = sb.attach_ready(pane);
    d.input(b"echo ply-$((6*7))\r").unwrap();
    assert!(
        d.pump_until(WAIT, |d| d.shows("ply-42")).unwrap(),
        "{:?}",
        d.screen()
    );
    d.settle(Duration::from_millis(300)).unwrap();
    let before = d.replica.clone();
    drop(d);
    drop(c);

    std::thread::sleep(Duration::from_millis(200));
    let (mut again, first) = Data::attach(&sb.data_socket(), pane, 80, 24).unwrap();
    assert!(
        matches!(first, Frame::Snapshot(_)),
        "reattach answers with a Snapshot"
    );
    again.apply(&first, true).unwrap();
    assert_eq!(again.screen(), before.screen_text());
    for y in 0..24 {
        assert_eq!(
            again.replica.resolved_row(y),
            before.resolved_row(y),
            "row {y}"
        );
    }
    assert_eq!(again.replica.cursor(), before.cursor());

    let (mut c, _) = sb.control();
    let panes = c.call("pane.list", json!({"workspace_id": ws})).unwrap();
    assert_eq!(panes[0]["id"], pane);
    assert_eq!(panes[0]["status"], "idle");
}

#[test]
fn a_client_that_stops_acking_gets_a_forced_snapshot_after_3_s() {
    let sb = Sandbox::new("slow");
    let _plyd = sb.start();
    let (mut c, ws) = sb.control();
    let pane = sb.shell(&mut c, ws);
    let mut d = sb.attach_ready(pane);
    d.settle(Duration::from_millis(200)).unwrap();
    d.input(b"while :; do echo tick; sleep 0.02; done\r")
        .unwrap();
    let mut deltas = 0;
    let mut fourth = None;
    let mut forced = None;
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline && forced.is_none() {
        let Some(frame) = d.recv(Duration::from_millis(200)).unwrap() else {
            continue;
        };
        match frame {
            Frame::Delta(_) => {
                deltas += 1;
                if deltas == 4 {
                    fourth = Some(Instant::now());
                }
            }
            Frame::Snapshot(_) => forced = Some(Instant::now()),
            _ => {}
        }
    }
    let (fourth, forced) = (
        fourth.expect("4 Deltas"),
        forced.expect("a forced Snapshot"),
    );
    assert_eq!(deltas, 4, "the window holds at most 4 unacked Deltas");
    let blocked = forced - fourth;
    assert!(
        blocked >= Duration::from_millis(2800) && blocked <= Duration::from_millis(4500),
        "forced Snapshot after {blocked:?}"
    );
    c.call("pane.close", json!({"pane_id": pane, "kill": true}))
        .unwrap();
}

#[test]
fn an_osc_11_query_is_answered_with_the_palette_background() {
    let sb = Sandbox::new("osc");
    let _plyd = sb.start();
    let (mut c, ws) = sb.control();
    let pane = sb.shell(&mut c, ws);
    let mut d = sb.attach_ready(pane);
    d.input(
        b"stty -echo -icanon min 0 time 10; printf '\\033]11;?\\007'; dd bs=256 count=1 2>/dev/null | od -An -c | tr -d ' \\n'; echo; stty sane\r",
    )
    .unwrap();
    assert!(
        d.pump_until(WAIT, |d| d.shows("rgb:0c0c/0e0e/1414"))
            .unwrap(),
        "{:?}",
        d.screen()
    );
}

#[test]
fn a_second_plyd_refuses_to_start() {
    let sb = Sandbox::new("twice");
    let mut first = sb.start();
    let out = sb.plyd(&["--foreground"]).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "exit 0 keeps launchd from restarting it: {stderr}"
    );
    assert!(stderr.contains("already running"), "{stderr}");
    assert!(first.is_running());
    let (mut c, _) = sb.control();
    assert!(
        c.call("workspace.list", json!({})).is_ok(),
        "the first one still serves"
    );
}

#[test]
fn a_live_pane_closes_only_with_kill_and_its_session_is_kept() {
    let sb = Sandbox::new("close");
    let _plyd = sb.start();
    let (mut c, ws) = sb.control();
    let pane = sb.shell(&mut c, ws);
    let refused = c
        .call("pane.close", json!({"pane_id": pane, "kill": false}))
        .unwrap_err();
    assert_eq!(refused.code, ErrorCode::PaneAlive);
    c.call("pane.close", json!({"pane_id": pane, "kill": true}))
        .unwrap();
    let exited = c
        .wait_event(
            WAIT,
            |e| matches!(e, Event::PaneExit(x) if x.pane_id == pane),
        )
        .expect("pane.exit");
    let Event::PaneExit(exit) = exited else {
        unreachable!()
    };
    assert_eq!(exit.code, 128 + 1, "SIGHUP");
    assert!(
        c.wait_event(
            WAIT,
            |e| matches!(e, Event::PaneRemoved(r) if r.pane_id == pane)
        )
        .is_some()
    );
    let open = c.call("pane.list", json!({"workspace_id": ws})).unwrap();
    assert_eq!(open, json!([]));
    let sessions = c
        .call(
            "session.list",
            json!({"workspace_id": ws, "include_closed": true}),
        )
        .unwrap();
    assert_eq!(sessions[0]["pane_id"], pane);
    assert_eq!(sessions[0]["status"], "exited");
    assert_eq!(sessions[0]["exit_code"], 129);
    assert!(sessions[0]["closed_at"].is_u64());
    assert!(
        !sb.ply_home
            .join("run/panes")
            .join(pane.to_string())
            .exists()
    );
}

#[test]
fn an_exiting_shell_reports_its_code_on_c1_and_c2() {
    let sb = Sandbox::new("exit");
    let _plyd = sb.start();
    let (mut c, ws) = sb.control();
    let pane = sb.shell(&mut c, ws);
    let mut d = sb.attach_ready(pane);
    d.input(b"echo bye; exit 3\r").unwrap();
    assert!(d.pump_until(WAIT, |d| d.exit.is_some()).unwrap());
    assert_eq!(d.exit, Some(3));
    assert!(
        d.shows("bye"),
        "the last output arrives before EXIT: {:?}",
        d.screen()
    );
    let status = c
        .wait_event(WAIT, |e| matches!(e, Event::PaneStatus(s) if s.pane_id == pane && s.status == PaneStatus::Exited))
        .expect("pane.status exited");
    let Event::PaneStatus(status) = status else {
        unreachable!()
    };
    assert_eq!(status.exit_code, Some(3));
    let (again, first) = Data::attach(&sb.data_socket(), pane, 80, 24).unwrap();
    assert!(matches!(first, Frame::Snapshot(_)));
    let mut again = again;
    again.apply(&first, true).unwrap();
    assert!(
        again.pump_until(WAIT, |d| d.exit == Some(3)).unwrap(),
        "an exited pane still attaches"
    );
    assert!(again.shows("bye"));
    c.call("pane.close", json!({"pane_id": pane, "kill": false}))
        .unwrap();
}

#[test]
fn a_restart_marks_live_panes_lost_and_resume_relaunches_them() {
    let sb = Sandbox::new("restart");
    let mut plyd = sb.start();
    let (mut c, ws) = sb.control();
    let pane = sb.shell(&mut c, ws);
    assert!(
        sb.ply_home
            .join("run/panes")
            .join(pane.to_string())
            .join("launch.json")
            .is_file()
    );
    let mut settings = c.call("settings.get", json!({})).unwrap();
    settings["option_as_meta"] = json!("left");
    c.call("settings.set", json!({"settings": settings}))
        .unwrap();
    drop(c);
    assert!(plyd.stop().success());
    let _plyd = sb.start();
    let mut c = Control::connect(&sb.control_socket()).unwrap();
    let stored = c.call("settings.get", json!({})).unwrap();
    assert_eq!(
        stored["option_as_meta"], "left",
        "settings survive a restart"
    );
    let panes = c.call("pane.list", json!({"workspace_id": ws})).unwrap();
    assert_eq!(panes[0]["status"], "lost");
    let (_, first) = Data::attach(&sb.data_socket(), pane, 80, 24).unwrap();
    assert!(
        matches!(first, Frame::Snapshot(_)),
        "a lost pane can still be viewed"
    );
    let resumed = c.call("pane.resume", json!({"pane_id": pane})).unwrap();
    assert_eq!(
        resumed["status"], "idle",
        "the stored palette lets it spawn before any theme.set"
    );
    let mut d = sb.attach_ready(pane);
    d.input(b"echo back-$((40+2))\r").unwrap();
    assert!(d.pump_until(WAIT, |d| d.shows("back-42")).unwrap());
    let again = c.call("pane.resume", json!({"pane_id": pane})).unwrap_err();
    assert_eq!(again.code, ErrorCode::InvalidState);
}

#[test]
fn a_pane_gets_only_plyds_allow_listed_environment() {
    let sb = Sandbox::new("env");
    let _plyd = sb.start_with(&[("PLY_EXAMPLE_SECRET", "1")]);
    let (mut c, ws) = sb.control();
    let pane = sb.shell(&mut c, ws);
    let mut d = sb.attach_ready(pane);
    d.input(b"env > \"$HOME/env.txt\"\r").unwrap();
    let env = read_when_ready(&sb.home.join("env.txt"), WAIT).expect("env.txt");
    assert!(env.contains("TERM=xterm-256color"), "{env}");
    assert!(env.contains("COLORTERM=truecolor"), "{env}");
    assert!(
        env.contains(&format!("HOME={}", sb.home.display())),
        "{env}"
    );
    assert!(!env.contains("PLY_EXAMPLE_SECRET"), "env_clear: {env}");
    assert!(!env.contains("PLY_HOME"), "env_clear: {env}");
}

#[test]
fn handshakes_check_versions_and_panes() {
    let sb = Sandbox::new("hello");
    let _plyd = sb.start();
    let mut raw = std::os::unix::net::UnixStream::connect(sb.control_socket()).unwrap();
    use std::io::{BufRead, Write};
    raw.write_all(b"{\"t\":\"hello\",\"v\":2,\"client\":\"ply-app\",\"app_version\":\"9.0.0\"}\n")
        .unwrap();
    let mut line = String::new();
    let mut reader = std::io::BufReader::new(raw);
    reader.read_line(&mut line).unwrap();
    let res: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(res["id"], 0);
    assert_eq!(res["ok"], false);
    assert_eq!(res["err"]["code"], "version_mismatch");
    line.clear();
    assert_eq!(reader.read_line(&mut line).unwrap(), 0, "then plyd closes");

    let (_, refused) = Data::attach(&sb.data_socket(), 999, 80, 24).unwrap();
    assert!(matches!(refused, Frame::AttachRefused(r) if r.reason == RefuseReason::UnknownPane));
    let (_, refused) = Data::attach_v(&sb.data_socket(), 9, 1, 80, 24).unwrap();
    assert!(
        matches!(refused, Frame::AttachRefused(r) if r.reason == RefuseReason::VersionMismatch)
    );

    let (mut c, ws) = sb.control();
    let bad = c
        .call(
            "pane.create",
            json!({"workspace_id": ws, "cli": "shell", "cwd": "relative"}),
        )
        .unwrap_err();
    assert_eq!(bad.code, ErrorCode::BadRequest);
    let unknown = c.call("no.such", json!({})).unwrap_err();
    assert_eq!(unknown.code, ErrorCode::UnknownMethod);
}

#[test]
fn install_agent_dry_run_renders_the_plist_and_touches_nothing() {
    let sb = Sandbox::new("agent");
    let out = sb
        .plyd(&["install-agent", "--dry-run"])
        .env_remove("PLY_HOME")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let plist = sb.home.join("Library/LaunchAgents/dev.ply.app.plyd.plist");
    assert!(stdout.contains(&plist.display().to_string()), "{stdout}");
    assert!(
        stdout.contains("<string>dev.ply.app.plyd</string>"),
        "{stdout}"
    );
    assert!(stdout.contains("launchctl kickstart gui/"), "{stdout}");
    assert!(!plist.exists(), "a dry run writes nothing");

    let refused = sb.plyd(&["install-agent"]).output().unwrap();
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("PLY_HOME"));
    assert!(eventually(Duration::from_millis(10), || !plist.exists()));
}

#[test]
fn an_agent_pane_runs_the_cli_from_the_login_path_with_its_launch_spec() {
    let sb = Sandbox::new("agent");
    let bin = sb.home.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(
        sb.home.join(".profile"),
        "PATH=\"$HOME/bin:$PATH\"\nexport PATH\n",
    )
    .unwrap();
    let fake = bin.join("claude");
    std::fs::write(
        &fake,
        "#!/bin/sh\necho \"argv: $*\"\necho \"pane: $PLY_PANE_ID sync: $CLAUDE_CODE_FORCE_SYNC_OUTPUT\"\nexec sleep 60\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let _plyd = sb.start();
    let (mut c, ws) = sb.control();

    let missing = c
        .call(
            "pane.create",
            json!({"workspace_id": ws, "cli": "codex", "cwd": sb.home}),
        )
        .unwrap_err();
    assert_eq!(missing.code, ErrorCode::CliNotFound);
    let worktree = c
        .call(
            "pane.create",
            json!({"workspace_id": ws, "cli": "shell", "cwd": sb.home, "worktree": {"name": "x"}}),
        )
        .unwrap_err();
    assert_eq!(worktree.code, ErrorCode::BadRequest);

    let pane = c
        .call(
            "pane.create",
            json!({"workspace_id": ws, "cli": "claude", "cwd": sb.home, "prompt": "hello there"}),
        )
        .unwrap();
    assert_eq!(pane["status"], "starting");
    assert_eq!(pane["title"], "claude");
    let id = pane["id"].as_u64().unwrap();
    let dir = sb.ply_home.join("run/panes").join(id.to_string());
    let settings = std::fs::read_to_string(dir.join("claude-settings.json")).unwrap();
    assert!(settings.contains("\"hooks\""), "{settings}");
    let launch: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("launch.json")).unwrap()).unwrap();
    assert_eq!(launch["cli"], "claude");
    assert_eq!(launch["argv"][0], fake.display().to_string());

    let (mut d, first) = Data::attach(&sb.data_socket(), id, 120, 24).unwrap();
    d.apply(&first, true).unwrap();
    assert!(
        d.pump_until(WAIT, |d| d.shows(&format!("pane: {id} sync: 1")))
            .unwrap(),
        "{:?}",
        d.screen()
    );
    assert!(d.shows("--settings"), "{:?}", d.screen());
    assert!(d.shows("hello there"), "{:?}", d.screen());

    let not_waiting = c
        .call("pane.answer", json!({"pane_id": id, "choice": 1}))
        .unwrap_err();
    assert_eq!(
        not_waiting.code,
        ErrorCode::InvalidState,
        "no dialog is shown"
    );
    c.call("pane.close", json!({"pane_id": id, "kill": true}))
        .unwrap();
    assert!(
        c.wait_event(
            WAIT,
            |e| matches!(e, Event::PaneRemoved(r) if r.pane_id == id)
        )
        .is_some()
    );
}

fn close_and_wait_exit(c: &mut Control, pane: u64) -> (i32, Duration) {
    let closed = Instant::now();
    c.call("pane.close", json!({"pane_id": pane, "kill": true}))
        .unwrap();
    let Some(Event::PaneExit(exit)) = c.wait_event(
        WAIT,
        |e| matches!(e, Event::PaneExit(x) if x.pane_id == pane),
    ) else {
        panic!("no pane.exit");
    };
    (exit.code, closed.elapsed())
}

fn alive(pid: i32) -> bool {
    rustix::process::Pid::from_raw(pid)
        .is_some_and(|p| rustix::process::test_kill_process(p).is_ok())
}

#[test]
fn a_shell_that_ignores_sighup_is_killed_after_the_grace() {
    let sb = Sandbox::new("hupsh");
    let _plyd = sb.start();
    let (mut c, ws) = sb.control();
    let pane = sb.shell(&mut c, ws);
    let mut d = sb.attach_ready(pane);
    d.input(b"trap '' HUP; echo trapped\r").unwrap();
    assert!(d.pump_until(WAIT, |d| d.shows("trapped")).unwrap());
    let (code, took) = close_and_wait_exit(&mut c, pane);
    assert_eq!(code, 128 + 9, "SIGKILL");
    assert!(
        took >= Duration::from_millis(1900),
        "after the 2 s grace: {took:?}"
    );
}

#[test]
fn a_group_member_that_ignores_sighup_is_killed_after_its_leader_exits() {
    let sb = Sandbox::new("hupbg");
    let _plyd = sb.start();
    let (mut c, ws) = sb.control();
    let pane = sb.shell(&mut c, ws);
    let mut d = sb.attach_ready(pane);
    d.input(b"set +m; (trap '' HUP; exec sleep 60) & echo $! > \"$HOME/bg.pid\"\r")
        .unwrap();
    let pid: i32 = read_when_ready(&sb.home.join("bg.pid"), WAIT)
        .expect("bg.pid")
        .trim()
        .parse()
        .unwrap();
    assert!(alive(pid));
    let (code, _) = close_and_wait_exit(&mut c, pane);
    assert_eq!(code, 128 + 1, "the shell itself dies of SIGHUP");
    assert!(
        c.wait_event(
            WAIT,
            |e| matches!(e, Event::PaneRemoved(r) if r.pane_id == pane)
        )
        .is_some()
    );
    assert!(
        eventually(Duration::from_secs(6), || !alive(pid)),
        "the SIGHUP-ignoring member got SIGKILL"
    );
}

#[test]
fn grids_too_large_for_one_frame_are_refused_and_never_recorded() {
    let sb = Sandbox::new("grid");
    let _plyd = sb.start();
    let (mut c, ws) = sb.control();
    let pane = sb.shell(&mut c, ws);
    let (_, refused) = Data::attach(&sb.data_socket(), pane, u16::MAX, u16::MAX).unwrap();
    assert!(
        matches!(&refused, Frame::AttachRefused(r) if r.reason == RefuseReason::NotAttached && r.message.contains("too large")),
        "{refused:?}"
    );
    let mut d = sb.attach_ready(pane);
    d.send(&Frame::Resize(ply_proto::data::Resize {
        cols: 4000,
        rows: 4000,
        cell_width_px: 8,
        cell_height_px: 16,
    }))
    .unwrap();
    let mut size = None;
    let deadline = Instant::now() + WAIT;
    while size.is_none() && Instant::now() < deadline {
        if let Some(Frame::Snapshot(s)) = d.recv(Duration::from_millis(200)).unwrap() {
            size = Some((s.cols, s.rows));
        }
    }
    assert_eq!(size, Some((80, 24)), "the RESIZE was ignored");
    let other = sb.shell(&mut c, ws);
    let mut o = sb.attach_ready(other);
    o.input(b"stty size\r").unwrap();
    assert!(
        o.pump_until(WAIT, |d| d.shows("24 80")).unwrap(),
        "{:?}",
        o.screen()
    );
}

#[test]
fn a_database_from_a_newer_plyd_is_refused_with_status_0() {
    let sb = Sandbox::new("schema");
    std::fs::create_dir_all(&sb.ply_home).unwrap();
    let db = rusqlite::Connection::open(sb.ply_home.join("ply.db")).unwrap();
    db.execute_batch("CREATE TABLE schema_version (version INTEGER NOT NULL); INSERT INTO schema_version VALUES (99);")
        .unwrap();
    drop(db);
    let out = sb.plyd(&["--foreground"]).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "status 0 keeps launchd from restarting it: {stderr}"
    );
    assert!(stderr.contains("schema version 99"), "{stderr}");
}
