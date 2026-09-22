use std::fmt;

/// Plaintext credential bytes.
///
/// * No `Clone`, `Display` or `Serialize`: copies and accidental logging are compile errors.
/// * `Debug` is redacted.
/// * The buffer is zeroed on drop.
pub struct SecretBytes(Vec<u8>);

impl SecretBytes {
    pub fn new(bytes: Vec<u8>) -> Self {
        SecretBytes(bytes)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Length-independent-time comparison of the contents.
    pub fn ct_eq(&self, other: &[u8]) -> bool {
        if self.0.len() != other.len() {
            return false;
        }
        self.0.iter().zip(other).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
    }
}

impl Drop for SecretBytes {
    fn drop(&mut self) {
        scrub(&mut self.0);
    }
}

impl fmt::Debug for SecretBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SecretBytes(<redacted {} bytes>)", self.0.len())
    }
}

/// Overwrites a buffer with zeros in a way the optimizer may not elide.
pub fn scrub(buf: &mut [u8]) {
    for b in buf.iter_mut() {
        // SAFETY: `b` is a valid, exclusive reference into the slice.
        unsafe { std::ptr::write_volatile(b, 0) };
    }
    std::hint::black_box(&*buf);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_is_redacted() {
        let s = SecretBytes::new(b"refresh_token_value".to_vec());
        let dbg = format!("{s:?}");
        assert!(!dbg.contains("refresh"));
        assert!(dbg.contains("19 bytes"));
    }

    #[test]
    fn ct_eq_compares_content() {
        let s = SecretBytes::new(b"abc".to_vec());
        assert!(s.ct_eq(b"abc"));
        assert!(!s.ct_eq(b"abd"));
        assert!(!s.ct_eq(b"ab"));
    }
}
