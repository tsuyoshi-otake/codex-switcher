use std::sync::Arc;

use super::model::{ProfileId, ProfileMetadata, StoredProfile};
use super::repository::{ProfileEntry, ProfileRepository};
use crate::ambient::Ambient;
use crate::auth::{parse_identity, AccountIdentity, SecretBytes, SecretProtector};
use crate::error::{Error, Result};

/// Encrypted credential storage keyed by account identity.
pub struct ProfileVault {
    repo: ProfileRepository,
    protector: Arc<dyn SecretProtector>,
    ambient: Arc<dyn Ambient>,
}

impl ProfileVault {
    pub fn new(repo: ProfileRepository, protector: Arc<dyn SecretProtector>, ambient: Arc<dyn Ambient>) -> Self {
        ProfileVault { repo, protector, ambient }
    }

    pub fn list(&self) -> Result<Vec<ProfileEntry>> {
        self.repo.list()
    }

    pub fn metadata(&self, id: &ProfileId) -> Result<ProfileMetadata> {
        Ok(self.repo.load(id)?.metadata)
    }

    pub fn find_by_identity(&self, identity: &AccountIdentity) -> Result<Option<ProfileMetadata>> {
        Ok(self.repo.list()?.into_iter().find_map(|e| match e {
            ProfileEntry::Valid(p) if p.metadata.identity.same_account(identity) => Some(p.metadata),
            _ => None,
        }))
    }

    /// Stores the given login as the latest credentials of the matching profile, creating
    /// a new profile when the account is not registered. Returns `(metadata, created)`.
    pub fn upsert(&self, credentials: &SecretBytes) -> Result<(ProfileMetadata, bool)> {
        let identity = parse_identity(credentials.as_bytes())?;
        let now = self.ambient.now_unix_secs();
        let (metadata, created) = match self.find_by_identity(&identity)? {
            Some(mut existing) => {
                existing.identity = identity;
                existing.updated_at = now;
                (existing, false)
            }
            None => {
                let mut rnd = [0u8; 16];
                self.ambient.random_bytes(&mut rnd);
                (ProfileMetadata { id: ProfileId::from_random_bytes(rnd), identity, created_at: now, updated_at: now }, true)
            }
        };
        let encrypted_credentials = self.protector.protect(credentials)?;
        self.repo.save(&StoredProfile { metadata: metadata.clone(), encrypted_credentials })?;
        Ok((metadata, created))
    }

    /// Decrypts a profile and checks that the blob still belongs to the recorded account.
    pub fn open(&self, id: &ProfileId) -> Result<(ProfileMetadata, SecretBytes)> {
        let stored = self.repo.load(id)?;
        let secret = self.protector.unprotect(&stored.encrypted_credentials)?;
        let identity = parse_identity(secret.as_bytes()).map_err(|_| Error::ProfileCorrupted {
            id: id.to_string(),
            reason: "decrypted credentials are not a ChatGPT login".into(),
        })?;
        if !identity.same_account(&stored.metadata.identity) {
            return Err(Error::ProfileCorrupted { id: id.to_string(), reason: "credentials belong to a different account".into() });
        }
        Ok((stored.metadata, secret))
    }

    pub fn delete(&self, id: &ProfileId) -> Result<()> {
        self.repo.delete(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::fixtures::auth_json;
    use crate::testing::{FakeProtector, FixedAmbient, MemFs};
    use std::path::PathBuf;

    fn vault() -> (Arc<FakeProtector>, ProfileVault) {
        let fs = Arc::new(MemFs::new());
        let protector = Arc::new(FakeProtector::default());
        let repo = ProfileRepository::new(fs, PathBuf::from("C:/s/profiles"));
        (protector.clone(), ProfileVault::new(repo, protector, Arc::new(FixedAmbient::default())))
    }

    #[test]
    fn same_email_personal_and_business_are_separate_profiles() {
        let (_, v) = vault();
        let (p1, c1) = v.upsert(&SecretBytes::new(auth_json("t@example.com", "acct-p", "u1", "pro", "r1"))).unwrap();
        let (p2, c2) = v.upsert(&SecretBytes::new(auth_json("t@example.com", "acct-b", "u1", "business", "r2"))).unwrap();
        assert!(c1 && c2);
        assert_ne!(p1.id, p2.id);
        assert_eq!(v.list().unwrap().len(), 2);
    }

    #[test]
    fn upsert_updates_rotated_credentials_in_place() {
        let (_, v) = vault();
        let (p1, _) = v.upsert(&SecretBytes::new(auth_json("a@example.com", "acct", "u", "plus", "old"))).unwrap();
        let newer = auth_json("a@example.com", "acct", "u", "pro", "new");
        let (p2, created) = v.upsert(&SecretBytes::new(newer.clone())).unwrap();
        assert!(!created);
        assert_eq!(p1.id, p2.id);
        assert_eq!(p2.identity.plan_type.as_deref(), Some("pro"));
        let (_, secret) = v.open(&p1.id).unwrap();
        assert!(secret.ct_eq(&newer));
    }

    #[test]
    fn open_fails_when_decryption_fails() {
        let (protector, v) = vault();
        let (p, _) = v.upsert(&SecretBytes::new(auth_json("a@example.com", "acct", "u", "pro", "r"))).unwrap();
        protector.fail_unprotect(true);
        assert!(matches!(v.open(&p.id), Err(Error::Protect(_))));
    }

    #[test]
    fn rejects_non_chatgpt_credentials() {
        let (_, v) = vault();
        assert!(v.upsert(&SecretBytes::new(b"{}".to_vec())).is_err());
        assert!(v.list().unwrap().is_empty());
    }
}
