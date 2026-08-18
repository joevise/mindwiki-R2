//! 密钥闸门：所有解密必须过闸。一键关闭 = 密钥 zeroize + 会话终止。
//! 内核：besure::crypto::VaultCrypto（Argon2id 派生 + AES-256-GCM）。

use anyhow::{anyhow, bail, Result};
use besure::crypto::VaultCrypto;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GatewayState {
    Open,
    Closed,
}

/// 会话注册表：层间解耦（mw-store 依赖此 trait，不依赖 KeyGateway 具体类型）
pub trait SessionRegistry: Send + Sync {
    fn register(&self, id: &str, work_dir: &Path, flag: Arc<AtomicBool>);
    fn unregister(&self, id: &str);
}

/// 活跃会话句柄：强制终止旗标由 DecryptedSession 持有
pub struct SessionHandle {
    pub work_dir: PathBuf,
    pub terminate: Arc<AtomicBool>,
}

pub struct KeyGateway {
    crypto: Mutex<Option<VaultCrypto>>,
    salt: Vec<u8>,
    verify_token: Mutex<Option<Vec<u8>>>,
    /// 活跃会话注册表：session_id → 强制终止句柄
    sessions: Mutex<HashMap<String, SessionHandle>>,
    /// 审计：何时关闭
    pub closed_at: Mutex<Option<Instant>>,
}

impl KeyGateway {
    /// 新建（init 用）：随机 salt，未解锁
    pub fn new() -> Result<Self> {
        let crypto = VaultCrypto::new().map_err(|e| anyhow!(e.to_string()))?;
        Ok(Self {
            salt: crypto.salt().to_vec(),
            crypto: Mutex::new(Some(crypto)),
            verify_token: Mutex::new(None),
            sessions: Mutex::new(HashMap::new()),
            closed_at: Mutex::new(None),
        })
    }

    /// 从已有容器加载（open_session 用）：salt + 密码验证令牌
    pub fn from_container(salt: Vec<u8>, verify_token: Vec<u8>) -> Self {
        Self {
            crypto: Mutex::new(Some(VaultCrypto::from_salt(salt.clone()))),
            salt,
            verify_token: Mutex::new(Some(verify_token)),
            sessions: Mutex::new(HashMap::new()),
            closed_at: Mutex::new(None),
        }
    }

    pub fn salt(&self) -> &[u8] {
        &self.salt
    }

    pub fn state(&self) -> GatewayState {
        let slot = self.crypto.lock().unwrap();
        match slot.as_ref() {
            Some(c) if c.is_unlocked() => GatewayState::Open,
            _ => GatewayState::Closed,
        }
    }

    /// 开启闸门：Argon2id 派生密钥；有验证令牌时先验密码
    pub fn open(&self, password: &str) -> Result<()> {
        let token = self.verify_token.lock().unwrap().clone();
        let mut slot = self.crypto.lock().unwrap();
        let crypto = slot
            .as_mut()
            .ok_or_else(|| anyhow!("key gateway destroyed — create a new gateway"))?;
        match token {
            Some(t) => {
                let ok = crypto
                    .unlock_with_verify(password, &t)
                    .map_err(|e| anyhow!(e.to_string()))?;
                if ok {
                    Ok(())
                } else {
                    bail!("wrong password")
                }
            }
            None => {
                crypto.unlock(password);
                Ok(())
            }
        }
    }

    /// 取密码验证令牌（init 时生成，调用方负责持久化进容器）
    pub fn ensure_verify_token(&self) -> Result<Vec<u8>> {
        let mut token_slot = self.verify_token.lock().unwrap();
        if let Some(t) = token_slot.as_ref() {
            return Ok(t.clone());
        }
        let slot = self.crypto.lock().unwrap();
        let crypto = slot.as_ref().ok_or_else(|| anyhow!("key gateway closed"))?;
        let t = crypto
            .generate_verify_token()
            .map_err(|e| anyhow!(e.to_string()))?;
        *token_slot = Some(t.clone());
        Ok(t)
    }

    /// 一键关闭：密钥 zeroize + 终止全部活跃会话 + 记录审计时间。
    /// 被终止会话的 Drop 不做 seal、直接销毁，此后一切密文为噪声。
    pub fn close(&self) {
        // 密钥 zeroize 即达成安全语义；保留 VaultCrypto 对象（已锁定态），
        // 同一闸门可凭主密码重新 open（网页 UI 锁定→再解锁的真实路径）。
        {
            let mut slot = self.crypto.lock().unwrap();
            if let Some(c) = slot.as_mut() {
                c.lock();
            }
        }
        let mut sessions = self.sessions.lock().unwrap();
        for handle in sessions.values() {
            handle.terminate.store(true, Ordering::SeqCst);
        }
        sessions.clear();
        *self.closed_at.lock().unwrap() = Some(Instant::now());
    }

    /// 当前活跃会话数（监控 / state 端点用）
    pub fn active_sessions(&self) -> usize {
        self.sessions.lock().unwrap().len()
    }

    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        self.guard()?;
        let slot = self.crypto.lock().unwrap();
        slot.as_ref()
            .unwrap()
            .encrypt(plaintext)
            .map_err(|e| anyhow!(e.to_string()))
    }

    pub fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        self.guard()?;
        let slot = self.crypto.lock().unwrap();
        slot.as_ref()
            .unwrap()
            .decrypt(ciphertext)
            .map_err(|e| anyhow!(e.to_string()))
    }

    /// 解密前置检查
    pub fn guard(&self) -> Result<()> {
        if self.state() == GatewayState::Closed {
            bail!("key gateway closed — all ciphertext is inert");
        }
        Ok(())
    }
}

impl SessionRegistry for KeyGateway {
    fn register(&self, id: &str, work_dir: &Path, flag: Arc<AtomicBool>) {
        self.sessions.lock().unwrap().insert(
            id.to_string(),
            SessionHandle {
                work_dir: work_dir.to_path_buf(),
                terminate: flag,
            },
        );
    }

    fn unregister(&self, id: &str) {
        self.sessions.lock().unwrap().remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_close_blocks_decrypt() {
        let gw = KeyGateway::new().unwrap();
        gw.open("pw").unwrap();
        assert!(gw.guard().is_ok());
        let ct = gw.encrypt(b"hello").unwrap();
        assert_eq!(gw.decrypt(&ct).unwrap(), b"hello");
        gw.close();
        assert!(gw.guard().is_err());
        assert!(gw.decrypt(&ct).is_err());
        assert_eq!(gw.state(), GatewayState::Closed);
    }

    #[test]
    fn wrong_password_rejected_by_gateway() {
        let gw = KeyGateway::new().unwrap();
        gw.open("pw-A").unwrap();
        let token = gw.ensure_verify_token().unwrap();
        let salt = gw.salt().to_vec();
        gw.close();

        let gw2 = KeyGateway::from_container(salt.clone(), token.clone());
        assert!(gw2.open("pw-B").is_err());
        assert_eq!(gw2.state(), GatewayState::Closed);

        let gw3 = KeyGateway::from_container(salt, token);
        gw3.open("pw-A").unwrap();
        assert_eq!(gw3.state(), GatewayState::Open);
    }
}
