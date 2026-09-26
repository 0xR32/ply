//! Skill and command files → `Skill` records (Ruling R61): front matter, names, invocations and the plugin registry.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ply_agents::skills::{
    FrontMatter, MAX_DESCRIPTION_CHARS, SYNCED_PLUGIN, SkillFile, enabled_plugins,
    parse_front_matter, plugin_roots, skill_of,
};
use ply_proto::pane::{AgentCli, Skill, SkillSource};

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/skills")
        .join(name);
    std::fs::read_to_string(path).unwrap()
}

#[test]
fn front_matter_reads_plain_quoted_and_folded_values() {
    let plain = parse_front_matter(
        "---\nname: review-pr\ndescription: Full pre-merge review: code, security, fixes\nargument-hint: '[#PR or URL]'\n---\n\n# Body\n",
    );
    assert_eq!(plain.name.as_deref(), Some("review-pr"));
    assert_eq!(
        plain.description.as_deref(),
        Some("Full pre-merge review: code, security, fixes")
    );
    assert_eq!(plain.argument_hint.as_deref(), Some("[#PR or URL]"));
    assert_eq!(plain.user_invocable, None);

    let quoted = parse_front_matter(
        "---\nname: brainstorming\ndescription: \"You MUST use this \\\"before\\\" any work\"\n---\n",
    );
    assert_eq!(
        quoted.description.as_deref(),
        Some("You MUST use this \"before\" any work")
    );

    let single = parse_front_matter("---\ndescription: 'it''s single-quoted'\n---\n");
    assert_eq!(single.description.as_deref(), Some("it's single-quoted"));

    let folded = parse_front_matter(
        "---\nname: archify\ndescription: >\n  Create polished diagrams\n  as standalone HTML.\nlicense: MIT\n---\n",
    );
    assert_eq!(
        folded.description.as_deref(),
        Some("Create polished diagrams as standalone HTML.")
    );

    let continued = parse_front_matter(
        "---\ndescription: A plain value that\n  goes on over two lines\nname: x\n---\n",
    );
    assert_eq!(
        continued.description.as_deref(),
        Some("A plain value that goes on over two lines")
    );
    assert_eq!(continued.name.as_deref(), Some("x"));
}

#[test]
fn nested_maps_comments_and_unknown_keys_are_ignored() {
    let meta = parse_front_matter(
        "---\n# a comment\nname: archify\nlicense: MIT\nmetadata:\n  version: \"2.17\"\n  name: not-this\nuser-invocable: false\ndisable-model-invocation: true\n---\n",
    );
    assert_eq!(meta.name.as_deref(), Some("archify"));
    assert_eq!(meta.description, None);
    assert_eq!(meta.user_invocable, Some(false));
}

#[test]
fn crlf_and_a_byte_order_mark_still_parse() {
    let meta = parse_front_matter("\u{feff}---\r\nname: x\r\ndescription: y\r\n---\r\nbody\r\n");
    assert_eq!(meta.name.as_deref(), Some("x"));
    assert_eq!(meta.description.as_deref(), Some("y"));
}

#[test]
fn a_file_without_front_matter_or_without_its_end_has_none() {
    assert_eq!(
        parse_front_matter("# Just a command\n\nDo the thing.\n"),
        FrontMatter::default()
    );
    assert_eq!(
        parse_front_matter("---\nname: x\nno end marker\n"),
        FrontMatter::default()
    );
    assert_eq!(parse_front_matter(""), FrontMatter::default());
}

fn meta(name: Option<&str>, description: Option<&str>) -> FrontMatter {
    FrontMatter {
        name: name.map(str::to_owned),
        description: description.map(str::to_owned),
        ..FrontMatter::default()
    }
}

#[test]
fn claude_skills_take_the_front_matter_name_and_plugins_prefix_it() {
    let user = skill_of(
        AgentCli::Claude,
        SkillFile::Skill {
            dir_name: "review-dir",
        },
        &meta(Some("review-pr"), Some("Review a PR")),
        SkillSource::User,
        None,
    )
    .unwrap();
    assert_eq!(
        user,
        Skill {
            name: "review-pr".into(),
            invocation: "/review-pr".into(),
            description: Some("Review a PR".into()),
            argument_hint: None,
            source: SkillSource::User,
            plugin: None,
        }
    );
    let from_dir = skill_of(
        AgentCli::Claude,
        SkillFile::Skill {
            dir_name: "team:deploy",
        },
        &FrontMatter::default(),
        SkillSource::Project,
        None,
    )
    .unwrap();
    assert_eq!(
        from_dir.invocation, "/team:deploy",
        "a colon in a directory name is kept"
    );
    let plugin = skill_of(
        AgentCli::Claude,
        SkillFile::Skill {
            dir_name: "brainstorming",
        },
        &meta(Some("brainstorming"), None),
        SkillSource::Plugin,
        Some("superpowers"),
    )
    .unwrap();
    assert_eq!(plugin.invocation, "/superpowers:brainstorming");
    assert_eq!(plugin.plugin.as_deref(), Some("superpowers"));
    let synced = skill_of(
        AgentCli::Claude,
        SkillFile::Skill { dir_name: "pdf" },
        &meta(Some("pdf"), None),
        SkillSource::Plugin,
        Some(SYNCED_PLUGIN),
    )
    .unwrap();
    assert_eq!(synced.invocation, "/anthropic-skills:pdf");
}

