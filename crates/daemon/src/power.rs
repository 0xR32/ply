//! Keeping the Mac awake while an agent works (spec 11.3, `keep_awake_while_running`).
//!
//! While at least one pane is `running` and the setting is on, plyd holds a "prevent idle system sleep" assertion,
//! released as soon as none is. The assertion comes from a `caffeinate -i -w <plyd pid>` child rather than from
//! IOKit's `IOPMAssertionCreateWithName` called directly: `caffeinate -i` creates exactly that assertion
//! (`PreventUserIdleSystemSleep`), `-w` ties it to plyd's lifetime so even a plyd crash cannot leave it behind, and
//! plyd keeps its single audited `unsafe` block in `pty.rs` instead of adding FFI to IOKit and CoreFoundation (no
//! crate for them meets INV-16). The cost is one small process while an agent runs; the assertion is attributed to
//! caffeinate in `pmset -g assertions`. Closing the lid still sleeps the Mac. A sandboxed plyd (`PLY_HOME`) is
//! built without a command, so tests never hold an assertion.

use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

/// The command that holds the assertion for the current process: `caffeinate -i -w <pid>`.
pub fn caffeinate_command() -> Vec<String> {
    vec![
        "/usr/bin/caffeinate".to_owned(),
        "-i".to_owned(),
        "-w".to_owned(),
        std::process::id().to_string(),
    ]
}

/// Holds or releases the assertion; `Sync`, and every call is quick (a spawn or a kill).
#[derive(Debug)]
pub struct KeepAwake {
    command: Option<Vec<String>>,
    child: Mutex<Option<Child>>,
}

impl KeepAwake {
    /// A holder that runs `command` while the assertion is wanted; `None` makes every call a logged no-op.
    pub fn new(command: Option<Vec<String>>) -> Self {
        Self {
            command,
            child: Mutex::new(None),
        }
    }

    /// Holds the assertion when `wanted`, releases it otherwise; idempotent, and restarts a holder that died.
    pub fn set(&self, wanted: bool) {
        self.set_with(|| wanted);
    }

    /// [`KeepAwake::set`] with the wish evaluated while this holder's lock is held, so concurrent callers apply in order.
    pub fn set_with(&self, wanted: impl FnOnce() -> bool) {
        let mut slot = match self.child.lock() {
            Ok(slot) => slot,
            Err(poisoned) => {
                tracing::error!("keep-awake state was poisoned; recovering");
                poisoned.into_inner()
            }
        };
        if let Some(child) = slot.as_mut() {
            match child.try_wait() {
                Ok(None) => {}
                Ok(Some(status)) => {
                    tracing::warn!(%status, "the keep-awake process ended by itself");
                    *slot = None;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "cannot check the keep-awake process");
                    *slot = None;
                }
            }
        }
        match (wanted(), slot.is_some()) {
            (true, false) => *slot = self.start(),
            (false, true) => {
                if let Some(child) = slot.take() {
                    stop(child);
                }
            }
            _ => {}
        }
    }

    /// Whether a holder process is running now.
    pub fn is_held(&self) -> bool {
        self.child
            .lock()
            .map(|slot| slot.is_some())
            .unwrap_or(false)
    }

    fn start(&self) -> Option<Child> {
        let Some((program, args)) = self.command.as_ref().and_then(|c| c.split_first()) else {
            tracing::debug!("keep-awake is disabled for this plyd");
            return None;
        };
        match Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => {
                tracing::info!(pid = child.id(), "holding the prevent-idle-sleep assertion");
                Some(child)
            }
            Err(e) => {
                tracing::warn!(program = %program, error = %e, "cannot start the keep-awake process");
                None
            }
        }
    }
}

fn stop(mut child: Child) {
    if let Err(e) = child.kill() {
        tracing::warn!(pid = child.id(), error = %e, "cannot stop the keep-awake process");
    }
    match child.wait() {
        Ok(_) => tracing::info!("released the prevent-idle-sleep assertion"),
        Err(e) => tracing::warn!(error = %e, "cannot reap the keep-awake process"),
    }
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        self.set(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_holder_runs_only_while_wanted() {
        let k = KeepAwake::new(Some(vec!["/bin/sleep".to_owned(), "30".to_owned()]));
        assert!(!k.is_held());
        k.set(true);
        assert!(k.is_held());
        k.set(true);
        assert!(k.is_held());
        k.set(false);
        assert!(!k.is_held());
    }

    #[test]
    fn without_a_command_nothing_is_held() {
        let k = KeepAwake::new(None);
        k.set(true);
        assert!(!k.is_held());
    }

    #[test]
    fn the_command_names_this_process() {
        let c = caffeinate_command();
        assert_eq!(c[..3], ["/usr/bin/caffeinate", "-i", "-w"]);
        assert_eq!(c[3], std::process::id().to_string());
    }
}
