//! Vault：一个知识库 = 一个加密容器 + 解密会话管理。

use anyhow::Result;
use std::path::{Path, PathBuf};

/// 磁盘上的 vault（永远密文）
pub struct Vault {
    pub root: PathBuf,
}

/// 解密会话（受控临时目录，drop 时逐文件销毁）
pub struct DecryptedSession {
    pub work_dir: PathBuf,
}

impl Drop for DecryptedSession {
    fn drop(&mut self) {
        // TODO Step2: shred 每个文件后删除目录（用 zeroize + 多轮覆写）
        let _ = std::fs::remove_dir_all(&self.work_dir);
    }
}

impl Vault {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        Ok(Self { root: root.as_ref().to_path_buf() })
    }

    pub fn container_path(&self) -> PathBuf {
        self.root.join("vault.mwenc")
    }

    pub fn exists(&self) -> bool {
        self.container_path().exists()
    }
}
