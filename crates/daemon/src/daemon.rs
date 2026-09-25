//! plyd's lifetime: open the store, restore the panes, serve C1 and C2, and stop only when asked.
//!
//! [`run`] expects the caller to hold the [`crate::lock::InstanceLock`]; it prepares the run directory, opens the
//! database (refusing a newer schema), loads `config.toml`, resolves the login shell, gives every stored open pane a
//! task, replaces stale socket files and serves until `daemon.shutdown` or SIGTERM, SIGINT or SIGHUP. There is no
//! idle exit (spec 11.3): with no client connected plyd keeps every pane running. On the way out it broadcasts
//! `daemon.stopping`, stops the panes' processes when asked to (`kill_panes`), removes its sockets and returns; the
//! processes of panes it did not stop lose their pty with plyd and come back as `lost` after the next start.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ply_proto::control::{DaemonStopping, Event};
use ply_proto::pane::{PaneId, PaneStatus, UnixSeconds};
use ply_term::Palette;
use tokio::net::UnixListener;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::{broadcast, watch};

use crate::config::Config;
use crate::db::Db;
use crate::error::{Result, io};
use crate::login::LoginEnv;
use crate::panes::launch;
use crate::panes::pane::PaneCmd;
use crate::panes::registry::Registry;
use crate::paths::Paths;
use crate::power::KeepAwake;
use crate::pty::Geometry;
use crate::server::{control, data};

/// C1 events buffered per connected client before a slow one is dropped.
pub const EVENT_CAPACITY: usize = 1024;

/// The view size a pane starts with before any client attached: 80 × 24 cells of 8 × 16 pixels.
pub const DEFAULT_GEOMETRY: Geometry = Geometry {
    cols: 80,
    rows: 24,
    cell_width_px: 8,
    cell_height_px: 16,
};

/// Longest `daemon.shutdown {kill_panes:true}` waits for the processes to exit (SIGKILL follows SIGHUP after 2 s).
pub const KILL_WAIT: Duration = Duration::from_secs(4);

/// How [`run`] is set up; `main.rs` fills it from the command line and the environment.
#[derive(Debug, Clone)]
pub struct Options {
    /// Where everything lives.
    pub paths: Paths,
    /// The `ply-hook` binary agent panes run their hooks with.
    pub hook_program: PathBuf,
    /// The keep-awake command (see [`crate::power`]); `None` disables the power assertion.
    pub keep_awake: Option<Vec<String>>,
}

/// State shared by the servers and the pane tasks.
#[derive(Debug)]
pub struct Shared {
    /// Resolved paths.
    pub paths: Paths,
    /// Login shell and base environment of children.
    pub login: LoginEnv,
    /// `ply-hook`, passed to agent launch specs.
    pub hook_program: PathBuf,
    /// C1 event broadcast.
    pub events: broadcast::Sender<Event>,
    /// The terminal palette; `None` until the first `theme.set` (or a stored one).
    pub palette: watch::Sender<Option<Palette>>,
    registry: Mutex<Registry>,
    geometry: Mutex<Geometry>,
    power: KeepAwake,
    shutdown: watch::Sender<Option<bool>>,
    next_client: AtomicU64,
}

impl Shared {
    /// Locks the registry; hold the guard only for in-memory work and short SQLite writes, never across an await.
    pub fn registry(&self) -> MutexGuard<'_, Registry> {
        match self.registry.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::error!("the registry lock was poisoned; continuing with its state");
                poisoned.into_inner()
            }
        }
    }

    /// The size of the last ATTACH or RESIZE, used for new panes.
    pub fn geometry(&self) -> Geometry {
        match self.geometry.lock() {
            Ok(g) => *g,
            Err(poisoned) => *poisoned.into_inner(),
        }
    }

    /// Records the size of an ATTACH or RESIZE.
    pub fn set_geometry(&self, geometry: Geometry) {
        match self.geometry.lock() {
            Ok(mut g) => *g = geometry,
            Err(poisoned) => *poisoned.into_inner() = geometry,
        }
    }

    /// A fresh id for a C2 connection.
    pub fn next_client_id(&self) -> u64 {
        self.next_client.fetch_add(1, Ordering::Relaxed)
    }

    /// Holds the power assertion while any pane is `running` and the setting is on.
    pub fn update_power(&self) {
        let (running, enabled) = {
            let reg = self.registry();
            (reg.running_count(), reg.settings().keep_awake_while_running)
        };
        self.power.set(enabled && running > 0);
    }

    /// Changes a pane's state (`pane.status`), then re-evaluates the power assertion; the state machine's entry point.
    pub fn set_status(&self, pane_id: PaneId, status: PaneStatus, detail: Option<String>) {
        self.registry()
            .set_status(pane_id, status, detail, unix_now());
        self.update_power();
    }

    /// Asks plyd to stop; `kill_panes` also stops every pane's process. The first request wins.
    pub fn request_shutdown(&self, kill_panes: bool) {
        self.shutdown.send_if_modified(|state| {
            if state.is_some() {
                return false;
            }
            *state = Some(kill_panes);
            true
        });
    }

    /// Whether a shutdown was requested; new work is refused from then on.
    pub fn is_stopping(&self) -> bool {
        self.shutdown.borrow().is_some()
    }

    /// Resolves once a shutdown was requested.
    pub fn shutdown_signal(&self) -> watch::Receiver<Option<bool>> {
        self.shutdown.subscribe()
    }
}

