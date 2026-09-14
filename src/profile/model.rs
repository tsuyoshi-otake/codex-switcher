use std::fmt;

use serde::{Deserialize, Serialize};

use crate::auth::AccountIdentity;
use crate::error::{Error, Result};

/// Random (v4) UUID in canonical lowercase form. The textual form is validated on every
/// construction, so a `ProfileId` is always safe to use as a file name.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProfileId(String);

impl ProfileId {
    pub fn parse(s: &str) -> Result<Self> {
        let bytes = s.as_bytes();
        let valid = bytes.len() == 36
            && bytes.iter().enumerate().all(|(i, c)| match i {
                8 | 13 | 18 | 23 => *c == b'-',
                _ => c.is_ascii_hexdigit(),
            });
        if !valid {
            return Err(Error::ProfileNotFound(format!("invalid profile id ({} chars)", s.len())));
        }
        Ok(ProfileId(s.to_ascii_lowercase()))
    }

    pub fn from_random_bytes(mut b: [u8; 16]) -> Self {
        b[6] = (b[6] & 0x0f) | 0x40;
        b[8] = (b[8] & 0x3f) | 0x80;
        let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
        ProfileId(format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32]))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ProfileId {
    type Error = String;
    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        ProfileId::parse(&value).map_err(|e| e.to_string())
    }
}

impl From<ProfileId> for String {
    fn from(id: ProfileId) -> String {
        id.0
    }
}

impl fmt::Display for ProfileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for ProfileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProfileId({})", self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProfileMetadata {
    pub id: ProfileId,
    pub identity: AccountIdentity,
    pub created_at: u64,
    pub updated_at: u64,
}

impl ProfileMetadata {
    /// Primary menu line: the email, or a short account id when no email is known.
    pub fn title(&self) -> String {
        match (&self.identity.email, &self.identity.account_id) {
            (Some(email), _) => email.clone(),
            (None, Some(acct)) => format!("Account {}", acct.chars().take(8).collect::<String>()),
            (None, None) => format!("Profile {}", &self.id.as_str()[..8]),
        }
    }

    /// Secondary line, e.g. "Personal / Pro" or "Business".
    pub fn plan_label(&self) -> String {
        plan_label(self.identity.plan_type.as_deref())
    }
}

/// A profile as persisted: metadata and the encrypted credential blob.
pub struct StoredProfile {
    pub metadata: ProfileMetadata,
    pub encrypted_credentials: Vec<u8>,
}

/// Maps Codex `chatgpt_plan_type` values to a human label.
pub fn plan_label(plan: Option<&str>) -> String {
    let Some(plan) = plan else {
        return "Unknown plan".into();
    };
    match plan.to_ascii_lowercase().as_str() {
        "free" => "Personal / Free".into(),
        "go" => "Personal / Go".into(),
        "plus" => "Personal / Plus".into(),
        "pro" => "Personal / Pro".into(),
        "team" | "business" => "Business".into(),
        "enterprise" => "Enterprise".into(),
        "edu" | "education" => "Edu".into(),
        other => {
            let mut c = other.chars();
            match c.next() {
                Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
                None => "Unknown plan".into(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_id_is_valid_v4() {
        let id = ProfileId::from_random_bytes([0xff; 16]);
        assert_eq!(id.as_str().len(), 36);
        assert_eq!(&id.as_str()[14..15], "4");
        assert!(ProfileId::parse(id.as_str()).is_ok());
    }

    #[test]
    fn rejects_path_traversal_ids() {
        for bad in ["../../../../etc/passwd", "..\\..\\x", "", "a".repeat(36).as_str(), "12345678-1234-1234-1234-12345678901/"] {
            assert!(ProfileId::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn deserialization_validates_id() {
        let r: std::result::Result<ProfileId, _> = serde_json::from_str("\"..\\\\evil\"");
        assert!(r.is_err());
    }

    #[test]
    fn plan_labels() {
        assert_eq!(plan_label(Some("pro")), "Personal / Pro");
        assert_eq!(plan_label(Some("business")), "Business");
        assert_eq!(plan_label(Some("team")), "Business");
        assert_eq!(plan_label(Some("mystery")), "Mystery");
        assert_eq!(plan_label(None), "Unknown plan");
    }
}
