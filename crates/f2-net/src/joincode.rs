//! Short join codes: the host's address, port and session nonce in Crockford base32.
//!
//! Layout (before base32): `ip (4 or 16 bytes) | port u16 BE | nonce u16 BE | crc8`.
//! IPv4 codes are 15 characters, shown as `XXXXX-XXXXX-XXXXX`; IPv6 codes are 34 characters.
//! Decoding is case-insensitive, ignores `-`/spaces and maps `I`/`L` → 1, `O` → 0.
//! No server is involved: the code *is* the address.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JoinCodeError {
    BadLength,
    BadCharacter(char),
    BadChecksum,
}

impl std::fmt::Display for JoinCodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JoinCodeError::BadLength => write!(f, "join code has the wrong length"),
            JoinCodeError::BadCharacter(c) => write!(f, "invalid character {c:?} in join code"),
            JoinCodeError::BadChecksum => write!(f, "join code checksum mismatch (typo?)"),
        }
    }
}

impl std::error::Error for JoinCodeError {}

fn crc8(data: &[u8]) -> u8 {
    // CRC-8/ATM (poly 0x07).
    let mut c = 0u8;
    for &b in data {
        c ^= b;
        for _ in 0..8 {
            c = if c & 0x80 != 0 { (c << 1) ^ 0x07 } else { c << 1 };
        }
    }
    c
}

fn to_base32(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut acc: u32 = 0;
    let mut bits = 0;
    for &b in bytes {
        acc = (acc << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((acc >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((acc << (5 - bits)) & 31) as usize] as char);
    }
    out
}

fn from_base32(s: &[u8], nbytes: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(nbytes);
    let mut acc: u32 = 0;
    let mut bits = 0;
    for &v in s {
        acc = (acc << 5) | v as u32;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out.truncate(nbytes);
    out
}

/// Encode the host address and session nonce.
pub fn encode_join_code(addr: SocketAddr, nonce: u16) -> String {
    let mut b = match addr.ip() {
        IpAddr::V4(ip) => ip.octets().to_vec(),
        IpAddr::V6(ip) => ip.octets().to_vec(),
    };
    b.extend_from_slice(&addr.port().to_be_bytes());
    b.extend_from_slice(&nonce.to_be_bytes());
    b.push(crc8(&b));
    let raw = to_base32(&b);
    raw.as_bytes().chunks(5).map(|c| std::str::from_utf8(c).unwrap()).collect::<Vec<_>>().join("-")
}

/// Decode a join code into `(host address, session nonce)`.
pub fn decode_join_code(code: &str) -> Result<(SocketAddr, u16), JoinCodeError> {
    let mut vals = Vec::new();
    for c in code.chars() {
        if c == '-' || c.is_whitespace() {
            continue;
        }
        let u = match c.to_ascii_uppercase() {
            'I' | 'L' => '1',
            'O' => '0',
            u => u,
        };
        let v = ALPHABET.iter().position(|&a| a as char == u).ok_or(JoinCodeError::BadCharacter(c))?;
        vals.push(v as u8);
    }
    let nbytes = match vals.len() {
        15 => 9,
        34 => 21,
        _ => return Err(JoinCodeError::BadLength),
    };
    let b = from_base32(&vals, nbytes);
    let (body, crc) = b.split_at(nbytes - 1);
    if crc8(body) != crc[0] {
        return Err(JoinCodeError::BadChecksum);
    }
    let iplen = nbytes - 5;
    let ip = if iplen == 4 {
        IpAddr::V4(Ipv4Addr::from(<[u8; 4]>::try_from(&body[..4]).unwrap()))
    } else {
        IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(&body[..16]).unwrap()))
    };
    let port = u16::from_be_bytes([body[iplen], body[iplen + 1]]);
    let nonce = u16::from_be_bytes([body[iplen + 2], body[iplen + 3]]);
    Ok((SocketAddr::new(ip, port), nonce))
}

/// A random non-zero session nonce (std only: hashes the time with a randomly keyed hasher).
pub fn random_nonce() -> u16 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut h = RandomState::new().build_hasher();
    if let Ok(d) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        h.write_u128(d.as_nanos());
    }
    let v = h.finish();
    ((v ^ (v >> 16) ^ (v >> 32) ^ (v >> 48)) as u16).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_v4_v6() {
        let a: SocketAddr = "203.0.113.7:7777".parse().unwrap();
        let code = encode_join_code(a, 0xBEEF);
        assert_eq!(code.len(), 17, "{code}");
        assert_eq!(decode_join_code(&code), Ok((a, 0xBEEF)));
        assert_eq!(decode_join_code(&code.to_lowercase().replace('-', " ")), Ok((a, 0xBEEF)));

        let b: SocketAddr = "[2001:db8::1]:65535".parse().unwrap();
        let code = encode_join_code(b, 1);
        assert_eq!(decode_join_code(&code), Ok((b, 1)));
    }

    #[test]
    fn detects_typos() {
        let a: SocketAddr = "10.0.0.2:5000".parse().unwrap();
        let code = encode_join_code(a, 42);
        let mut chars: Vec<char> = code.chars().collect();
        chars[3] = if chars[3] == 'Z' { 'Y' } else { 'Z' };
        let typo: String = chars.into_iter().collect();
        assert_eq!(decode_join_code(&typo), Err(JoinCodeError::BadChecksum));
        assert_eq!(decode_join_code("ABC"), Err(JoinCodeError::BadLength));
        assert!(matches!(decode_join_code("UUUUU-UUUUU-UUUUU"), Err(JoinCodeError::BadCharacter('U'))));
    }

    #[test]
    fn nonce_nonzero() {
        for _ in 0..100 {
            assert_ne!(random_nonce(), 0);
        }
    }
}
