//! Ambient inputs (randomness and wall-clock time) behind a seam so tests are deterministic.

pub trait Ambient: Send + Sync {
    fn random_bytes(&self, buf: &mut [u8]);
    fn now_unix_secs(&self) -> u64;
}
