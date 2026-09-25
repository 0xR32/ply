//! The per-pane Claude Code `--settings` file: only `hooks`, plus `"theme":"dark-ansi"` when enabled (spec 6.1).

use std::path::Path;

use serde_json::{Map, Value, json};

use crate::adapter::utf8;
use crate::error::{Result, json as json_err};

/// File name of the generated settings inside `run/panes/<id>/` (Ruling R6).
pub const SETTINGS_FILE: &str = "claude-settings.json";

/// The theme key's value when "Use ply colours in Claude Code" is on.
pub const THEME: &str = "dark-ansi";

/// Every hook ply registers, each as `ply-hook claude <Event>`; never WorktreeCreate/WorktreeRemove (R15, INV-7).
pub const HOOK_EVENTS: [&str; 12] = [
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "PermissionDenied",
    "Notification",
    "Stop",
    "StopFailure",
    "SessionEnd",
    "CwdChanged",
];

/// Renders the settings JSON; `hook_program` must be UTF-8 ([`crate::Error::NonUtf8Path`]) and is shell-quoted.
pub fn claude_settings_json(hook_program: &Path, use_ply_colours: bool) -> Result<String> {
    let program = shell_quote(&utf8(hook_program)?);
    let hooks: Map<String, Value> = HOOK_EVENTS
        .iter()
        .map(|event| {
            let command = format!("{program} claude {event}");
            let entry = json!([{"hooks": [{"type": "command", "command": command}]}]);
            ((*event).to_owned(), entry)
        })
        .collect();
    let mut settings = Map::new();
    settings.insert("hooks".to_owned(), Value::Object(hooks));
    if use_ply_colours {
        settings.insert("theme".to_owned(), Value::String(THEME.to_owned()));
    }
    let mut out = serde_json::to_string_pretty(&Value::Object(settings))
        .map_err(json_err("Claude settings"))?;
    out.push('\n');
    Ok(out)
}

/// Quotes `s` for POSIX `sh`, which Claude Code runs hook commands with.
pub(crate) fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_spaces_and_single_quotes() {
        assert_eq!(
            shell_quote("/Applications/Ply.app/ply-hook"),
            "'/Applications/Ply.app/ply-hook'"
        );
        assert_eq!(shell_quote("/a b/it's"), r"'/a b/it'\''s'");
    }
}
