//! Codex's sign-in, read from the `auth.json` Codex writes and changed only by its own `codex login` in a pane; ply
//! runs neither here, writes nothing under `CODEX_HOME` and keeps no token or key.
//!
//! `auth.json` is not a published format, so it is read tolerantly: `auth_mode` tells a ChatGPT sign-in from an API
//! key, and a file without it counts an `OPENAI_API_KEY` with no `tokens` as a key. A ChatGPT sign-in's email and plan
//! come from the payload of `tokens.id_token`, a JWT decoded without checking its signature, since they are only
//! shown. The key's value and every other token are skipped while parsing, and no error quotes the file.

use std::path::Path;

use ply_proto::pane::CodexAccount;
use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::Value;

use crate::adapter::utf8;
use crate::error::{Result, invalid};

/// The file under `CODEX_HOME` where Codex keeps its sign-in.
pub const AUTH_FILE: &str = "auth.json";

const WHAT: &str = "Codex auth.json";

#[derive(Deserialize)]
struct AuthFile {
    auth_mode: Option<String>,
    #[serde(rename = "OPENAI_API_KEY")]
    api_key: Option<IgnoredAny>,
    tokens: Option<Tokens>,
}

#[derive(Deserialize)]
struct Tokens {
    id_token: Option<String>,
}

#[derive(Deserialize)]
struct Claims {
    email: Option<Value>,
    #[serde(rename = "https://api.openai.com/auth")]
    auth: Option<Value>,
}

/// `codex login`, run in a pane: Codex's own ChatGPT sign-in, which writes its own `auth.json`.
/// errors: [`crate::Error::NonUtf8Path`] when `program` is not UTF-8.
pub fn sign_in_argv(program: &Path) -> Result<Vec<String>> {
    Ok(vec![utf8(program)?, "login".to_owned()])
}