#[test]
fn commands_and_prompts_are_named_after_their_file_and_codex_skills_use_a_dollar() {
    let command = skill_of(
        AgentCli::Claude,
        SkillFile::Command { stem: "open-pr" },
        &meta(Some("something-else"), Some("Open a PR")),
        SkillSource::User,
        None,
    )
    .unwrap();
    assert_eq!(
        (command.name.as_str(), command.invocation.as_str()),
        ("open-pr", "/open-pr")
    );
    let prompt = skill_of(
        AgentCli::Codex,
        SkillFile::Prompt { stem: "open-pr" },
        &FrontMatter::default(),
        SkillSource::Prompt,
        None,
    )
    .unwrap();
    assert_eq!(prompt.invocation, "/prompts:open-pr");
    let codex = skill_of(
        AgentCli::Codex,
        SkillFile::Skill {
            dir_name: "review-agent",
        },
        &meta(Some("review-agent"), None),
        SkillSource::System,
        None,
    )
    .unwrap();
    assert_eq!(codex.invocation, "$review-agent");
}

#[test]
fn skills_that_opt_out_or_cannot_be_typed_are_left_out() {
    let hidden = FrontMatter {
        name: Some("hidden".into()),
        user_invocable: Some(false),
        ..FrontMatter::default()
    };
    let file = SkillFile::Skill { dir_name: "hidden" };
    assert_eq!(
        skill_of(AgentCli::Claude, file, &hidden, SkillSource::User, None),
        None
    );
    for bad in [
        "",
        "two words",
        "-flag",
        "a/b",
        "tab\there",
        "$x",
        &"n".repeat(200),
    ] {
        let file = SkillFile::Skill { dir_name: bad };
        assert_eq!(
            skill_of(
                AgentCli::Claude,
                file,
                &FrontMatter::default(),
                SkillSource::User,
                None
            ),
            None,
            "{bad:?} cannot be typed as a skill name"
        );
    }
    let bad_front_matter_name = meta(Some("has space"), None);
    let named_by_dir = skill_of(
        AgentCli::Claude,
        SkillFile::Skill { dir_name: "good" },
        &bad_front_matter_name,
        SkillSource::User,
        None,
    )
    .unwrap();
    assert_eq!(
        named_by_dir.invocation, "/good",
        "an unusable front matter name falls back to the folder"
    );
}

#[test]
fn long_descriptions_and_hints_are_cut_on_a_char_boundary() {
    let long = "é".repeat(MAX_DESCRIPTION_CHARS + 50);
    let skill = skill_of(
        AgentCli::Claude,
        SkillFile::Skill { dir_name: "x" },
        &FrontMatter {
            description: Some(long),
            argument_hint: Some("h".repeat(500)),
            ..FrontMatter::default()
        },
        SkillSource::User,
        None,
    )
    .unwrap();
    let description = skill.description.unwrap();
    assert_eq!(description.chars().count(), MAX_DESCRIPTION_CHARS);
    assert!(description.ends_with('…'));
    assert!(skill.argument_hint.unwrap().chars().count() < 500);
}

#[test]
fn enabled_plugins_come_from_the_settings_and_a_later_file_wins() {
    let mut enabled: BTreeMap<String, bool> = enabled_plugins(&fixture("settings.json")).unwrap();
    assert_eq!(
        enabled.get("superpowers@claude-plugins-official"),
        Some(&true)
    );
    assert_eq!(enabled.get("team-tools@team"), Some(&false));
    enabled.extend(enabled_plugins(&fixture("project-settings.json")).unwrap());
    assert_eq!(enabled.get("team-tools@team"), Some(&true));
    assert!(enabled_plugins("{}").unwrap().is_empty());
    assert!(
        enabled_plugins("{\"enabledPlugins\": []}")
            .unwrap()
            .is_empty()
    );
    assert!(enabled_plugins("not json").is_err());
}

#[test]
fn plugin_roots_are_the_enabled_installs_for_this_project() {
    let mut enabled = enabled_plugins(&fixture("settings.json")).unwrap();
    enabled.extend(enabled_plugins(&fixture("project-settings.json")).unwrap());
    let roots = plugin_roots(
        &fixture("installed_plugins.json"),
        &enabled,
        "/Users/example/project",
    )
    .unwrap();
    let got: Vec<(String, PathBuf)> = roots.into_iter().map(|r| (r.name, r.path)).collect();
    let base = PathBuf::from("/Users/example/.claude/plugins/cache");
    assert_eq!(
        got,
        vec![
            (
                "rust-analyzer-lsp".to_owned(),
                base.join("claude-plugins-official/rust-analyzer-lsp/1.0.0")
            ),
            (
                "superpowers".to_owned(),
                base.join("claude-plugins-official/superpowers/6.4.1")
            ),
            ("team-tools".to_owned(), base.join("team/team-tools/0.4.0")),
        ],
        "enabled only, and a project install only for its own project"
    );
    let elsewhere = plugin_roots(
        &fixture("installed_plugins.json"),
        &enabled,
        "/Users/example/else",
    )
    .unwrap();
    assert!(elsewhere.iter().all(|r| r.name != "team-tools"));
    assert!(
        plugin_roots("{\"plugins\": 3}", &enabled, "/")
            .unwrap()
            .is_empty()
    );
    assert!(plugin_roots("[", &enabled, "/").is_err());
}
