# SPEC: Step 3 — 密钥闸门一键关闭（本地 + 远程）+ 活跃会话管理

## 目标
PPT 第 4 页的承诺落地：一键关闭闸门（本地 CLI / 远程 API），立即终止所有活跃解密会话、密钥清零，此后所有密文不可解。

## 改的文件
- `crates/mw-crypto/src/gateway.rs` — 活跃会话注册表 + 关闭时终止全部会话
- `crates/mw-store/src/vault.rs` — DecryptedSession 注册到 gateway
- `crates/mw-server/src/main.rs` — CLI：mindwiki lock（本地一键关闭）
- `crates/mw-server/src/serve.rs`（新）— axum Web 服务骨架 + 远程关闭端点
- `crates/mw-server/Cargo.toml` — 加 axum

## 详细设计

### 1. 会话注册表（mw-crypto）

```rust
pub struct KeyGateway {
    crypto: Mutex<Option<VaultCrypto>>,
    // 活跃会话注册表：session_id → 强制终止句柄
    sessions: Mutex<HashMap<String, SessionHandle>>,
    pub closed_at: Mutex<Option<std::time::Instant>>, // 审计：何时关闭
}

pub struct SessionHandle {
    pub work_dir: PathBuf,
    pub terminate: Arc<AtomicBool>,  // DecryptedSession 的终止旗标
}
```

**关闭流程（close()）：**
1. crypto 置 None（密钥 zeroize）
2. 遍历 sessions：每个 terminate 置 true
3. DecryptedSession 的 Drop 检查 terminate 旗标——被终止的会话**不做 seal、直接 shred 临时目录**
4. sessions 清空
5. 记录 closed_at（审计日志）

**DecryptedSession 修改（mw-store）：**
- 持有 Option<Arc<AtomicBool>>（终止旗标）+ session_id
- `seal_session()` 正常路径：封印后从注册表注销，正常清理
- Drop：若 terminate=true → 只销毁不封印（数据丢弃，密文容器保持旧状态）
- 注册/注销走 gateway 回调（通过 trait 保持层间解耦，不直接依赖具体类型）

层间解耦方案：mw-crypto 定义 trait SessionRegistry：
```rust
pub trait SessionRegistry: Send + Sync {
    fn register(&self, id: &str, work_dir: &Path, flag: Arc<AtomicBool>);
    fn unregister(&self, id: &str);
}
```
mw-store 的 DecryptedSession 拿 &dyn SessionRegistry（由上层注入）。

### 2. CLI 本地关闭

```bash
mindwiki lock        # 一键关闭：闸门关闭 + 终止会话 + 密钥清零
```

单进程模式（CLI）：lock 作用于当前进程的 gateway。
常驻模式（serve）：lock 通过内存通道发给服务进程。

### 3. Web 服务骨架 + 远程关闭（mw-server/serve.rs）

axum 服务（Step 4 会扩展成完整 Web 界面，本步只做安全端点）：

```
POST /api/gateway/open   {password}   → 开闸（解密密钥进内存）
POST /api/gateway/close  {admin_token} → 一键远程关闭
GET  /api/gateway/state               → {state: open|closed, closed_at, active_sessions}
```

**admin_token**：客户的管理密钥。init 时生成随机 token 写入 vault 目录 admin.token 文件（chmod 600），
客户保存。远程关闭必须带此 token——防止任意人调 API 关闭（DoS）。
注意：关闭是"拒绝服务"性质的安全操作，token 保护足够；开闸需要主密码（更强）。

服务绑定：默认 127.0.0.1:7900（本地模式）。远程管理需要 --admin-bind 0.0.0.0:7901 显式开启（独立端口）。

### 4. CLI

```bash
mindwiki serve [--admin-bind 0.0.0.0:7901]   # 启动服务
mindwiki lock                                 # 本地一键关闭（对 serve 进程发请求）
mindwiki remote-close --host x.x.x.x:7901 --token ***  # 远程一键关闭
```

### 5. 测试

```rust
#[test] fn close_terminates_sessions() {
    // open → open_session → close → session 的 terminate=true
    // seal 被拒绝；容器保持旧密文
}
#[test] fn terminated_session_not_sealed() {
    // 被终止的会话 drop 时容器未更新（旧数据完好）
}
#[tokio::test] async fn remote_close_endpoint() {
    // 起 axum → open → POST /api/gateway/close (带错token) → 403
    // 带对 token → 200 → state 变 closed
}
#[test] fn admin_token_file_created_on_init() {
    // init 后 admin.token 存在且权限 600
}
```

## 验收
1. cargo build --release + cargo test --workspace 全绿（新增 ≥4 测试）
2. mindwiki lock 可用；被终止会话不落任何明文
3. curl 远程关闭端点：错 token 拒绝、对 token 关闭成功
4. 提交推送
