//! Skills and commands the CLIs offer (Ruling R61), read from text plyd hands in: a skill's `SKILL.md`, a command or
//! custom prompt file, Claude Code's `settings.json` and its plugin registry `plugins/installed_plugins.json`.
//!
//! [`parse_front_matter`] reads the few keys ply shows from a file's `---` block: `name`, `description`,
//! `argument-hint` and `user-invocable`, as plain, quoted, folded or literal YAML scalars; nested maps, lists and
//! every other key are skipped, and a file without a closed block has no front matter. [`skill_of`] turns one file into
//! the [`Skill`] the app lists, with the invocation the user types: `/name` or `/<plugin>:name` for Claude Code,
//! `$name` for a Codex skill, `/prompts:name` for a Codex custom prompt. A skill whose front matter says
//! `user-invocable: false`, or whose name cannot be typed as one word, is left out; descriptions and hints are cut to a
//! length the pop-over shows. [`enabled_plugins`] and [`plugin_roots`] find the enabled plugins' install folders.
//!
//! The crate reads no directory itself: plyd walks the folders (`crates/daemon/src/skills.rs`) and never writes any of
//! these files (INV-8).

use std::collections::BTreeMap;
use std::path::PathBuf;

use ply_proto::pane::{AgentCli, Skill, SkillSource};
use serde_json::Value;

use crate::error::{Result, json};

/// The namespace Claude Code gives skills synced from the user's claude.ai account (`~/.claude/skills/synced/`).
pub const SYNCED_PLUGIN: &str = "anthropic-skills";

/// Longest description kept, in characters; a longer one ends in `…`.
pub const MAX_DESCRIPTION_CHARS: usize = 400;

/// Longest argument hint kept, in characters.
pub const MAX_HINT_CHARS: usize = 80;

/// Longest skill or plugin name accepted, in bytes.
pub const MAX_NAME_BYTES: usize = 128;

/// The keys of a file's front matter that ply shows; every other key is ignored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FrontMatter {
    /// `name`.
    pub name: Option<String>,
    /// `description`.
    pub description: Option<String>,
    /// `argument-hint`.
    pub argument_hint: Option<String>,
    /// `user-invocable`; `Some(false)` hides the skill from the user's slash menu.
    pub user_invocable: Option<bool>,
}

/// What kind of file a skill was read from, with the file-system name its invocation falls back to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillFile<'a> {
    /// A `<dir_name>/SKILL.md`; the front matter's `name` wins over the folder's.
    Skill {
        /// The folder holding `SKILL.md`.
        dir_name: &'a str,
    },
    /// A Claude Code command, `<stem>.md`, named after its file.
    Command {
        /// The file name without `.md`.
        stem: &'a str,
    },
    /// A Codex custom prompt, `<stem>.md`, run as `/prompts:<stem>`.
    Prompt {
        /// The file name without `.md`.
        stem: &'a str,
    },
}

/// An enabled Claude Code plugin's install folder, whose `skills/` and `commands/` hold its skills.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginRoot {
    /// The plugin's name, the part of its registry key before `@`.
    pub name: String,
    /// The absolute `installPath` the registry records.
    pub path: PathBuf,
}

/// The front matter of `text`; a text that does not open with `---` or never closes the block has none.
pub fn parse_front_matter(text: &str) -> FrontMatter {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return FrontMatter::default();
    }
    let mut block = Vec::new();
    let mut closed = false;
    for line in lines {
        let end = line.trim_end();
        if end == "---" || end == "..." {
            closed = true;
            break;
        }
        block.push(line.trim_end_matches('\r'));
    }
    if !closed {
        return FrontMatter::default();
    }
    let mut meta = FrontMatter::default();
    let mut i = 0;
    while i < block.len() {
        let line = block[i];
        i += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || line.starts_with([' ', '\t']) {
            continue;
        }
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let mut more = Vec::new();
        while i < block.len() && (block[i].starts_with([' ', '\t']) || block[i].trim().is_empty()) {
            more.push(block[i]);
            i += 1;
        }
        let value = scalar(rest.trim(), &more);
        match key.trim() {
            "name" => meta.name = value,
            "description" => meta.description = value,
            "argument-hint" => meta.argument_hint = value,
            "user-invocable" => {
                meta.user_invocable = value.and_then(|v| match v.to_ascii_lowercase().as_str() {
                    "true" => Some(true),
                    "false" => Some(false),
                    _ => None,
                });
            }
            _ => {}
        }
    }
    meta
}

