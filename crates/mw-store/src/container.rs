//! 加密容器：追加式块结构（历史只读块 + 增量块）。
//! 布局：MAGIC("MWVAULT1") + VERSION(1B) + SALT(16B) + CHUNKS...

use anyhow::{bail, Result};

pub const MAGIC: &[u8; 8] = b"MWVAULT1";
pub const VERSION: u8 = 0x01;

/// 容器头部：MAGIC + VERSION + SALT
pub fn header(salt: &[u8]) -> Vec<u8> {
    let mut h = Vec::with_capacity(8 + 1 + 16);
    h.extend_from_slice(MAGIC);
    h.push(VERSION);
    h.extend_from_slice(&salt[..16.min(salt.len())]);
    h
}

/// 校验容器头
pub fn validate(data: &[u8]) -> Result<(u8, &[u8])> {
    if data.len() < 25 || &data[..8] != MAGIC {
        bail!("not a mindwiki vault container");
    }
    Ok((data[8], &data[9..25]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_roundtrip() {
        let salt = [7u8; 16];
        let h = header(&salt);
        let (v, s) = validate(&h).unwrap();
        assert_eq!(v, VERSION);
        assert_eq!(s, &salt[..]);
    }

    #[test]
    fn rejects_garbage() {
        assert!(validate(b"garbage").is_err());
    }
}
