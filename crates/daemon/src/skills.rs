//! `skill.list` (Ruling R61): the skills and commands Claude Code and Codex offer on this machine, read-only.
//!
//! plyd walks the folders each CLI reads its skills from and hands every file to `ply_agents::skills`, which turns it
//! into a [`Skill`] with the invocation the user types. For Claude Code: the project's `.claude/skills` and
//! `.claude/commands` in the pane's directory, the user's `skills/` and `commands/` in `$CLAUDE_CONFIG_DIR` (else
//! `~/.claude`), the enabled plugins (`enabledPlugins` of the user's and the project's settings files, found in
//! `plugins/installed_plugins.json`) and the skills synced from the user's account (`skills/synced/*/`). For Codex:
//! `.agents/skills` and `.codex/skills` of the pane's directory and each parent up to its repository root,
//! `$CODEX_HOME/skills` (default `~/.codex`) and `~/.agents/skills`, the preinstalled `$CODEX_HOME/skills/.system`, and
//! the custom prompts in `$CODEX_HOME/prompts`. Earlier sources win when two offer the same invocation.
//!
//! Everything is bounded: [`MAX_ENTRIES_PER_DIR`] entries per folder, the first [`MAX_SKILL_FILE_BYTES`] of each skill
//! file, [`MAX_REGISTRY_BYTES`] of a settings file or plugin registry, [`MAX_SKILLS`] in all. A folder or file that is
//! missing is simply absent; one that cannot be read is skipped and logged, never an error. Nothing is ever written
//! (INV-8). An answer is reused for [`CACHE_FOR`] per CLI and directory.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ply_agents::skills::{
    SYNCED_PLUGIN, SkillFile, enabled_plugins, parse_front_matter, plugin_roots, skill_of,
};
use ply_proto::pane::{AgentCli, Skill, SkillList, SkillSource};
use tokio::sync::Mutex;

/// How long one reading answers `skill.list` for the same CLI and directory.
pub const CACHE_FOR: Duration = Duration::from_secs(10);

/// Entries of one folder looked at; a folder with more lists the first ones by name.
pub const MAX_ENTRIES_PER_DIR: usize = 256;

/// Bytes read from the start of a skill, command or prompt file; the front matter must end within them.
pub const MAX_SKILL_FILE_BYTES: u64 = 32 * 1024;

/// Largest settings file or plugin registry read.
pub const MAX_REGISTRY_BYTES: u64 = 1 << 20;

/// Most skills one answer lists.
pub const MAX_SKILLS: usize = 512;

/// Parent directories searched for Codex's project skills above the pane's directory.
pub const MAX_PROJECT_DEPTH: usize = 8;

/// Answers kept, one per CLI and directory; a new one beyond this evicts the oldest.
const CACHED_ANSWERS: usize = 32;

/// Where the CLIs keep their skills; `None` when the environment names no home for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSources {
    /// Claude Code's configuration folder: `$CLAUDE_CONFIG_DIR`, else `~/.claude`.
    pub claude_dir: Option<PathBuf>,
    /// Codex's home: `$CODEX_HOME`, else `~/.codex`.
    pub codex_home: Option<PathBuf>,
    /// The home directory, for `~/.agents/skills`.
    pub home: Option<PathBuf>,
}

impl SkillSources {
    /// The folders the CLIs use in `env` (plyd's login environment, R52).
    pub fn from_env(env: &BTreeMap<String, String>) -> Self {
        let dir = |key: &str| env.get(key).filter(|v| !v.is_empty()).map(PathBuf::from);
        let home = dir("HOME");
        Self {
            claude_dir: dir("CLAUDE_CONFIG_DIR")
                .or_else(|| home.as_ref().map(|h| h.join(".claude"))),
            codex_home: dir("CODEX_HOME").or_else(|| home.as_ref().map(|h| h.join(".codex"))),
            home,
        }
    }
}

/// The skills being collected: first one per invocation wins, at most [`MAX_SKILLS`].
#[derive(Debug, Default)]
struct Collector {
    skills: Vec<Skill>,
    seen: HashSet<String>,
    full: bool,
}

impl Collector {
    fn push(&mut self, skill: Skill) {
        if self.skills.len() >= MAX_SKILLS {
            if !self.full {
                self.full = true;
                tracing::warn!(
                    max = MAX_SKILLS,
                    "more skills than one answer lists; the rest are left out"
                );
            }
            return;
        }
        if self.seen.insert(skill.invocation.clone()) {
            self.skills.push(skill);
        }
    }

