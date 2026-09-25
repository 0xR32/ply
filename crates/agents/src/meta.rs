//! What a session reports about itself (spec 6.5, `pane.meta` per Ruling R4): model, worktree, directory, session id.

/// The directory Claude Code keeps its worktrees in, below the repository root (ADR-0003, R15).
pub const CLAUDE_WORKTREES_DIR: &str = "/.claude/worktrees/";

/// A pane's session facts as the CLI reports them; `None` means not reported yet, and ply never chooses any of them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionMeta {
    /// The CLI's own session id (Claude `session_id`, Codex thread id), stored for `--resume` / `resume`.
    pub session_ref: Option<String>,
    /// Model name exactly as reported (Claude `SessionStart.model`, Codex `turn_context.model`).
    pub model: Option<String>,
    /// Working directory the CLI last reported, absolute.
    pub cwd: Option<String>,
    /// Worktree name derived from `cwd` by [`worktree_from_cwd`]; `None` outside a CLI worktree (INV-7).
    pub worktree: Option<String>,
}

impl SessionMeta {
    /// Records a reported working directory and re-derives `worktree`; returns whether anything changed.
    pub fn set_cwd(&mut self, cwd: &str) -> bool {
        if cwd.is_empty() || self.cwd.as_deref() == Some(cwd) {
            return false;
        }
        self.cwd = Some(cwd.to_owned());
        self.worktree = worktree_from_cwd(cwd);
        true
    }

    /// Records a reported model; empty names are ignored; returns whether it changed.
    pub fn set_model(&mut self, model: &str) -> bool {
        set_if_changed(&mut self.model, model)
    }

    /// Records the CLI's session id; empty ids are ignored; returns whether it changed.
    pub fn set_session_ref(&mut self, id: &str) -> bool {
        set_if_changed(&mut self.session_ref, id)
    }
}

fn set_if_changed(slot: &mut Option<String>, value: &str) -> bool {
    if value.is_empty() || slot.as_deref() == Some(value) {
        return false;
    }
    *slot = Some(value.to_owned());
    true
}

/// The `<name>` of a cwd at or below `<repo>/.claude/worktrees/<name>`, using the innermost such directory; else `None`.
pub fn worktree_from_cwd(cwd: &str) -> Option<String> {
    let (_, rest) = cwd.rsplit_once(CLAUDE_WORKTREES_DIR)?;
    let name = rest.split('/').next()?;
    (!name.is_empty()).then(|| name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_comes_from_the_claude_worktrees_path() {
        let cases = [
            (
                "/Users/example/repo/.claude/worktrees/feat-x",
                Some("feat-x"),
            ),
            (
                "/Users/example/repo/.claude/worktrees/feat-x/src/deep",
                Some("feat-x"),
            ),
            ("/Users/example/repo/.claude/worktrees/", None),
            ("/Users/example/repo", None),
            ("/Users/example/.claude/worktrees-old/x", None),
        ];
        for (cwd, want) in cases {
            assert_eq!(worktree_from_cwd(cwd).as_deref(), want, "{cwd}");
        }
    }

    #[test]
    fn setters_report_changes_only() {
        let mut meta = SessionMeta::default();
        assert!(meta.set_cwd("/Users/example/repo/.claude/worktrees/a"));
        assert_eq!(meta.worktree.as_deref(), Some("a"));
        assert!(!meta.set_cwd("/Users/example/repo/.claude/worktrees/a"));
        assert!(meta.set_cwd("/Users/example/repo"));
        assert_eq!(meta.worktree, None);
        assert!(meta.set_model("gpt-6-sol"));
        assert!(!meta.set_model("gpt-6-sol"));
        assert!(!meta.set_model(""));
        assert!(meta.set_session_ref("id-1"));
        assert!(!meta.set_session_ref("id-1"));
    }
}