/// The sign-in in the bytes of `$CODEX_HOME/auth.json`; `None` when they hold neither an API key nor ChatGPT tokens.
/// errors: [`crate::Error::InvalidPayload`] when the bytes are not the JSON object Codex writes or a ChatGPT sign-in's ID token is not a readable JWT; the detail never quotes the file.
pub fn parse_auth(bytes: &[u8]) -> Result<Option<CodexAccount>> {
    let file: AuthFile = serde_json::from_slice(bytes)
        .map_err(|_| invalid(WHAT, "not the JSON object Codex writes"))?;
    let api_key = match file.auth_mode.as_deref() {
        Some(mode) => !mode.to_ascii_lowercase().starts_with("chatgpt"),
        None => file.api_key.is_some() && file.tokens.is_none(),
    };
    if api_key {
        return Ok(file.api_key.map(|_| CodexAccount {
            email: None,
            plan: None,
            api_key: true,
        }));
    }
    let Some(tokens) = file.tokens else {
        return Ok(None);
    };
    let token = tokens
        .id_token
        .ok_or_else(|| invalid(WHAT, "a ChatGPT sign-in without an ID token"))?;
    let claims = id_token_claims(&token)?;
    let text = |v: Option<&Value>| {
        v.and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    Ok(Some(CodexAccount {
        email: text(claims.email.as_ref()),
        plan: text(
            claims
                .auth
                .as_ref()
                .and_then(|a| a.get("chatgpt_plan_type")),
        ),
        api_key: false,
    }))
}

/// Whether `a` and `b` are one sign-in: both API keys, or both ChatGPT sign-ins with the same email, ignoring case, or both without one.
pub fn same_account(a: &CodexAccount, b: &CodexAccount) -> bool {
    a.api_key == b.api_key
        && match (&a.email, &b.email) {
            (Some(x), Some(y)) => x.eq_ignore_ascii_case(y),
            (None, None) => true,
            _ => false,
        }
}

fn id_token_claims(token: &str) -> Result<Claims> {
    let mut parts = token.split('.');
    let (Some(_), Some(payload), Some(_), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(invalid(WHAT, "the ID token is not a JWT"));
    };
    let bytes = base64url(payload)
        .ok_or_else(|| invalid(WHAT, "the ID token's payload is not base64url"))?;
    serde_json::from_slice(&bytes)
        .map_err(|_| invalid(WHAT, "the ID token's payload is not a JSON object"))
}

/// RFC 4648 §5 without padding (trailing `=` tolerated); `None` for any other character or an impossible length.
fn base64url(text: &str) -> Option<Vec<u8>> {
    let text = text.trim_end_matches('=');
    if text.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() / 4 * 3 + 2);
    let mut acc: u32 = 0;
    let mut bits = 0;
    for byte in text.bytes() {
        let sextet = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(sextet);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits).to_le_bytes()[0]);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;

    fn encode(bytes: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = chunk
                .iter()
                .enumerate()
                .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
            for i in 0..=chunk.len() {
                out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize]));
            }
        }
        out
    }

    fn jwt(claims: &Value) -> String {
        format!(
            "{}.{}.c2lnbmF0dXJl",
            encode(br#"{"alg":"RS256","typ":"JWT"}"#),
            encode(claims.to_string().as_bytes())
        )
    }

    fn chatgpt(id_token: &str) -> Vec<u8> {
        serde_json::json!({
            "auth_mode": "chatgpt",
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token": id_token,
                "access_token": "example-access-token",
                "refresh_token": "example-refresh-token",
                "account_id": "00000000-0000-4000-8000-0000000000c1"
            },
            "last_refresh": "2026-11-01T00:00:00Z"
        })
        .to_string()
        .into_bytes()
    }

    #[test]
    fn base64url_decodes_every_length_and_refuses_other_alphabets() {
        for text in [
            "",
            "f",
            "fo",
            "foo",
            "foob",
            "fooba",
            "foobar",
            "\u{ff}\u{fe}?>",
        ] {
            assert_eq!(
                base64url(&encode(text.as_bytes())).unwrap(),
                text.as_bytes()
            );
        }
        assert_eq!(base64url("Zm9vYg==").unwrap(), b"foob");
        assert_eq!(base64url("-_8").unwrap(), [0xfb, 0xff]);
        for bad in ["Zm9vY", "Zm+v", "Zm/v", "Zm9v YQ", "Zm9v\n"] {
            assert_eq!(base64url(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_chatgpt_sign_in_gives_the_email_and_plan_of_its_id_token() {
        let token = jwt(&serde_json::json!({
            "iss": "https://auth.openai.com",
            "email": "someone@example.com",
            "email_verified": true,
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "00000000-0000-4000-8000-0000000000c1",
                "chatgpt_plan_type": "pro",
                "chatgpt_user_id": "user-example"
            }
        }));
        assert_eq!(
            parse_auth(&chatgpt(&token)).unwrap(),
            Some(CodexAccount {
                email: Some("someone@example.com".to_owned()),
                plan: Some("pro".to_owned()),
                api_key: false,
            })
        );
        let bare = jwt(&serde_json::json!({"email": 7, "https://api.openai.com/auth": "x"}));
        assert_eq!(
            parse_auth(&chatgpt(&bare)).unwrap(),
            Some(CodexAccount {
                email: None,
                plan: None,
                api_key: false,
            }),
            "claims of another type are left out"
        );
        let older =
            format!(r#"{{"OPENAI_API_KEY":"sk-example","tokens":{{"id_token":"{token}"}}}}"#);
        assert_eq!(
            parse_auth(older.as_bytes()).unwrap().map(|a| a.api_key),
            Some(false),
            "without auth_mode, tokens make it a ChatGPT sign-in"
        );
    }

    #[test]
    fn an_api_key_is_an_account_without_an_email() {
        let key = Some(CodexAccount {
            email: None,
            plan: None,
            api_key: true,
        });
        for file in [
            &br#"{"auth_mode":"apikey","OPENAI_API_KEY":"sk-example","tokens":null}"#[..],
            br#"{"OPENAI_API_KEY":"sk-example"}"#,
            br#"{"auth_mode":"ApiKey","OPENAI_API_KEY":"sk-example","tokens":{"id_token":"not-a-jwt"}}"#,
        ] {
            assert_eq!(parse_auth(file).unwrap(), key);
        }
    }

    #[test]
    fn a_file_without_a_sign_in_is_none() {
        for file in [
            &b"{}"[..],
            br#"{"auth_mode":"chatgpt","OPENAI_API_KEY":null,"tokens":null}"#,
            br#"{"auth_mode":"apikey","OPENAI_API_KEY":null}"#,
            br#"{"OPENAI_API_KEY":null,"last_refresh":null}"#,
        ] {
            assert_eq!(parse_auth(file).unwrap(), None);
        }
    }

    #[test]
    fn a_malformed_file_or_token_is_an_error_that_quotes_nothing_of_it() {
        let secret = "sk-secret-example";
        let header = encode(br#"{"alg":"none"}"#);
        let malformed = [
            String::new(),
            "[]".to_owned(),
            "not json".to_owned(),
            format!(r#"{{"auth_mode":"chatgpt","tokens":"{secret}"}}"#),
            format!(r#"{{"auth_mode":"chatgpt","tokens":{{"id_token":{{"x":"{secret}"}}}}}}"#),
            format!(r#"{{"auth_mode":["{secret}"]}}"#),
            r#"{"auth_mode":"chatgpt","tokens":{"access_token":"x"}}"#.to_owned(),
        ];
        let tokens = [
            secret.to_owned(),
            format!("{header}.{secret}"),
            format!("{header}.{secret}*.sig"),
            format!("{header}.{}.sig", encode(secret.as_bytes())),
            format!("{header}.{}.sig", encode(br#""text""#)),
            format!("{header}.{}.sig.extra", encode(b"{}")),
        ];
        let files = malformed
            .into_iter()
            .map(String::into_bytes)
            .chain(tokens.iter().map(|t| chatgpt(t)));
        for file in files {
            let err = parse_auth(&file).unwrap_err();
            assert!(
                matches!(err, Error::InvalidPayload { .. }),
                "{err:?} for {}",
                String::from_utf8_lossy(&file)
            );
            assert!(!err.to_string().contains("secret"), "{err}");
        }
    }

    #[test]
    fn the_sign_in_is_codex_login_and_nothing_else() {
        assert_eq!(
            sign_in_argv(Path::new("/Users/example/.local/bin/codex")).unwrap(),
            ["/Users/example/.local/bin/codex", "login"]
        );
    }

    #[test]
    fn one_email_is_one_account_and_an_api_key_is_another() {
        let account = |email: Option<&str>, api_key| CodexAccount {
            email: email.map(str::to_owned),
            plan: None,
            api_key,
        };
        let personal = account(Some("Someone@Example.com"), false);
        assert!(same_account(
            &personal,
            &account(Some("someone@example.com"), false)
        ));
        assert!(!same_account(
            &personal,
            &account(Some("other@example.com"), false)
        ));
        assert!(!same_account(&personal, &account(None, true)));
        assert!(!same_account(&personal, &account(None, false)));
        assert!(same_account(&account(None, true), &account(None, true)));
    }
}
