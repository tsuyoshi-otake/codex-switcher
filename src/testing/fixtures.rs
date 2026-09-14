use serde_json::json;

use crate::auth::jwt::base64url_encode;

/// Builds an auth.json shaped like Codex's `AuthDotJson` with an unsigned id_token.
pub fn auth_json(email: &str, account_id: &str, user_id: &str, plan: &str, refresh_token: &str) -> Vec<u8> {
    let claims = json!({
        "email": email,
        "https://api.openai.com/auth": {
            "chatgpt_plan_type": plan,
            "chatgpt_user_id": user_id,
            "chatgpt_account_id": account_id
        }
    });
    let id_token = format!(
        "{}.{}.signature",
        base64url_encode(br#"{"alg":"none"}"#),
        base64url_encode(claims.to_string().as_bytes())
    );
    json!({
        "auth_mode": "chatgpt",
        "OPENAI_API_KEY": null,
        "tokens": {
            "id_token": id_token,
            "access_token": format!("at-{refresh_token}"),
            "refresh_token": refresh_token,
            "account_id": account_id
        },
        "last_refresh": "2026-09-10T00:00:00Z"
    })
    .to_string()
    .into_bytes()
}
