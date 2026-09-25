//! `ply-cli`, a command-line test client for plyd (WP4 exit criterion): it drives a shell through C1 and C2,
//! detaches, reattaches and checks that it sees the same screen.
//!
//! ```text
//! cargo run -p ply-daemon --example ply-cli -- demo          # shell pane, echo, detach, reattach, compare, close
//! cargo run -p ply-daemon --example ply-cli -- list          # panes of every workspace
//! cargo run -p ply-daemon --example ply-cli -- screen 3      # attach to pane 3 at 80 x 24 and print its screen
//! ```
//!
//! It talks to the plyd of `PLY_HOME` (else the installed one). Attaching resizes the pane to the client's size, as
//! the app's terminal view does. The demo sends a neutral palette only when plyd has none yet.

#![forbid(unsafe_code)]

#[path = "../tests/common/client.rs"]
mod client;

use std::error::Error;
use std::process::ExitCode;
use std::time::Duration;

use client::{Control, Data};
use ply_daemon::paths::Paths;
use serde_json::{Value, json};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const WAIT: Duration = Duration::from_secs(10);

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let run = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["demo"] => demo(),
        ["list"] => list(),
        ["screen", id] => match id.parse() {
            Ok(id) => screen(id),
            Err(e) => Err(format!("pane id {id:?}: {e}").into()),
        },
        _ => Err("usage: ply-cli demo | list | screen <pane_id>".into()),
    };
    match run {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("ply-cli: {e}");
            ExitCode::FAILURE
        }
    }
}

fn paths() -> Result<Paths> {
    Ok(Paths::from_env(None)?)
}

fn call(c: &mut Control, method: &str, params: Value) -> Result<Value> {
    c.call(method, params)
        .map_err(|e| format!("{method}: {:?}: {}", e.code, e.msg).into())
}

fn example_palette() -> Value {
    let ansi: Vec<String> = (0..16)
        .map(|i| format!("#{0:02X}{0:02X}{0:02X}", i * 16))
        .collect();
    json!({
        "ansi": ansi, "fg": "#E0E0E0", "bg": "#101010", "cursor": "#E0E0E0",
        "cursorText": "#101010", "selectionBg": "#404040", "selectionFg": "#E0E0E0"
    })
}

fn demo() -> Result<bool> {
    let paths = paths()?;
    let mut c = Control::connect(&paths.control_socket())?;
    let workspaces = call(&mut c, "workspace.list", json!({}))?;
    let ws = &workspaces[0];
    let params = json!({"workspace_id": ws["id"], "cli": "shell", "cwd": ws["path"]});
    let pane = match c.call("pane.create", params.clone()) {
        Ok(pane) => pane,
        Err(e) if e.code == ply_proto::control::ErrorCode::InvalidState => {
            println!("plyd has no palette yet; sending a neutral one");
            call(&mut c, "theme.set", json!({"palette": example_palette()}))?;
            call(&mut c, "pane.create", params)?
        }
        Err(e) => return Err(format!("pane.create: {:?}: {}", e.code, e.msg).into()),
    };
    let id = pane["id"].as_u64().ok_or("pane.create returned no id")?;
    println!("created shell pane {id}");

    let (mut d, first) = Data::attach(&paths.data_socket(), id, 80, 24)?;
    d.apply(&first, true)?;
    d.input(b"echo ply-cli $((6*7))\r")?;
    if !d.pump_until(WAIT, |d| d.shows("ply-cli 42"))? {
        return Err("the shell did not answer".into());
    }
    d.settle(Duration::from_millis(300))?;
    let before = d.screen();
    print_screen("attached", &before);
    drop(d);
    println!("detached");

    let (mut again, first) = Data::attach(&paths.data_socket(), id, 80, 24)?;
    again.apply(&first, true)?;
    let after = again.screen();
    print_screen("reattached", &after);
    let same = before == after;
    println!(
        "same screen after reattach: {}",
        if same { "yes" } else { "NO" }
    );

    call(&mut c, "pane.close", json!({"pane_id": id, "kill": true}))?;
    println!("closed pane {id}");
    Ok(same)
}

fn list() -> Result<bool> {
    let mut c = Control::connect(&paths()?.control_socket())?;
    let workspaces = call(&mut c, "workspace.list", json!({}))?;
    for ws in workspaces.as_array().into_iter().flatten() {
        println!("workspace {} {}", ws["id"], ws["path"]);
        let panes = call(&mut c, "pane.list", json!({"workspace_id": ws["id"]}))?;
        for p in panes.as_array().into_iter().flatten() {
            println!(
                "  pane {:>3}  tab {:>3}  {:<6} {:<18} {}  {}",
                p["id"],
                p["tab_id"],
                p["cli"].as_str().unwrap_or("?"),
                p["status"].as_str().unwrap_or("?"),
                p["title"],
                p["cwd"]
            );
        }
    }
    Ok(true)
}

fn screen(id: u64) -> Result<bool> {
    let (mut d, first) = Data::attach(&paths()?.data_socket(), id, 80, 24)?;
    d.apply(&first, true)?;
    d.settle(Duration::from_millis(300))?;
    print_screen(&format!("pane {id}"), &d.screen());
    Ok(true)
}

fn print_screen(label: &str, rows: &[String]) {
    println!("--- {label} ---");
    let last = rows
        .iter()
        .rposition(|r| !r.trim().is_empty())
        .map_or(0, |i| i + 1);
    for row in &rows[..last] {
        println!("|{row}");
    }
}
