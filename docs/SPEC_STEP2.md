# SPEC: Step 2 — 加密容器 + Git 版本管理 + 内存解密

## 目标
Wiki 落盘全密文；解密只在受控临时目录（会话结束即焚）；git2 在解密环境内做版本管理；每次会话结束把变更封回容器（追加式增量块）。

## 改的文件
- `crates/mw-crypto/src/gateway.rs` — 接 besure::VaultCrypto（Argon2id + AES-256-GCM），KeyGateway 真实化
- `crates/mw-store/src/vault.rs` — Vault 真实实现：init / open_session / seal_session
- `crates/mw-store/src/container.rs` — 追加式容器读写（MAGIC + VERSION + SALT + 块区）
- `crates/mw-server/src/main.rs` — CLI 命令：mindwiki init / unlock（进入受控 REPL 占位）/ status
- `crates/mw-server/Cargo.toml` — 加 besure git 依赖

## 详细设计

### 1. 容器格式（追加式块）

```
vault.mwenc 布局：
[MAGIC 8B "MWVAULT1"][VERSION 1B][SALT 16B]
[CHUNK header: len(8B LE) + tag?]... 密文块序列

逻辑：
- 每次 seal_session 追加一个新块（整个 work_dir 的当前快照打包为 tar → AES-GCM 加密 → 追加）
- 解密 = 从头读所有块？不——简化 V1：
  - 块 0 = 完整快照（最新状态）
  - 历史版本 = Git 已经在块内管理（.git 目录就在快照里），不需要容器层多块历史
  - 所以容器 = header + 单个加密 tar 块（内含 wiki 文件 + .git 完整历史）
  - 每次 seal 重写整个块（原子：写临时文件 + rename）
- V1 就这么简单：容器单块 + Git 管历史。PPT 里的"增量块"叙事由 Git 的对象存储天然实现
  （没变的 blob 复用，新增只占增量）——对外话术依然成立
```

### 2. KeyGateway 真实化（mw-crypto）

```rust
pub struct KeyGateway {
    // besure 的 VaultCrypto：Argon2id 派生 + AES-256-GCM
    crypto: Mutex<Option<besure::crypto::VaultCrypto>>,
    salt: Vec<u8>, // 从容器头读
}

impl KeyGateway {
    pub fn open(&self, password: &str) -> Result<()>;   // unlock_with_verify
    pub fn close(&self);                                 // crypto 置 None（zeroize 由 besure 内部处理）
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>>;
    pub fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>>;
    pub fn guard(&self) -> Result<()>;                   // 已有
}
```

注：besure crate 名是 `besure`（package = "besure"），路径 `besure::crypto::VaultCrypto`。

### 3. Vault 真实实现（mw-store）

```rust
impl Vault {
    /// 初始化：创建 vault.mwenc（空 tar 加密）
    pub fn init(&self, gateway: &KeyGateway, password: &str) -> Result<()>;
    
    /// 打开解密会话：解密容器 → 展开 tar 到受控临时目录 → 返回 DecryptedSession
    /// work_dir 里可以做任何 wiki 操作（Agent、git commit 等）
    pub fn open_session(&self, gateway: &KeyGateway) -> Result<DecryptedSession>;
    
    /// 封印：tar 打包 work_dir（含 .git）→ 加密 → 原子重写容器
    pub fn seal_session(&self, gateway: &KeyGateway, session: &DecryptedSession) -> Result<()>;
}

impl DecryptedSession {
    pub fn work_dir(&self) -> &Path;
    // Drop 已有：删临时目录。seal 前手动调 seal_session，未 seal 的改动丢弃
}
```

tar 打包：不引 tar crate 也行——V1 用简单方案：把目录序列化为自定义格式
（文件数 + [路径len + 路径 + 内容len + 内容]...），避免额外依赖。或者直接引 `tar` crate（成熟轻量）。
**决定：引 tar + flate2（压缩）**，标准格式未来可迁移。

### 4. git2 集成（mw-store 内）

DecryptedSession 里提供便捷方法（供 Step 4 的 Agent 用）：
```rust
impl DecryptedSession {
    /// 在解密环境里做一次 git commit（如果没有 repo 则 init）
    pub fn git_commit(&self, message: &str) -> Result<()>;
    pub fn git_log(&self) -> Result<Vec<String>>;
}
```

### 5. CLI

```bash
mindwiki init --password ***          # 在当前目录创建 vault.mwenc
mindwiki status                       # 显示容器状态（存在/大小/块数）
mindwiki ask "..."                    # 已有（Step3 再接到加密会话上）
```

### 6. 测试（自动化，不需要 LLM）

```rust
#[test] fn vault_roundtrip() {
    // init → open_session → 写文件 + git_commit → seal → drop
    // 再 open_session → 验证文件在 + git log 有记录
}
#[test] fn wrong_password_rejected() {
    // init(password=A) → 用 B open → 应失败
}
#[test] fn gateway_close_blocks_decrypt() {
    // close 后 decrypt 应报错
}
#[test] fn disk_is_ciphertext() {
    // seal 后读容器文件，不应包含明文关键词（如写入的 "TOPSECRET"）
}
```

## 依赖变化
workspace Cargo.toml：加 besure git 依赖；mw-store 加 tar、flate2；mw-crypto 加 besure。

注意 besure 的 VaultCrypto 是否好独立用——如果它的 API 带着 Vault 概念太重，就在 mw-crypto 里直接用 argon2 + aes-gcm crates 自己写 100 行（workspace 已声明这两个依赖）。
**决定：优先试 besure::crypto::VaultCrypto；不顺手就自写**（反正算法一样，PPT 话术不变）。

## 验收
1. cargo build --release + cargo test --workspace 全绿（新增 ≥4 测试）
2. mindwiki init 成功创建加密容器
3. 磁盘上无明文（disk_is_ciphertext 测试）
4. roundtrip：写入→封印→重开→数据完整 + git 历史保留
5. 错密码拒绝 + 闸门关闭拒绝
6. 提交推送
