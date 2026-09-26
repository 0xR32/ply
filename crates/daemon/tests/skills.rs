//! `skill.list` against a real plyd (Ruling R61): each CLI's skills from a sandboxed home, in source order, each
//! invocation once, with every file it read left byte for byte as it was (INV-8).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::{Path, PathBuf};

use common::{Control, Sandbox};
use serde_json::{Value, json};

fn write(path: &Path, text: &str) -> PathBuf {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
    path.to_path_buf()
}

fn skill_md(name: &str, description: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}\n---\n\n# {name}\n")
}

fn invocations(list: &Value) -> Vec<String> {
    list["skills"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["invocation"].as_str().unwrap().to_owned())
        .collect()
}

fn read_all(files: &[PathBuf]) -> Vec<Vec<u8>> {
    files.iter().map(|f| std::fs::read(f).unwrap()).collect()
}

#[test]
fn claude_skills_come_from_the_project_the_user_the_enabled_plugins_and_the_synced_ones() {
    let sb = Sandbox::new("skills-claude");
    let claude = sb.home.join(".claude");
    let project = sb.home.join("project");
    let plugin = sb.home.join("plugin-cache/superpowers/6.4.1");
    let files = vec![
        write(
            &project.join(".claude/skills/deploy/SKILL.md"),
            &skill_md("deploy", "Ship it"),
        ),
        write(
            &claude.join("skills/review-pr/SKILL.md"),
            &skill_md("review-pr", "Review a PR"),
        ),
        write(
            &claude.join("skills/hidden/SKILL.md"),
            "---\nname: hidden\nuser-invocable: false\n---\n",
        ),
        write(
            &claude.join("commands/open-pr.md"),
            "Open a pull request for this branch.\n",
        ),
        write(
            &claude.join("skills/synced/u1/pdf/SKILL.md"),
            &skill_md("pdf", "PDF files"),
        ),
        write(
            &claude.join("skills/synced/u1/manifest.json"),
            "{\"skills\": []}\n",
        ),
        write(
            &claude.join("settings.json"),
            "{\"enabledPlugins\": {\"superpowers@official\": true, \"off@official\": false}}\n",
        ),
        write(
            &claude.join("plugins/installed_plugins.json"),
            &json!({"version": 2, "plugins": {
                "superpowers@official": [{"scope": "user", "installPath": plugin}],
                "off@official": [{"scope": "user", "installPath": sb.home.join("off")}]
            }})
            .to_string(),
        ),
        write(
            &plugin.join("skills/brainstorming/SKILL.md"),
            &skill_md("brainstorming", "Explore first"),
        ),
        write(&plugin.join("commands/write-plan.md"), "Write the plan.\n"),
        write(
            &sb.home.join("off/skills/nope/SKILL.md"),
            &skill_md("nope", "Disabled"),
        ),
    ];
    let before = read_all(&files);

    let _plyd = sb.start();
    let mut c = Control::connect(&sb.control_socket()).unwrap();
    let list = c
        .call("skill.list", json!({"cli": "claude", "cwd": project}))
        .unwrap();
    assert_eq!(
        invocations(&list),
        [
            "/deploy",
            "/review-pr",
            "/open-pr",
            "/superpowers:brainstorming",
            "/superpowers:write-plan",
            "/anthropic-skills:pdf",
        ]
    );
    let skills = list["skills"].as_array().unwrap();
    assert_eq!(skills[0]["source"], "project");
    assert_eq!(skills[1]["description"], "Review a PR");
    assert_eq!(skills[3]["source"], "plugin");
    assert_eq!(skills[3]["plugin"], "superpowers");
    assert_eq!(
        c.call("skill.list", json!({"cli": "claude", "cwd": project}))
            .unwrap(),
        list,
        "the cached answer"
    );
    assert_eq!(read_all(&files), before, "INV-8: read-only");
}

#[test]
fn codex_skills_come_from_the_repository_codex_home_the_agents_folder_and_the_prompts() {
    let sb = Sandbox::new("skills-codex");
    let codex = sb.home.join(".codex");
    let repo = sb.home.join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(repo.join("sub")).unwrap();
    let files = vec![
        write(
            &repo.join(".agents/skills/triage/SKILL.md"),
            &skill_md("triage", "Sort bugs"),
        ),
        write(
            &codex.join("skills/review-pr/SKILL.md"),
            &skill_md("review-pr", "Review a PR"),
        ),
        write(
            &codex.join("skills/.system/review-agent/SKILL.md"),
            &skill_md("review-agent", "Review a change"),
        ),
        write(
            &sb.home.join(".agents/skills/archify/SKILL.md"),
            &skill_md("archify", "Diagrams"),
        ),
        write(
            &codex.join("prompts/open-pr.md"),
            "---\ndescription: Open a PR\nargument-hint: '[title]'\n---\nOpen a pull request.\n",
        ),
        write(
            &sb.home.join(".claude/skills/claude-only/SKILL.md"),
            &skill_md("claude-only", "x"),
        ),
    ];
    let before = read_all(&files);

    let _plyd = sb.start();
    let mut c = Control::connect(&sb.control_socket()).unwrap();
    let list = c
        .call(
            "skill.list",
            json!({"cli": "codex", "cwd": repo.join("sub")}),
        )
        .unwrap();
    assert_eq!(
        invocations(&list),
        [
            "$triage",
            "$review-pr",
            "$archify",
            "$review-agent",
            "/prompts:open-pr"
        ]
    );
    let skills = list["skills"].as_array().unwrap();
    assert_eq!(skills[3]["source"], "system");
    assert_eq!(skills[4]["source"], "prompt");
    assert_eq!(skills[4]["argument_hint"], "[title]");
    assert_eq!(read_all(&files), before, "INV-8: read-only");
}

#[test]
fn an_empty_home_lists_nothing_and_a_relative_directory_is_refused() {
    let sb = Sandbox::new("skills-none");
    let _plyd = sb.start();
    let mut c = Control::connect(&sb.control_socket()).unwrap();
    for cli in ["claude", "codex"] {
        let list = c
            .call("skill.list", json!({"cli": cli, "cwd": sb.home}))
            .unwrap();
        assert_eq!(list, json!({"skills": []}), "{cli}");
    }
    let err = c
        .call("skill.list", json!({"cli": "claude", "cwd": "project"}))
        .unwrap_err();
    assert_eq!(serde_json::to_value(err.code).unwrap(), "bad_request");
}