/// One scalar value: `rest` after the key's colon and the indented lines below it; `None` for a map, list or empty value.
fn scalar(rest: &str, more: &[&str]) -> Option<String> {
    let body: Vec<&str> = more.iter().map(|l| l.trim()).collect();
    let value = if let Some(style) = rest.chars().next().filter(|c| *c == '>' || *c == '|') {
        let sep = if style == '>' { " " } else { "\n" };
        body.iter()
            .filter(|l| !l.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(sep)
    } else if rest.starts_with('"') || rest.starts_with('\'') {
        let joined = std::iter::once(rest)
            .chain(body.iter().copied().filter(|l| !l.is_empty()))
            .collect::<Vec<_>>()
            .join(" ");
        quoted(&joined)?
    } else if rest.is_empty() {
        let nested = body
            .iter()
            .any(|l| l.starts_with("- ") || l.ends_with(':') || l.contains(": "));
        if nested {
            return None;
        }
        body.iter()
            .filter(|l| !l.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        let joined = std::iter::once(rest)
            .chain(body.iter().copied().filter(|l| !l.is_empty()))
            .collect::<Vec<_>>()
            .join(" ");
        match joined.find(" #") {
            Some(at) => joined[..at].to_owned(),
            None => joined,
        }
    };
    let value = value.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

/// The contents of a `"…"` or `'…'` scalar; `None` when it never closes.
fn quoted(s: &str) -> Option<String> {
    let mut chars = s.chars();
    let quote = chars.next()?;
    let mut out = String::new();
    let mut chars = chars.peekable();
    while let Some(c) = chars.next() {
        match (quote, c) {
            ('\'', '\'') if chars.peek() == Some(&'\'') => {
                chars.next();
                out.push('\'');
            }
            ('"', '\\') => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                other => out.push(other),
            },
            (q, c) if q == c => return Some(out),
            (_, c) => out.push(c),
        }
    }
    None
}

/// Whether `name` can be typed as one word after `/` or `$`.
fn typable(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_BYTES
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| !c.is_whitespace() && !c.is_control() && c != '/' && c != '$')
}

/// `text` with its whitespace runs made single spaces, cut to `max` characters with a closing `…`.
fn cut(text: &str, max: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= max {
        return text;
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// The skill one file describes, or `None` when it opts out of the slash menu or has no name that can be typed.
pub fn skill_of(
    cli: AgentCli,
    file: SkillFile<'_>,
    meta: &FrontMatter,
    source: SkillSource,
    plugin: Option<&str>,
) -> Option<Skill> {
    if meta.user_invocable == Some(false) || plugin.is_some_and(|p| !typable(p)) {
        return None;
    }
    let name = match file {
        SkillFile::Skill { dir_name } => meta
            .name
            .as_deref()
            .filter(|n| typable(n))
            .unwrap_or(dir_name),
        SkillFile::Command { stem } | SkillFile::Prompt { stem } => stem,
    };
    if !typable(name) {
        return None;
    }
    let invocation = match (cli, file, plugin) {
        (_, SkillFile::Prompt { .. }, _) => format!("/prompts:{name}"),
        (AgentCli::Codex, _, _) => format!("${name}"),
        (AgentCli::Claude, _, Some(plugin)) => format!("/{plugin}:{name}"),
        (AgentCli::Claude, _, None) => format!("/{name}"),
    };
    Some(Skill {
        name: name.to_owned(),
        invocation,
        description: meta
            .description
            .as_deref()
            .map(|d| cut(d, MAX_DESCRIPTION_CHARS)),
        argument_hint: meta
            .argument_hint
            .as_deref()
            .map(|h| cut(h, MAX_HINT_CHARS)),
        source,
        plugin: plugin.map(str::to_owned),
    })
}

/// The `enabledPlugins` map of a Claude Code `settings.json`, empty when it has none; errors: [`crate::Error::Json`].
pub fn enabled_plugins(settings_json: &str) -> Result<BTreeMap<String, bool>> {
    let value: Value = serde_json::from_str(settings_json).map_err(json("Claude Code settings"))?;
    Ok(value
        .get("enabledPlugins")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(k, v)| v.as_bool().map(|b| (k.clone(), b)))
                .collect()
        })
        .unwrap_or_default())
}

/// The install folders of the plugins `enabled` turns on, by name; a project-scoped install counts only in its own `project`.
/// errors: [`crate::Error::Json`] when the registry is not JSON; an unexpected shape yields no roots.
pub fn plugin_roots(
    installed_json: &str,
    enabled: &BTreeMap<String, bool>,
    project: &str,
) -> Result<Vec<PluginRoot>> {
    let value: Value =
        serde_json::from_str(installed_json).map_err(json("Claude Code plugin registry"))?;
    let Some(plugins) = value.get("plugins").and_then(Value::as_object) else {
        return Ok(Vec::new());
    };
    let mut roots: Vec<PluginRoot> = plugins
        .iter()
        .filter(|(key, _)| enabled.get(key.as_str()) == Some(&true))
        .filter_map(|(key, installs)| {
            let name = key.split('@').next().filter(|n| typable(n))?;
            let path = installs.as_array()?.iter().find_map(|install| {
                let own_project = install
                    .get("projectPath")
                    .and_then(Value::as_str)
                    .is_none_or(|p| p == project);
                let path = PathBuf::from(install.get("installPath")?.as_str()?);
                (own_project && path.is_absolute()).then_some(path)
            })?;
            Some(PluginRoot {
                name: name.to_owned(),
                path,
            })
        })
        .collect();
    roots.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(roots)
}
