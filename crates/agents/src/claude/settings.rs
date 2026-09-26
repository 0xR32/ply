//! The per-pane Claude Code `--settings` file: `hooks`, the `statusLine` that reports plan usage, and `"theme":"dark-ansi"` when enabled (spec 6.1).
//!
//! The status line is `ply-hook statusline`, which forwards Claude Code's status line payload to plyd (its
//! `rate_limits` are the live plan usage) and then runs the user's own status line command, if they set one,
//! with the same input, printing what it prints. The user's other `statusLine` keys (`padding`, `refreshInterval`) are
//! kept, so the pane shows the status line it would show without ply.

use std::path::Path;

use serde_json::{Map, Value, json};

use crate::adapter::utf8;
use crate::error::{Result, json as json_err};

/// File name of the generated settings inside `run/panes/<id>/` (Ruling R6).
pub const SETTINGS_FILE: &str = "claude-settings.json";

/// The theme key's value when "Use ply colours in Claude Code" is on.
pub const THEME: &str = "dark-ansi";

/// The envelope event `ply-hook statusline` sends; plyd takes it for plan usage, never as a hook (R17 ignores it).
pub const STATUS_LINE_EVENT: &str = "StatusLine";

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

/// Renders the settings JSON around the user's own `statusLine` (`user_status_line`); `hook_program` must be UTF-8 ([`crate::Error::NonUtf8Path`]) and is shell-quoted.
pub fn claude_settings_json(
    hook_program: &Path,
    use_ply_colours: bool,
    user_status_line: Option<&Value>,
) -> Result<String> {
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
    settings.insert(
        "statusLine".to_owned(),
        status_line(&program, user_status_line),
    );
    if use_ply_colours {
        settings.insert("theme".to_owned(), Value::String(THEME.to_owned()));
    }
    let mut out = serde_json::to_string_pretty(&Value::Object(settings))
        .map_err(json_err("Claude settings"))?;
    out.push('\n');
    Ok(out)
}

/// `ply-hook statusline`, followed by the user's command when their `statusLine` is a command one, with their other keys.
fn status_line(program: &str, user: Option<&Value>) -> Value {
    let mut entry = Map::new();
    let mut command = format!("{program} statusline");
    let user = user
        .and_then(Value::as_object)
        .filter(|u| u.get("type").and_then(Value::as_str) == Some("command"));
    if let Some(user) = user {
        for (key, value) in user {
            if key != "command" {
                entry.insert(key.clone(), value.clone());
            }
        }
        if let Some(own) = user
            .get("command")
            .and_then(Value::as_str)
            .filter(|c| !c.trim().is_empty())
        {
            command.push(' ');
            command.push_str(&shell_quote(own));
        }
    }
    entry.insert("type".to_owned(), Value::String("command".to_owned()));
    entry.insert("command".to_owned(), Value::String(command));
    Value::Object(entry)
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
