//! 加密容器：MAGIC + VERSION + SALT + 验证令牌 + 单个加密快照块。
//! 历史版本由块内 .git 管理，容器层只存最新快照（原子重写）。
//!
//! 布局：
//! ```text
//! [MAGIC 8B "MWVAULT1"][VERSION 1B][SALT 16B]
//! [VERIFY_LEN 4B LE][VERIFY_TOKEN]
//! [PAYLOAD_LEN 8B LE][PAYLOAD = AES-256-GCM(tar.gz(wiki + .git))]
//! ```

use anyhow::{bail, Result};

pub const MAGIC: &[u8; 8] = b"MWVAULT1";
pub const VERSION: u8 = 0x01;
pub const SALT_LEN: usize = 16;
const HEADER_LEN: usize = 8 + 1 + SALT_LEN;

#[derive(Debug)]
pub struct ContainerData {
    pub version: u8,
    pub salt: Vec<u8>,
    pub verify_token: Vec<u8>,
    pub payload: Vec<u8>,
}

/// 编码容器
pub fn encode(salt: &[u8], verify_token: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + 4 + verify_token.len() + 8 + payload.len());
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.extend_from_slice(&salt[..SALT_LEN.min(salt.len())]);
    out.extend_from_slice(&(verify_token.len() as u32).to_le_bytes());
    out.extend_from_slice(verify_token);
    out.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// 解析容器
pub fn decode(data: &[u8]) -> Result<ContainerData> {
    if data.len() < HEADER_LEN + 4 || &data[..8] != MAGIC {
        bail!("not a mindwiki vault container");
    }
    let version = data[8];
    if version != VERSION {
        bail!("unsupported vault version: {version}");
    }
    let salt = data[9..HEADER_LEN].to_vec();
    let vlen = u32::from_le_bytes(data[HEADER_LEN..HEADER_LEN + 4].try_into().unwrap()) as usize;
    let vstart = HEADER_LEN + 4;
    if data.len() < vstart + vlen + 8 {
        bail!("truncated vault container");
    }
    let verify_token = data[vstart..vstart + vlen].to_vec();
    let pstart = vstart + vlen;
    let plen = u64::from_le_bytes(data[pstart..pstart + 8].try_into().unwrap()) as usize;
    if data.len() < pstart + 8 + plen {
        bail!("truncated vault payload");
    }
    let payload = data[pstart + 8..pstart + 8 + plen].to_vec();
    Ok(ContainerData { version, salt, verify_token, payload })
}

/// 校验容器头（仅头部，不取 payload）
pub fn validate(data: &[u8]) -> Result<(u8, &[u8])> {
    if data.len() < HEADER_LEN || &data[..8] != MAGIC {
        bail!("not a mindwiki vault container");
    }
    Ok((data[8], &data[9..HEADER_LEN]))
}

/// 容器头部字节（MAGIC + VERSION + SALT）
pub fn header(salt: &[u8]) -> Vec<u8> {
    let mut h = Vec::with_capacity(HEADER_LEN);
    h.extend_from_slice(MAGIC);
    h.push(VERSION);
    h.extend_from_slice(&salt[..SALT_LEN.min(salt.len())]);
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_roundtrip() {
        let salt = [7u8; SALT_LEN];
        let h = header(&salt);
        let (v, s) = validate(&h).unwrap();
        assert_eq!(v, VERSION);
        assert_eq!(s, &salt[..]);
    }

    #[test]
    fn rejects_garbage() {
        assert!(validate(b"garbage").is_err());
        assert!(decode(b"garbage").is_err());
    }

    #[test]
    fn encode_decode_roundtrip() {
        let salt = [3u8; SALT_LEN];
        let token = b"verify-token";
        let payload = b"encrypted-payload";
        let data = encode(&salt, token, payload);
        let c = decode(&data).unwrap();
        assert_eq!(c.version, VERSION);
        assert_eq!(c.salt, salt);
        assert_eq!(c.verify_token, token);
        assert_eq!(c.payload, payload);
    }
}
