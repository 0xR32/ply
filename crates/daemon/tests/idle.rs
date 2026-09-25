//! plyd without clients and without output (spec 11.3, R-R21): no idle exit, panes keep running while nobody is
//! connected, and an idle pane sends nothing.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::{Sandbox, read_when_ready};
use serde_json::json;

#[test]
fn with_no_client_plyd_and_its_panes_keep_running() {
    let sb = Sandbox::new("alone");
    let mut plyd = sb.start();
    let (mut c, ws) = sb.control();
    let pane = sb.shell(&mut c, ws);
    let mut d = sb.attach_ready(pane);
    d.input(b"sleep 2; echo still-here > \"$HOME/alive.txt\"\r")
        .unwrap();
    drop(d);
    drop(c);

    std::thread::sleep(Duration::from_secs(3));
    assert!(plyd.is_running(), "plyd has no idle exit");
    assert_eq!(
        read_when_ready(&sb.home.join("alive.txt"), Duration::from_secs(5)).as_deref(),
        Some("still-here\n"),
        "the shell ran on while no client was connected"
    );
    let (mut c, _) = sb.control();
    let panes = c.call("pane.list", json!({"workspace_id": ws})).unwrap();
    assert_eq!(panes[0]["id"], pane);
    let mut d = sb.attach_ready(pane);
    d.input(b"echo reconnected\r").unwrap();
    assert!(
        d.pump_until(Duration::from_secs(10), |d| d.shows("reconnected"))
            .unwrap()
    );
}

#[test]
fn an_idle_pane_sends_no_frames() {
    let sb = Sandbox::new("quiet");
    let _plyd = sb.start();
    let (mut c, ws) = sb.control();
    let pane = sb.shell(&mut c, ws);
    let mut d = sb.attach_ready(pane);
    d.settle(Duration::from_millis(300)).unwrap();
    assert!(
        d.recv(Duration::from_millis(1500)).unwrap().is_none(),
        "no Delta without dirty rows"
    );
}
