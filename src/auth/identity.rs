//! Read-only extraction of account identity from Codex's auth.json bytes.
//!
//! Mirrors the claims Codex itself reads (`codex-rs/login/src/token_data.rs`):
//! top-level `email` (fallback `https://api.openai.com/profile.email`) and the
//! `https://api.openai.com/auth` object (`chatgpt_plan_type`, `chatgpt_user_id`
//! / `user_id`, `chatgpt_account_id`). Unknown fields are ignored so future
//! additions to auth.json do not break parsing, and access/refresh tokens are
//! never materialized as strings.

use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};

use super::jwt;
use crate::error::{Error, Result};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountIdentity {
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub plan_type: Option<String>,
    /// ChatGPT account (workspace) id. Personal and Business workspaces differ here.
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
}

impl AccountIdentity {
    /// Two logins belong to the same switchable account when the workspace matches and
    /// the user matches (by user id, or by email when a user id is unavailable).
    /// The same email in a Personal and a Business workspace are *different* accounts.
    pub fn same_account(&self, other: &AccountIdentity) -> bool {
        let (Some(a), Some(b)) = (&self.account_id, &other.account_id) else {
            return false;
        };
        if a != b {
            return false;
        }
        match (&self.user_id, &other.user_id) {
            (Some(x), Some(y)) => x == y,
            _ => match (&self.email, &other.email) {
                (Some(x), Some(y)) => x.eq_ignore_ascii_case(y),
                _ => false,
            },
        }
    }
}

#[derive(Deserialize)]
struct AuthFileView {
    #[serde(default)]
    tokens: Option<TokensView>,
    #[serde(rename = "OPENAI_API_KEY", default)]
    api_key: Option<IgnoredAny>,
}

#[derive(Deserialize)]
struct TokensView {
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    account_id: Option<String>,
}

#[derive(Deserialize, Default)]
struct Claims {
    #[serde(default)]
    email: Option<String>,
    #[serde(rename = "https://api.openai.com/profile", default)]
    profile: Option<ProfileClaims>,
    #[serde(rename = "https://api.openai.com/auth", default)]
    auth: Option<AuthClaims>,
}

#[derive(Deserialize, Default)]
struct ProfileClaims {
    #[serde(default)]
    email: Option<String>,
}

#[derive(Deserialize, Default)]
struct AuthClaims {
    #[serde(default)]
    chatgpt_plan_type: Option<String>,
    #[serde(default)]
    chatgpt_user_id: Option<String>,
    #[serde(default)]
    user_id: Option<String>,
    #[serde(default)]
    chatgpt_account_id: Option<String>,
}

pub fn parse_identity(auth_json: &[u8]) -> Result<AccountIdentity> {
    let view: AuthFileView = serde_json::from_slice(auth_json)
        .map_err(|e| Error::InvalidCredentials(format!("auth.json is not valid JSON (line {}, column {})", e.line(), e.column())))?;

    let Some(tokens) = view.tokens else {
        return Err(Error::InvalidCredentials(if view.api_key.is_some() {
            "API-key logins cannot be switched; sign in to Codex with ChatGPT".into()
        } else {
            "no ChatGPT tokens present".into()
        }));
    };
    let id_token = tokens
        .id_token
        .ok_or_else(|| Error::InvalidCredentials("id_token missing".into()))?;
    let payload = jwt::decode_payload(&id_token)
        .ok_or_else(|| Error::InvalidCredentials("id_token is not a JWT".into()))?;
    drop(id_token);
    let claims: Claims = serde_json::from_slice(&payload)
        .map_err(|_| Error::InvalidCredentials("id_token claims are not JSON".into()))?;

    let auth = claims.auth.unwrap_or_default();
    let identity = AccountIdentity {
        email: claims.email.or(claims.profile.and_then(|p| p.email)),
        plan_type: auth.chatgpt_plan_type,
        account_id: auth.chatgpt_account_id.or(tokens.account_id),
        user_id: auth.chatgpt_user_id.or(auth.user_id),
    };
    if identity.account_id.is_none() {
        return Err(Error::InvalidCredentials("ChatGPT account id missing".into()));
    }
    Ok(identity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::fixtures::auth_json;

    #[test]
    fn extracts_identity_fields() {
        let bytes = auth_json("a@example.com", "acct-1", "user-1", "pro", "rt-1");
        let id = parse_identity(&bytes).unwrap();
        assert_eq!(id.email.as_deref(), Some("a@example.com"));
        assert_eq!(id.plan_type.as_deref(), Some("pro"));
        assert_eq!(id.account_id.as_deref(), Some("acct-1"));
        assert_eq!(id.user_id.as_deref(), Some("user-1"));
    }

    #[test]
    fn same_email_different_workspace_is_different_account() {
        let personal = parse_identity(&auth_json("t@example.com", "acct-personal", "user-1", "pro", "r1")).unwrap();
        let business = parse_identity(&auth_json("t@example.com", "acct-biz", "user-1", "business", "r2")).unwrap();
        assert!(!personal.same_account(&business));
        assert!(personal.same_account(&personal.clone()));
    }

    #[test]
    fn token_rotation_keeps_account_identity() {
        let before = parse_identity(&auth_json("a@example.com", "acct", "u", "pro", "old")).unwrap();
        let after = parse_identity(&auth_json("a@example.com", "acct", "u", "pro", "new")).unwrap();
        assert!(before.same_account(&after));
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let mut v: serde_json::Value = serde_json::from_slice(&auth_json("a@example.com", "acct", "u", "plus", "r")).unwrap();
        v["future_field"] = serde_json::json!({"x": [1, 2, 3]});
        v["tokens"]["new_token_kind"] = serde_json::json!("zzz");
        assert!(parse_identity(&serde_json::to_vec(&v).unwrap()).is_ok());
    }

    #[test]
    fn api_key_login_is_rejected() {
        let err = parse_identity(br#"{"OPENAI_API_KEY":"sk-test","tokens":null}"#).unwrap_err();
        assert!(err.to_string().contains("API-key"));
        assert!(!err.to_string().contains("sk-test"));
    }

    #[test]
    fn garbage_is_rejected_without_echoing_content() {
        let err = parse_identity(b"{\"tokens\": {\"id_token\": \"secret-not-jwt\"}}").unwrap_err();
        assert!(!err.to_string().contains("secret-not-jwt"));
        assert!(parse_identity(b"not json").is_err());
    }

    #[test]
    fn email_falls_back_to_profile_claim() {
        let claims = serde_json::json!({
            "https://api.openai.com/profile": {"email": "p@example.com"},
            "https://api.openai.com/auth": {"chatgpt_account_id": "acct", "user_id": "u"}
        });
        let jwt = format!("e30.{}.sig", crate::auth::jwt::base64url_encode(claims.to_string().as_bytes()));
        let file = serde_json::json!({"tokens": {"id_token": jwt, "access_token": "a", "refresh_token": "r"}});
        let id = parse_identity(file.to_string().as_bytes()).unwrap();
        assert_eq!(id.email.as_deref(), Some("p@example.com"));
        assert_eq!(id.user_id.as_deref(), Some("u"));
    }
}
