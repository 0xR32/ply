//! Claude Code's sign-in, through its own commands only: `claude auth status --json` says which claude.ai account it
//! uses, and `claude auth login` in a pane signs in to another; ply runs neither itself here and stores no credential.
//!
//! The status JSON is not a published format, so it is read tolerantly: an account needs `loggedIn: true`, a claude.ai
//! `authMethod` and an `email`; `orgId`, `orgName` and `subscriptionType` are taken when they are strings, and every
//! other key is ignored.

use std::path::Path;

use ply_proto::pane::ClaudeAccount;
use serde_json::Value;

use crate::adapter::{check_value, utf8};
use crate::error::{Error, Result, invalid, json};

/// Longest email address accepted for `--email` (RFC 5321's path limit).
pub const MAX_EMAIL_BYTES: usize = 254;

/// The `authMethod` of a claude.ai sign-in, personal or an organization's.
pub const CLAUDE_AI: &str = "claude.ai";

/// The plans of an organization rather than a person (`subscriptionType`).
pub const ORGANIZATION_PLANS: [&str; 2] = ["team", "enterprise"];

/// `claude auth status --json`: prints the sign-in as one JSON object and exits.
pub fn status_argv(program: &Path) -> Result<Vec<String>> {
    Ok(vec![
        utf8(program)?,
        "auth".to_owned(),
        "status".to_owned(),
        "--json".to_owned(),
    ])
}

/// `claude auth login --claudeai [--email <email>]`, run in a pane: Claude's own browser sign-in, which writes Claude Code's own credentials.
/// Fails with [`Error::InvalidLaunch`] for an email that is empty, too long, holds whitespace or control characters, lacks `@` or reads as an option.
pub fn sign_in_argv(program: &Path, email: Option<&str>) -> Result<Vec<String>> {
    let mut argv = vec![
        utf8(program)?,
        "auth".to_owned(),
        "login".to_owned(),
        "--claudeai".to_owned(),
    ];
    if let Some(email) = email {
        check_email(email)?;
        argv.extend(["--email".to_owned(), email.to_owned()]);
    }
    Ok(argv)
}

fn check_email(email: &str) -> Result<()> {
    check_value("email", email)?;
    if email.len() > MAX_EMAIL_BYTES {
        return Err(Error::InvalidLaunch(format!(
            "email of {} bytes exceeds {MAX_EMAIL_BYTES}",
            email.len()
        )));
    }
    if email.chars().any(|c| c.is_whitespace() || c.is_control()) || !email.contains('@') {
        return Err(Error::InvalidLaunch(format!(
            "{email:?} is not an email address"
        )));
    }
    Ok(())
}

/// The claude.ai account in `claude auth status --json` output; `None` when signed out or signed in another way (API key, cloud provider).
/// Fails with [`Error::Json`] when the output is not JSON, or [`Error::InvalidPayload`] when it is not an object.
pub fn parse_auth_status(bytes: &[u8]) -> Result<Option<ClaudeAccount>> {
    let status: Value = serde_json::from_slice(bytes).map_err(json("claude auth status"))?;
    if !status.is_object() {
        return Err(invalid("claude auth status", "not a JSON object"));
    }
    let text = |key: &str| {
        status
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let signed_in = status.get("loggedIn").and_then(Value::as_bool) == Some(true)
        && text("authMethod").as_deref() == Some(CLAUDE_AI);
    if !signed_in {
        return Ok(None);
    }
    Ok(text("email").map(|email| ClaudeAccount {
        email,
        org_id: text("orgId"),
        org_name: text("orgName"),
        plan: text("subscriptionType"),
    }))
}

/// Whether `a` and `b` are one sign-in: the same email, ignoring case, and the same organization when both name one.
pub fn same_account(a: &ClaudeAccount, b: &ClaudeAccount) -> bool {
    a.email.eq_ignore_ascii_case(&b.email)
        && match (&a.org_id, &b.org_id) {
            (Some(x), Some(y)) => x == y,
            _ => true,
        }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(email: &str, org: Option<&str>) -> ClaudeAccount {
        ClaudeAccount {
            email: email.to_owned(),
            org_id: org.map(str::to_owned),
            org_name: None,
            plan: None,
        }
    }

    #[test]
    fn a_claude_ai_sign_in_is_an_account_and_anything_else_is_none() {
        let personal = br#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","email":"someone@example.com","orgId":"00000000-0000-4000-8000-0000000000a1","orgName":"someone@example.com's Organization","subscriptionType":"max","later":{"x":1}}"#;
        assert_eq!(
            parse_auth_status(personal).unwrap(),
            Some(ClaudeAccount {
                email: "someone@example.com".to_owned(),
                org_id: Some("00000000-0000-4000-8000-0000000000a1".to_owned()),
                org_name: Some("someone@example.com's Organization".to_owned()),
                plan: Some("max".to_owned()),
            })
        );
        let team = br#"{"loggedIn":true,"authMethod":"claude.ai","email":"someone@example.com","orgName":7,"subscriptionType":"team"}"#;
        let team = parse_auth_status(team).unwrap().unwrap();
        assert_eq!(team.org_name, None, "a field of another type is left out");
        assert_eq!(team.plan.as_deref(), Some("team"));

        for other in [
            &br#"{"loggedIn":false}"#[..],
            br#"{"loggedIn":true,"authMethod":"api_key","apiKeySource":"ANTHROPIC_API_KEY"}"#,
            br#"{"loggedIn":true,"authMethod":"claude.ai"}"#,
            br#"{"loggedIn":"yes","authMethod":"claude.ai","email":"someone@example.com"}"#,
        ] {
            assert_eq!(parse_auth_status(other).unwrap(), None);
        }
        assert!(parse_auth_status(b"Not logged in").is_err());
        assert!(parse_auth_status(b"[]").is_err());
    }

    #[test]
    fn the_sign_in_fills_in_a_valid_email_and_refuses_anything_else() {
        let claude = Path::new("/Users/example/.local/bin/claude");
        assert_eq!(
            sign_in_argv(claude, Some("someone@example.com")).unwrap(),
            [
                "/Users/example/.local/bin/claude",
                "auth",
                "login",
                "--claudeai",
                "--email",
                "someone@example.com"
            ]
        );
        assert_eq!(
            sign_in_argv(claude, None).unwrap(),
            [
                "/Users/example/.local/bin/claude",
                "auth",
                "login",
                "--claudeai"
            ]
        );
        let long = format!("{}@example.com", "a".repeat(MAX_EMAIL_BYTES));
        for bad in [
            "",
            "--sso",
            "someone",
            "some one@example.com",
            "someone@example.com\n",
            long.as_str(),
        ] {
            assert!(
                matches!(
                    sign_in_argv(claude, Some(bad)),
                    Err(Error::InvalidLaunch(_))
                ),
                "{bad:?}"
            );
        }
        assert_eq!(
            status_argv(claude).unwrap(),
            [
                "/Users/example/.local/bin/claude",
                "auth",
                "status",
                "--json"
            ]
        );
    }

    #[test]
    fn one_email_in_two_organizations_is_two_accounts() {
        let personal = account("Someone@Example.com", Some("org-a"));
        assert!(same_account(
            &personal,
            &account("someone@example.com", Some("org-a"))
        ));
        assert!(!same_account(
            &personal,
            &account("someone@example.com", Some("org-b"))
        ));
        assert!(same_account(
            &personal,
            &account("someone@example.com", None)
        ));
        assert!(!same_account(
            &personal,
            &account("other@example.com", Some("org-a"))
        ));
    }
}
