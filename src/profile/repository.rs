//! Profile persistence: `<profiles_dir>/<uuid>.profile`, one JSON document per profile.
//!
//! ```json
//! { "format": 1, "metadata": {...}, "protection": "dpapi-current-user",
//!   "ciphertext_fnv1a64": "…", "ciphertext_hex": "…" }
//! ```
//! Only ciphertext is stored. The FNV checksum detects accidental corruption early; the
//! DPAPI MAC remains the real integrity check on decrypt.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::model::{ProfileId, ProfileMetadata, StoredProfile};
use crate::error::{Error, Result};
use crate::fsio::FileStore;

const EXTENSION: &str = ".profile";
const FORMAT_VERSION: u32 = 1;
const PROTECTION: &str = "dpapi-current-user";

pub enum ProfileEntry {
    Valid(StoredProfile),
    Corrupted { id: ProfileId, reason: String },
}

#[derive(Serialize, Deserialize)]
struct ProfileFile {
    format: u32,
    metadata: ProfileMetadata,
    protection: String,
    ciphertext_fnv1a64: String,
    ciphertext_hex: String,
}

pub struct ProfileRepository {
    fs: Arc<dyn FileStore>,
    dir: PathBuf,
}

impl ProfileRepository {
    pub fn new(fs: Arc<dyn FileStore>, dir: PathBuf) -> Self {
        ProfileRepository { fs, dir }
    }

    fn path_for(&self, id: &ProfileId) -> PathBuf {
        self.dir.join(format!("{}{}", id.as_str(), EXTENSION))
    }

    /// All profiles, oldest first. Unreadable files are reported, not skipped silently.
    pub fn list(&self) -> Result<Vec<ProfileEntry>> {
        let names = self.fs.list(&self.dir).map_err(|e| Error::io("listing profiles", e))?;
        let mut entries = Vec::new();
        for name in names {
            let Some(stem) = name.strip_suffix(EXTENSION) else { continue };
            let Ok(id) = ProfileId::parse(stem) else { continue };
            // Non-canonical (uppercase) names are ignored so each id maps to exactly one file.
            if id.as_str() != stem {
                continue;
            }
            match self.load(&id) {
                Ok(p) => entries.push(ProfileEntry::Valid(p)),
                Err(Error::ProfileCorrupted { reason, .. }) => entries.push(ProfileEntry::Corrupted { id, reason }),
                Err(Error::ProfileNotFound(_)) => {}
                Err(e) => return Err(e),
            }
        }
        entries.sort_by_key(|e| match e {
            ProfileEntry::Valid(p) => (0, p.metadata.created_at, p.metadata.id.clone()),
            ProfileEntry::Corrupted { id, .. } => (1, 0, id.clone()),
        });
        Ok(entries)
    }

    pub fn load(&self, id: &ProfileId) -> Result<StoredProfile> {
        let corrupted = |reason: &str| Error::ProfileCorrupted { id: id.to_string(), reason: reason.into() };
        let bytes = self
            .fs
            .read(&self.path_for(id))
            .map_err(|e| Error::io(format!("reading profile {id}"), e))?
            .ok_or_else(|| Error::ProfileNotFound(id.to_string()))?;
        let file: ProfileFile = serde_json::from_slice(&bytes).map_err(|_| corrupted("not a valid profile document"))?;
        if file.format != FORMAT_VERSION {
            return Err(corrupted("unsupported profile format version"));
        }
        if file.protection != PROTECTION {
            return Err(corrupted("unknown protection scheme"));
        }
        if &file.metadata.id != id {
            return Err(corrupted("profile id does not match file name"));
        }
        let ciphertext = hex_decode(&file.ciphertext_hex).ok_or_else(|| corrupted("credential blob is not hex"))?;
        if ciphertext.is_empty() {
            return Err(corrupted("credential blob is empty"));
        }
        if format!("{:016x}", fnv1a64(&ciphertext)) != file.ciphertext_fnv1a64 {
            return Err(corrupted("credential blob checksum mismatch"));
        }
        Ok(StoredProfile { metadata: file.metadata, encrypted_credentials: ciphertext })
    }