    /// Every `<dir>/<name>/SKILL.md` under `root`, by folder name; folders starting with `.` are skipped.
    fn skill_dirs(
        &mut self,
        cli: AgentCli,
        root: &Path,
        source: SkillSource,
        plugin: Option<&str>,
    ) {
        for (name, path) in entries(root) {
            if name.starts_with('.') {
                continue;
            }
            let Some(text) = read_head(&path.join("SKILL.md")) else {
                continue;
            };
            let meta = parse_front_matter(&text);
            if let Some(skill) = skill_of(
                cli,
                SkillFile::Skill { dir_name: &name },
                &meta,
                source,
                plugin,
            ) {
                self.push(skill);
            }
        }
    }

    /// Every `<stem>.md` file directly in `root`, as commands or Codex prompts.
    fn md_files(&mut self, cli: AgentCli, root: &Path, source: SkillSource, plugin: Option<&str>) {
        for (name, path) in entries(root) {
            let Some(stem) = name.strip_suffix(".md").filter(|s| !s.starts_with('.')) else {
                continue;
            };
            if !path.is_file() {
                continue;
            }
            let Some(text) = read_head(&path) else {
                continue;
            };
            let meta = parse_front_matter(&text);
            let file = if source == SkillSource::Prompt {
                SkillFile::Prompt { stem }
            } else {
                SkillFile::Command { stem }
            };
            if let Some(skill) = skill_of(cli, file, &meta, source, plugin) {
                self.push(skill);
            }
        }
    }
}

/// The entries of `dir` by name, at most [`MAX_ENTRIES_PER_DIR`]; a missing folder has none, an unreadable one is logged.
fn entries(dir: &Path) -> Vec<(String, PathBuf)> {
    let read = match std::fs::read_dir(dir) {
        Ok(read) => read,
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
            return Vec::new();
        }
        Err(e) => {
            tracing::warn!(dir = %dir.display(), error = %e, "cannot list a skill folder; skipping it");
            return Vec::new();
        }
    };
    let mut out: Vec<(String, PathBuf)> = read
        .filter_map(|entry| match entry {
            Ok(entry) => Some(entry),
            Err(e) => {
                tracing::warn!(dir = %dir.display(), error = %e, "cannot read a skill folder entry; skipping it");
                None
            }
        })
        .take(MAX_ENTRIES_PER_DIR)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            Some((name, entry.path()))
        })
        .collect();
    out.sort();
    out
}

