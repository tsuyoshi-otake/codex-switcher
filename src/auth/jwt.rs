//! Minimal JWT payload extraction. Signatures are NOT verified: the token is only used
//! locally to label a profile, never to make an authorization decision.

/// Returns the decoded payload segment of a compact JWS (`header.payload.signature`).
pub fn decode_payload(token: &str) -> Option<Vec<u8>> {
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    parts.next()?; // signature must exist
    base64url_decode(payload)
}

/// Decodes unpadded (or padded) base64url.
pub fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    let trimmed = input.trim_end_matches('=');
    let mut out = Vec::with_capacity(trimmed.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for c in trimmed.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    // A single leftover sextet cannot encode a byte.
    if bits >= 6 {
        return None;
    }
    Some(out)
}

#[cfg(test)]
pub(crate) fn base64url_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |acc, (i, b)| acc | (*b as u32) << (16 - 8 * i));
        let chars = chunk.len() + 1;
        for i in 0..chars {
            out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn decodes_known_vector() {
        assert_eq!(base64url_decode("eyJhIjoxfQ").unwrap(), br#"{"a":1}"#);
    }

    #[test]
    fn rejects_malformed_token() {
        assert!(decode_payload("only.two").is_none());
        assert!(decode_payload("a.!!!.c").is_none());
    }

    proptest! {
        #[test]
        fn roundtrip(data in proptest::collection::vec(any::<u8>(), 0..256)) {
            prop_assert_eq!(base64url_decode(&base64url_encode(&data)).unwrap(), data);
        }
    }
}
