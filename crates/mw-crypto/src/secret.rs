//! SecretStore trait —— 层间依赖倒置的锚点。
//! L2 知识库层只依赖此接口，不知道背后实现（besure 内核 / 未来算法）。

pub trait SecretStore: Send + Sync {
    /// 解密 vault 到受控会话（返回临时工作目录路径）
    fn decrypt_to_session(&self, vault_id: &str) -> anyhow::Result<std::path::PathBuf>;
    /// 会话结束后，把增量封回容器
    fn seal_increment(&self, vault_id: &str, work_dir: &std::path::Path) -> anyhow::Result<()>;
}