    pub fn save(&self, profile: &StoredProfile) -> Result<()> {
        let file = ProfileFile {
            format: FORMAT_VERSION,
            metadata: profile.metadata.clone(),
            protection: PROTECTION.into(),
            ciphertext_fnv1a64: format!("{:016x}", fnv1a64(&profile.encrypted_credentials)),
            ciphertext_hex: hex_encode(&profile.encrypted_credentials),
        };
        let json = serde_json::to_vec_pretty(&file).map_err(|e| Error::Config(e.to_string()))?;
        self.fs.create_dir_all(&self.dir).map_err(|e| Error::io("creating profile directory", e))?;
        self.fs
            .write_atomic(&self.path_for(&profile.metadata.id), &json)
            .map_err(|e| Error::io(format!("writing profile {}", profile.metadata.id), e))
    }

    pub fn delete(&self, id: &ProfileId) -> Result<()> {
        self.fs.remove(&self.path_for(id)).map_err(|e| Error::io(format!("deleting profile {id}"), e))
    }
}

fn fnv1a64(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x0000_0100_0000_01b3))
}

fn hex_encode(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::auth::AccountIdentity;
    use crate::testing::MemFs;

    fn profile(n: u8, created: u64) -> StoredProfile {
        StoredProfile {
            metadata: ProfileMetadata {
                id: ProfileId::from_random_bytes([n; 16]),
                identity: AccountIdentity {
                    email: Some(format!("u{n}@example.com")),
                    account_id: Some(format!("a{n}")),
                    ..Default::default()
                },
                created_at: created,
                updated_at: created,
            },
            encrypted_credentials: vec![n, 1, 2, 3],
        }
    }

    fn repo() -> (Arc<MemFs>, ProfileRepository) {
        let fs = Arc::new(MemFs::new());
        (fs.clone(), ProfileRepository::new(fs, PathBuf::from("C:/state/profiles")))
    }

    #[test]
    fn save_load_roundtrip_and_ordering() {
        let (_, repo) = repo();
        repo.save(&profile(2, 20)).unwrap();
        repo.save(&profile(1, 10)).unwrap();
        let list = repo.list().unwrap();
        let ids: Vec<u64> = list
            .iter()
            .map(|e| match e {
                ProfileEntry::Valid(p) => p.metadata.created_at,
                _ => panic!("corrupted"),
            })
            .collect();
        assert_eq!(ids, vec![10, 20]);
        let p = profile(1, 10);
        assert_eq!(repo.load(&p.metadata.id).unwrap().encrypted_credentials, vec![1, 1, 2, 3]);
    }

    #[test]
    fn detects_corruption() {
        let (fs, repo) = repo();
        let p = profile(3, 1);
        repo.save(&p).unwrap();
        let path = repo.path_for(&p.metadata.id);
        let text = String::from_utf8(fs.get(&path).unwrap()).unwrap();
        fs.put(&path, text.replace("03010203", "03010204").as_bytes());
        assert!(matches!(repo.load(&p.metadata.id), Err(Error::ProfileCorrupted { .. })));
        fs.put(&path, b"{truncated");
        assert!(matches!(repo.list().unwrap()[0], ProfileEntry::Corrupted { .. }));
    }

    #[test]
    fn file_name_and_embedded_id_must_agree() {
        let (fs, repo) = repo();
        let a = profile(4, 1);
        repo.save(&a).unwrap();
        let other = ProfileId::from_random_bytes([9; 16]);
        let bytes = fs.get(&repo.path_for(&a.metadata.id)).unwrap();
        fs.put(&repo.path_for(&other), &bytes);
        assert!(matches!(repo.load(&other), Err(Error::ProfileCorrupted { .. })));
    }

    #[test]
    fn ignores_foreign_files() {
        let (fs, repo) = repo();
        fs.put(Path::new("C:/state/profiles/notes.txt"), b"x");
        fs.put(Path::new("C:/state/profiles/../evil.profile"), b"x");
        assert!(repo.list().unwrap().is_empty());
    }

    #[test]
    fn delete_and_missing() {
        let (_, repo) = repo();
        let p = profile(5, 1);
        repo.save(&p).unwrap();
        repo.delete(&p.metadata.id).unwrap();
        assert!(matches!(repo.load(&p.metadata.id), Err(Error::ProfileNotFound(_))));
        assert!(repo.list().unwrap().is_empty());
    }
}