/// Unix time now in whole seconds.
pub fn unix_now() -> UnixSeconds {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Runs plyd until it is asked to stop (module docs); the caller holds the instance lock.
/// Fails with [`crate::Error`] when the run directory, database or sockets cannot be set up.
pub async fn run(options: Options) -> Result<()> {
    let paths = options.paths;
    paths.prepare()?;
    let db = Db::open(&paths.database())?;
    let config = Config::load(&paths.config());
    let palette = config.palette.as_ref().map(Palette::from_theme);
    let login = LoginEnv::resolve().await;
    tracing::info!(shell = %login.shell.display(), path = %login.path(), "login environment resolved");
    let home = login
        .base
        .get("HOME")
        .cloned()
        .unwrap_or_else(|| "/".to_owned());
    let (events, _) = broadcast::channel(EVENT_CAPACITY);
    let registry = Registry::load(
        db,
        config,
        &paths.config(),
        events.clone(),
        &home,
        unix_now(),
    )?;
    let (shutdown, _) = watch::channel(None);
    let shared = Arc::new(Shared {
        paths,
        login,
        hook_program: options.hook_program,
        events,
        palette: watch::Sender::new(palette),
        registry: Mutex::new(registry),
        geometry: Mutex::new(DEFAULT_GEOMETRY),
        power: KeepAwake::new(options.keep_awake),
        shutdown,
        next_client: AtomicU64::new(1),
    });
    launch::restore(&shared);

    let control_listener = bind(&shared.paths.control_socket())?;
    let data_listener = bind(&shared.paths.data_socket())?;
    tracing::info!(
        control = %shared.paths.control_socket().display(),
        data = %shared.paths.data_socket().display(),
        "plyd is serving"
    );
    let control_task = tokio::spawn(control::serve(control_listener, Arc::clone(&shared)));
    let data_task = tokio::spawn(data::serve(data_listener, Arc::clone(&shared)));

    let kill_panes = wait_for_stop(&shared).await?;
    tracing::info!(kill_panes, "plyd is stopping");
    shared.request_shutdown(kill_panes);
    if shared
        .events
        .send(Event::DaemonStopping(DaemonStopping { kill_panes }))
        .is_err()
    {
        tracing::debug!("no C1 client to tell about the stop");
    }
    if kill_panes {
        stop_processes(&shared).await;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    control_task.abort();
    data_task.abort();
    let handles = shared.registry().handles();
    for (_, handle) in handles {
        if handle.send(PaneCmd::Stop).await.is_err() {
            tracing::debug!("a pane task had already stopped");
        }
    }
    shared.power.set(false);
    for sock in [shared.paths.control_socket(), shared.paths.data_socket()] {
        if let Err(e) = std::fs::remove_file(&sock) {
            tracing::warn!(socket = %sock.display(), error = %e, "cannot remove the socket");
        }
    }
    tracing::info!("plyd stopped");
    Ok(())
}

fn bind(path: &std::path::Path) -> Result<UnixListener> {
    match std::fs::remove_file(path) {
        Ok(()) => tracing::debug!(socket = %path.display(), "removed a stale socket"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(io("cannot replace", path)(e)),
    }
    UnixListener::bind(path).map_err(io("cannot bind", path))
}

async fn wait_for_stop(shared: &Shared) -> Result<bool> {
    let signal_error = |e| io("cannot install a handler for", "signals")(e);
    let mut term = signal(SignalKind::terminate()).map_err(signal_error)?;
    let mut int = signal(SignalKind::interrupt()).map_err(signal_error)?;
    let mut hup = signal(SignalKind::hangup()).map_err(signal_error)?;
    let mut requested = shared.shutdown_signal();
    let kill_panes = tokio::select! {
        _ = term.recv() => { tracing::info!("SIGTERM"); false }
        _ = int.recv() => { tracing::info!("SIGINT"); false }
        _ = hup.recv() => { tracing::info!("SIGHUP"); false }
        changed = requested.wait_for(Option::is_some) => match changed {
            Ok(state) => state.unwrap_or(false),
            Err(e) => {
                tracing::error!(error = %e, "the shutdown channel closed");
                false
            }
        },
    };
    Ok(kill_panes)
}

async fn stop_processes(shared: &Shared) {
    let live = {
        let reg = shared.registry();
        reg.live_panes()
            .into_iter()
            .filter_map(|id| reg.entry(id).and_then(|e| e.handle.clone()))
            .collect::<Vec<_>>()
    };
    for handle in live {
        if handle.send(PaneCmd::Kill).await.is_err() {
            tracing::debug!("a pane task had already stopped");
        }
    }
    let deadline = Instant::now() + KILL_WAIT;
    while Instant::now() < deadline && !shared.registry().live_panes().is_empty() {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let left = shared.registry().live_panes();
    if !left.is_empty() {
        tracing::warn!(panes = ?left, "some panes' processes did not exit in time");
    }
}
