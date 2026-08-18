//! 密钥闸门：所有解密必须过闸。一键关闭 = 密钥 zeroize + 会话终止。

use anyhow::{bail, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use zeroize::Zeroizing;

/// 派生密钥（Zeroizing：drop 时自动清零内存）
pub struct SessionKey(pub Zeroizing<Vec<u8>>);

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GatewayState {
    Open,
    Closed,
}

pub struct KeyGateway {
    state: AtomicBool, // true = open
    // TODO Step2: 接 besure::VaultCrypto 做 Argon2id 派生 + AES-256-GCM
    _key_slot: Mutex<Option<SessionKey>>,
}

impl KeyGateway {
    pub fn new() -> Self {
        Self { state: AtomicBool::new(false), _key_slot: Mutex::new(None) }
    }

    pub fn state(&self) -> GatewayState {
        if self.state.load(Ordering::SeqCst) { GatewayState::Open } else { GatewayState::Closed }
    }

    /// 客户开启闸门（本地密码 / 远程授权都汇到这）
    pub fn open(&self, _password: &str) -> Result<()> {
        self.state.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// 一键关闭：密钥清零、状态闭锁。此后所有解密请求被拒绝。
    pub fn close(&self) {
        let mut slot = self._key_slot.lock().unwrap();
        *slot = None; // Zeroizing drop 自动清零
        self.state.store(false, Ordering::SeqCst);
        // TODO Step3: 终止所有活跃 DecryptedSession
    }

    /// 解密前置检查
    pub fn guard(&self) -> Result<()> {
        if self.state() == GatewayState::Closed {
            bail!("key gateway closed — all ciphertext is inert");
        }
        Ok(())
    }
}

impl Default for KeyGateway {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_close_blocks_decryption() {
        let gw = KeyGateway::new();
        gw.open("pw").unwrap();
        assert!(gw.guard().is_ok());
        gw.close();
        assert!(gw.guard().is_err());
        assert_eq!(gw.state(), GatewayState::Closed);
    }
}