/// The first `max` bytes of `path` as text; `None` when the file is missing, unreadable (logged) or larger than `max` when `whole`.
fn read_bounded(path: &Path, max: u64, whole: bool) -> Option<String> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
            return None;
        }
        Err(e) => {
            tracing::warn!(file = %path.display(), error = %e, "cannot open a skill file; skipping it");
            return None;
        }
    };
    let mut bytes = Vec::new();
    if let Err(e) = file.take(max + 1).read_to_end(&mut bytes) {
        tracing::warn!(file = %path.display(), error = %e, "cannot read a skill file; skipping it");
        return None;
    }
    if bytes.len() as u64 > max {
        if whole {
            tracing::warn!(file = %path.display(), max, "a settings file or plugin registry is too large; skipping it");
            return None;
        }
        bytes.truncate(usize::try_from(max).unwrap_or(usize::MAX));
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn read_head(path: &Path) -> Option<String> {
    read_bounded(path, MAX_SKILL_FILE_BYTES, false)
}

fn read_registry(path: &Path) -> Option<String> {
    read_bounded(path, MAX_REGISTRY_BYTES, true)
}

/// The enabled plugins' install folders: the user's settings, then the project's (a later file wins).
fn claude_plugins(claude_dir: &Path, cwd: &Path) -> Vec<ply_agents::skills::PluginRoot> {
    let settings = [
        claude_dir.join("settings.json"),
        cwd.join(".claude/settings.json"),
        cwd.join(".claude/settings.local.json"),
    ];
    let mut enabled = BTreeMap::new();
    for path in &settings {
        let Some(text) = read_registry(path) else {
            continue;
        };
        match enabled_plugins(&text) {
            Ok(map) => enabled.extend(map),
            Err(e) => {
                tracing::warn!(file = %path.display(), error = %e, "cannot read enabledPlugins; skipping the file");
            }
        }
    }
    if !enabled.values().any(|on| *on) {
        return Vec::new();
    }
    let registry = claude_dir.join("plugins/installed_plugins.json");
    let Some(text) = read_registry(&registry) else {
        return Vec::new();
    };
    match plugin_roots(&text, &enabled, &cwd.to_string_lossy()) {
        Ok(roots) => roots,
        Err(e) => {
            tracing::warn!(file = %registry.display(), error = %e, "cannot read the plugin registry; listing no plugin skills");
            Vec::new()
        }
    }
}

/// `cwd` and its parents up to the first that holds `.git`, at most [`MAX_PROJECT_DEPTH`] above it, never `home` itself.
fn project_dirs<'a>(cwd: &'a Path, home: Option<&'a Path>) -> Vec<&'a Path> {
    let mut out = Vec::new();
    for dir in cwd.ancestors().take(MAX_PROJECT_DEPTH + 1) {
        if Some(dir) == home || dir.parent().is_none() {
            break;
        }
        out.push(dir);
        if dir.join(".git").exists() {
            break;
        }
    }
    out
}

/// Every skill `cli` offers for a pane in `cwd`; blocks on file I/O, so plyd calls it through [`SkillCache::get`].
pub fn read_skills(sources: &SkillSources, cli: AgentCli, cwd: &Path) -> SkillList {
    let mut c = Collector::default();
    match cli {
        AgentCli::Claude => {
            c.skill_dirs(cli, &cwd.join(".claude/skills"), SkillSource::Project, None);
            c.md_files(
                cli,
                &cwd.join(".claude/commands"),
                SkillSource::Project,
                None,
            );
            if let Some(dir) = &sources.claude_dir {
                c.skill_dirs(cli, &dir.join("skills"), SkillSource::User, None);
                c.md_files(cli, &dir.join("commands"), SkillSource::User, None);
                for root in claude_plugins(dir, cwd) {
                    c.skill_dirs(
                        cli,
                        &root.path.join("skills"),
                        SkillSource::Plugin,
                        Some(&root.name),
                    );
                    c.md_files(
                        cli,
                        &root.path.join("commands"),
                        SkillSource::Plugin,
                        Some(&root.name),
                    );
                }
                for (name, account) in entries(&dir.join("skills/synced")) {
                    if !name.starts_with('.') {
                        c.skill_dirs(cli, &account, SkillSource::Plugin, Some(SYNCED_PLUGIN));
                    }
                }
            }
        }
        AgentCli::Codex => {
            for dir in project_dirs(cwd, sources.home.as_deref()) {
                c.skill_dirs(cli, &dir.join(".agents/skills"), SkillSource::Project, None);
                c.skill_dirs(cli, &dir.join(".codex/skills"), SkillSource::Project, None);
            }
            if let Some(home) = &sources.codex_home {
                c.skill_dirs(cli, &home.join("skills"), SkillSource::User, None);
            }
            if let Some(home) = &sources.home {
                c.skill_dirs(cli, &home.join(".agents/skills"), SkillSource::User, None);
            }
            if let Some(home) = &sources.codex_home {
                c.skill_dirs(cli, &home.join("skills/.system"), SkillSource::System, None);
                c.md_files(cli, &home.join("prompts"), SkillSource::Prompt, None);
            }
        }
    }
    SkillList { skills: c.skills }
}

/// The last `skill.list` answers, one per CLI and directory, each reused for [`CACHE_FOR`].
#[derive(Debug, Default)]
pub struct SkillCache {
    answers: Mutex<HashMap<(AgentCli, PathBuf), (Instant, SkillList)>>,
}

impl SkillCache {
    /// The skills `cli` offers in `cwd`, read on a blocking thread unless a fresh answer is cached.
    pub async fn get(&self, sources: SkillSources, cli: AgentCli, cwd: PathBuf) -> SkillList {
        let key = (cli, cwd.clone());
        if let Some((at, list)) = self.answers.lock().await.get(&key)
            && at.elapsed() < CACHE_FOR
        {
            return list.clone();
        }
        let list = match tokio::task::spawn_blocking(move || read_skills(&sources, cli, &cwd)).await
        {
            Ok(list) => list,
            Err(e) => {
                tracing::error!(error = %e, "the skill reader failed; answering no skills");
                return SkillList::default();
            }
        };
        let mut answers = self.answers.lock().await;
        if answers.len() >= CACHED_ANSWERS
            && !answers.contains_key(&key)
            && let Some(oldest) = answers
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(k, _)| k.clone())
        {
            answers.remove(&oldest);
        }
        answers.insert(key, (Instant::now(), list.clone()));
        list
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("ply-skills-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }

        fn write(&self, rel: &str, text: &str) {
            let path = self.0.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn sources(root: &Path) -> SkillSources {
        SkillSources {
            claude_dir: Some(root.join(".claude")),
            codex_home: Some(root.join(".codex")),
            home: Some(root.to_path_buf()),
        }
    }

    fn invocations(list: &SkillList) -> Vec<&str> {
        list.skills.iter().map(|s| s.invocation.as_str()).collect()
    }

    #[test]
    fn sources_follow_the_login_environment() {
        let env = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect()
        };
        let plain = SkillSources::from_env(&env(&[("HOME", "/Users/example")]));
        assert_eq!(
            plain.claude_dir,
            Some(PathBuf::from("/Users/example/.claude"))
        );
        assert_eq!(
            plain.codex_home,
            Some(PathBuf::from("/Users/example/.codex"))
        );
        let moved = SkillSources::from_env(&env(&[
            ("HOME", "/Users/example"),
            ("CLAUDE_CONFIG_DIR", "/Users/example/cc"),
            ("CODEX_HOME", "/Users/example/cx"),
        ]));
        assert_eq!(moved.claude_dir, Some(PathBuf::from("/Users/example/cc")));
        assert_eq!(moved.codex_home, Some(PathBuf::from("/Users/example/cx")));
        assert_eq!(SkillSources::from_env(&env(&[])).claude_dir, None);
    }

    #[test]
    fn an_earlier_source_wins_an_invocation_and_hidden_folders_are_skipped() {
        let d = Dir::new("dedupe");
        d.write(
            "p/.claude/skills/review/SKILL.md",
            "---\ndescription: project\n---\n",
        );
        d.write(
            ".claude/skills/review/SKILL.md",
            "---\ndescription: user\n---\n",
        );
        d.write(".claude/skills/.cache/SKILL.md", "---\nname: cache\n---\n");
        let list = read_skills(&sources(&d.0), AgentCli::Claude, &d.0.join("p"));
        assert_eq!(invocations(&list), ["/review"]);
        assert_eq!(list.skills[0].description.as_deref(), Some("project"));
    }

    #[test]
    fn front_matter_past_the_read_limit_does_not_count() {
        let d = Dir::new("limit");
        let padding = "x".repeat(usize::try_from(MAX_SKILL_FILE_BYTES).unwrap());
        d.write(
            ".claude/skills/big/SKILL.md",
            &format!("---\nname: renamed\ndescription: {padding}\n---\n"),
        );
        let list = read_skills(&sources(&d.0), AgentCli::Claude, &d.0.join("p"));
        assert_eq!(
            invocations(&list),
            ["/big"],
            "an unclosed block is no front matter"
        );
    }

    #[test]
    fn a_folder_lists_at_most_its_limit_and_one_answer_at_most_max_skills() {
        let d = Dir::new("many");
        for i in 0..(MAX_ENTRIES_PER_DIR + 10) {
            d.write(&format!(".claude/commands/c{i:04}.md"), "x\n");
        }
        let list = read_skills(&sources(&d.0), AgentCli::Claude, &d.0.join("p"));
        assert_eq!(list.skills.len(), MAX_ENTRIES_PER_DIR);
        let mut c = Collector::default();
        for i in 0..(MAX_SKILLS + 5) {
            c.push(Skill {
                name: format!("s{i}"),
                invocation: format!("/s{i}"),
                description: None,
                argument_hint: None,
                source: SkillSource::User,
                plugin: None,
            });
        }
        assert_eq!(c.skills.len(), MAX_SKILLS);
    }

    #[test]
    fn an_oversized_or_broken_registry_lists_no_plugins() {
        let d = Dir::new("registry");
        d.write(
            ".claude/settings.json",
            "{\"enabledPlugins\": {\"p@m\": true}}",
        );
        d.write(".claude/plugins/installed_plugins.json", "{ not json");
        let list = read_skills(&sources(&d.0), AgentCli::Claude, &d.0.join("p"));
        assert!(list.skills.is_empty());
        let huge = format!(
            "{{\"enabledPlugins\": {{\"p@m\": true}}, \"pad\": \"{}\"}}",
            "x".repeat(1 << 20)
        );
        d.write(".claude/settings.json", &huge);
        assert!(claude_plugins(&d.0.join(".claude"), &d.0.join("p")).is_empty());
    }

    #[test]
    fn codex_project_folders_stop_at_the_repository_root_and_never_take_the_home() {
        let d = Dir::new("project");
        std::fs::create_dir_all(d.0.join("repo/.git")).unwrap();
        std::fs::create_dir_all(d.0.join("repo/a/b")).unwrap();
        let cwd = d.0.join("repo/a/b");
        let dirs = project_dirs(&cwd, Some(&d.0));
        assert_eq!(
            dirs,
            [cwd.as_path(), &d.0.join("repo/a"), &d.0.join("repo")]
        );
        assert!(project_dirs(&d.0, Some(&d.0)).is_empty());
    }
}
