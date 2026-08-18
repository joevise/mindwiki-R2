//! Web 服务：内嵌单页界面 + 密钥闸门远程管理 + vault 操作 API（Step 4 完整闭环）。
//!
//! GET  /                     → 内嵌 webui.html（单页界面）
//! POST /api/gateway/open     {password}      → 开闸（主密码，更强保护）
//! POST /api/gateway/close    {admin_token}   → 一键远程关闭（admin token 防 DoS）
//! GET  /api/gateway/state                    → {state, closed_at, active_sessions}
//! POST /api/vault/init       {password}      → 创建 vault（已存在 409）
//! GET  /api/vault/status                     → {exists, size, state: sealed|open}
//! POST /api/ingest           multipart .md   → 解密会话 + Agent 入库 + 封印
//! POST /api/query            {question}      → 解密会话 + Agent 查询（有变更才封印）

use anyhow::{bail, Context, Result};
use axum::{
    extract::{Multipart, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use mw_crypto::{GatewayState, KeyGateway};
use mw_store::Vault;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

const WEBUI: &str = include_str!("webui.html");

pub struct AppState {
    pub vault: Vault,
    /// Arc 包装：handler 里克隆出来后跨 await 使用（解密会话借用 gateway 生命周期）
    pub gateway: RwLock<Arc<KeyGateway>>,
    pub admin_token: RwLock<String>,
    pub skills_root: PathBuf,
    /// 每 vault 一把锁：同时只允许一个解密会话在工作（加密容器的天然约束）。
    /// 后续多 vault 时可按 vault_id 建锁表。
    pub vault_lock: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Deserialize)]
struct OpenRequest {
    password: String,
}

#[derive(Deserialize)]
struct CloseRequest {
    admin_token: String,
}

#[derive(Deserialize)]
struct InitRequest {
    password: String,
}

#[derive(Deserialize)]
struct QueryRequest {
    question: String,
}

#[derive(Serialize)]
struct StateResponse {
    state: &'static str,
    closed_at: Option<String>,
    active_sessions: usize,
}

#[derive(Serialize)]
struct VaultStatusResponse {
    exists: bool,
    size: u64,
    state: &'static str,
}

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(webui_handler))
        .route("/api/gateway/open", post(open_handler))
        .route("/api/gateway/close", post(close_handler))
        .route("/api/gateway/state", get(state_handler))
        .route("/api/vault/init", post(init_handler))
        .route("/api/vault/status", get(vault_status_handler))
        .route("/api/ingest", post(ingest_handler))
        .route("/api/query", post(query_handler))
        .with_state(state)
}

async fn webui_handler() -> Html<&'static str> {
    Html(WEBUI)
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({"error": msg.into()}))).into_response()
}

fn gateway(s: &AppState) -> Arc<KeyGateway> {
    s.gateway.read().unwrap().clone()
}

/// LLM 未配置 → 503（先于闸门检查，给用户更明确的配置提示）
fn check_llm() -> Result<(), Response> {
    let missing = std::env::var("MW_LLM_API_KEY").map(|v| v.is_empty()).unwrap_or(true);
    if missing {
        return Err(err(
            StatusCode::SERVICE_UNAVAILABLE,
            "未配置 LLM：请设置环境变量 MW_LLM_API_KEY（可选 MW_LLM_PROVIDER / MW_LLM_BASE_URL / MW_LLM_MODEL）后重启 mindwiki serve",
        ));
    }
    Ok(())
}

/// 闸门关闭 → 423 Locked
fn check_gate(gw: &KeyGateway) -> Result<(), Response> {
    gw.guard().map_err(|_| {
        err(
            StatusCode::LOCKED,
            "知识库已锁定：请先解锁（POST /api/gateway/open）",
        )
    })
}

async fn open_handler(
    State(s): State<Arc<AppState>>,
    Json(req): Json<OpenRequest>,
) -> impl IntoResponse {
    if !s.vault.exists() {
        return err(StatusCode::CONFLICT, "vault 不存在：请先创建知识库（POST /api/vault/init）");
    }
    match gateway(&s).open(&req.password) {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"state": "open"}))).into_response(),
        Err(e) => err(StatusCode::FORBIDDEN, e.to_string()),
    }
}

async fn close_handler(
    State(s): State<Arc<AppState>>,
    Json(req): Json<CloseRequest>,
) -> impl IntoResponse {
    if req.admin_token != *s.admin_token.read().unwrap() {
        return err(StatusCode::FORBIDDEN, "invalid admin token");
    }
    gateway(&s).close();
    tracing::warn!("gateway closed remotely — all sessions terminated, keys zeroized");
    (
        StatusCode::OK,
        Json(serde_json::json!({"state": "closed"})),
    )
        .into_response()
}

async fn state_handler(State(s): State<Arc<AppState>>) -> Json<StateResponse> {
    let gw = gateway(&s);
    let state = match gw.state() {
        GatewayState::Open => "open",
        GatewayState::Closed => "closed",
    };
    let closed_at = gw.closed_at.lock().unwrap().map(|i| format!("{i:?}"));
    Json(StateResponse {
        state,
        closed_at,
        active_sessions: gw.active_sessions(),
    })
}

async fn init_handler(
    State(s): State<Arc<AppState>>,
    Json(req): Json<InitRequest>,
) -> impl IntoResponse {
    if req.password.is_empty() {
        return err(StatusCode::BAD_REQUEST, "password 不能为空");
    }
    let _lock = s.vault_lock.lock().await;
    if s.vault.exists() {
        return err(StatusCode::CONFLICT, "vault 已存在");
    }
    let gw = match KeyGateway::new() {
        Ok(g) => g,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    if let Err(e) = s.vault.init(&gw, &req.password) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    // 创建后立即落回密封态，并从容器热加载 gateway（等 /api/gateway/open 解锁）
    gw.close();
    match gateway_from_container(&s.vault) {
        Ok((fresh, token)) => {
            *s.gateway.write().unwrap() = Arc::new(fresh);
            *s.admin_token.write().unwrap() = token;
        }
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "created": true,
            "container": s.vault.container_path().display().to_string(),
            "hint": "admin token 已写入 admin.token（chmod 600），一键锁定需要它",
        })),
    )
        .into_response()
}

async fn vault_status_handler(State(s): State<Arc<AppState>>) -> Json<VaultStatusResponse> {
    let exists = s.vault.exists();
    let size = if exists {
        std::fs::metadata(s.vault.container_path()).map(|m| m.len()).unwrap_or(0)
    } else {
        0
    };
    let state = if exists && gateway(&s).state() == GatewayState::Open {
        "open"
    } else {
        "sealed"
    };
    Json(VaultStatusResponse { exists, size, state })
}

async fn ingest_handler(
    State(s): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    if let Err(r) = check_llm() {
        return r;
    }
    let gw = gateway(&s);
    if let Err(r) = check_gate(&gw) {
        return r;
    }
    let mut filename = None;
    let mut content = None;
    loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                if let Some(name) = field.file_name().map(|n| n.to_string()) {
                    if name.ends_with(".md") {
                        match field.bytes().await {
                            Ok(b) => {
                                filename = Some(name);
                                content = Some(b);
                            }
                            Err(e) => return err(StatusCode::BAD_REQUEST, e.to_string()),
                        }
                    }
                }
            }
            Ok(None) => break,
            Err(e) => return err(StatusCode::BAD_REQUEST, e.to_string()),
        }
    }
    let (filename, content) = match (filename, content) {
        (Some(f), Some(c)) => (f, c),
        _ => return err(StatusCode::BAD_REQUEST, "请上传 .md 文件（multipart 字段带 filename）"),
    };

    let _lock = s.vault_lock.lock().await;
    let session = match s.vault.open_session(&gw) {
        Ok(s) => s,
        Err(e) => return err(StatusCode::LOCKED, format!("打开解密会话失败：{e}")),
    };
    let work = session.work_dir().to_path_buf();
    let before = snapshot(&work);

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let rel = format!("inbox/upload-{ts}.md");
    let dst = work.join(&rel);
    if let Some(p) = dst.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    if let Err(e) = std::fs::write(&dst, &content) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    let agent = mw_agent::WikiAgent::new(&s.skills_root, &work);
    let prompt = format!(
        "使用 wiki-ingest 技能，将上传的文件 {rel}（原始文件名 {filename}）入库到知识库。work_dir 即知识库根目录。完成后简述入库结果。"
    );
    let answer = match agent.ask(&prompt).await {
        Ok(a) => a,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("Agent 入库失败：{e}")),
    };

    let after = snapshot(&work);
    let files: Vec<String> = diff_snapshots(&before, &after);
    if let Err(e) = s.vault.seal_session(&gw, &session) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, format!("封印失败：{e}"));
    }
    (StatusCode::OK, Json(serde_json::json!({"answer": answer, "files": files}))).into_response()
}

async fn query_handler(
    State(s): State<Arc<AppState>>,
    Json(req): Json<QueryRequest>,
) -> impl IntoResponse {
    if let Err(r) = check_llm() {
        return r;
    }
    let gw = gateway(&s);
    if let Err(r) = check_gate(&gw) {
        return r;
    }
    if req.question.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "question 不能为空");
    }

    let _lock = s.vault_lock.lock().await;
    let session = match s.vault.open_session(&gw) {
        Ok(s) => s,
        Err(e) => return err(StatusCode::LOCKED, format!("打开解密会话失败：{e}")),
    };
    let work = session.work_dir().to_path_buf();
    let before = snapshot(&work);

    let agent = mw_agent::WikiAgent::new(&s.skills_root, &work);
    let prompt = format!("使用 wiki-query 技能回答问题：{}", req.question);
    let answer = match agent.ask(&prompt).await {
        Ok(a) => a,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("Agent 查询失败：{e}")),
    };

    // 默认只读：agent 在 wiki/ 产生新内容（用户选择沉淀答案）才 seal，否则直接 drop session
    let after = snapshot(&work);
    let files: Vec<String> = diff_snapshots(&before, &after);
    let mut sealed = false;
    if !files.is_empty() {
        if let Err(e) = s.vault.seal_session(&gw, &session) {
            return err(StatusCode::INTERNAL_SERVER_ERROR, format!("封印失败：{e}"));
        }
        sealed = true;
    }
    (StatusCode::OK, Json(serde_json::json!({"answer": answer, "sealed": sealed, "files": files})))
        .into_response()
}

/// work_dir 文件快照（相对路径 → 内容哈希，跳过 .git）
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, u64> {
    use std::hash::Hasher;
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        if let Ok(entries) = std::fs::read_dir(&d) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name();
                if name == ".git" {
                    continue;
                }
                match entry.file_type() {
                    Ok(t) if t.is_dir() => stack.push(path),
                    Ok(t) if t.is_file() => {
                        if let Ok(bytes) = std::fs::read(&path) {
                            let mut h = std::collections::hash_map::DefaultHasher::new();
                            h.write(&bytes);
                            if let Ok(rel) = path.strip_prefix(dir) {
                                out.insert(rel.to_path_buf(), h.finish());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    out
}

/// 新增或变更的文件列表
fn diff_snapshots(before: &BTreeMap<PathBuf, u64>, after: &BTreeMap<PathBuf, u64>) -> Vec<String> {
    after
        .iter()
        .filter(|(p, h)| before.get(*p) != Some(h))
        .map(|(p, _)| p.display().to_string())
        .collect()
}

/// 从磁盘容器构建服务状态：gateway 初始为关闭态（等 /api/gateway/open 解锁）。
/// vault 不存在时给出空 gateway（可通过 /api/vault/init 创建）。
pub fn load_state(vault: Vault, skills_root: PathBuf) -> Result<Arc<AppState>> {
    let (gateway, admin_token) = if vault.exists() {
        gateway_from_container(&vault)?
    } else {
        (KeyGateway::new()?, String::new())
    };
    Ok(Arc::new(AppState {
        vault,
        gateway: RwLock::new(Arc::new(gateway)),
        admin_token: RwLock::new(admin_token),
        skills_root,
        vault_lock: Arc::new(tokio::sync::Mutex::new(())),
    }))
}

fn gateway_from_container(vault: &Vault) -> Result<(KeyGateway, String)> {
    let data = std::fs::read(vault.container_path()).context("read vault container")?;
    let c = mw_store::container::decode(&data)?;
    let gateway = KeyGateway::from_container(c.salt, c.verify_token);
    let admin_token = vault.ensure_admin_token()?;
    Ok((gateway, admin_token))
}

pub async fn serve(listener: tokio::net::TcpListener, state: Arc<AppState>) -> Result<()> {
    axum::serve(listener, build_router(state))
        .await
        .context("axum serve")
}

/// 远程一键关闭：POST {admin_token} 到目标 serve 进程
pub async fn remote_close(host: &str, token: &str) -> Result<()> {
    let url = format!("http://{host}/api/gateway/close");
    let resp = reqwest::Client::new()
        .post(&url)
        .json(&serde_json::json!({"admin_token": token}))
        .send()
        .await
        .context("connect to mindwiki serve")?;
    if resp.status() == StatusCode::FORBIDDEN {
        bail!("remote close rejected: invalid admin token");
    }
    if !resp.status().is_success() {
        bail!("remote close failed: HTTP {}", resp.status());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn spawn_server(state: Arc<AppState>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve(listener, state));
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn remote_close_endpoint() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::open(tmp.path()).unwrap();
        let gw = KeyGateway::new().unwrap();
        vault.init(&gw, "pw-A").unwrap();
        gw.close();

        let state = load_state(vault, tmp.path().to_path_buf()).unwrap();
        let token = state.admin_token.read().unwrap().clone();
        assert_eq!(state.gateway.read().unwrap().state(), GatewayState::Closed);

        let base = spawn_server(state).await;
        let client = reqwest::Client::new();

        // 开闸
        let resp = client
            .post(format!("{base}/api/gateway/open"))
            .json(&serde_json::json!({"password": "pw-A"}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // 错 token → 403，闸门仍开
        let resp = client
            .post(format!("{base}/api/gateway/close"))
            .json(&serde_json::json!({"admin_token": "wrong"}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let st: serde_json::Value = client
            .get(format!("{base}/api/gateway/state"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(st["state"], "open");

        // 对 token → 200，state 变 closed 且记录 closed_at
        let resp = client
            .post(format!("{base}/api/gateway/close"))
            .json(&serde_json::json!({"admin_token": token}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let st: serde_json::Value = client
            .get(format!("{base}/api/gateway/state"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(st["state"], "closed");
        assert!(st["closed_at"].is_string());
        assert_eq!(st["active_sessions"], 0);
    }

    #[tokio::test]
    async fn webui_served() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::open(tmp.path()).unwrap();
        let state = load_state(vault, tmp.path().to_path_buf()).unwrap();
        let base = spawn_server(state).await;

        let resp = reqwest::get(format!("{base}/")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp.headers()["content-type"].to_str().unwrap().to_string();
        assert!(ct.contains("text/html"));
        let body = resp.text().await.unwrap();
        assert!(body.contains("Mind Wiki"));

        // 未初始化：status 报不存在、密封
        let st: serde_json::Value = reqwest::get(format!("{base}/api/vault/status"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(st["exists"], false);
        assert_eq!(st["state"], "sealed");
    }

    #[tokio::test]
    async fn full_loop_e2e() {
        std::env::remove_var("MW_LLM_API_KEY");
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::open(tmp.path()).unwrap();
        let state = load_state(vault, tmp.path().to_path_buf()).unwrap();
        let admin_token_path = state.vault.admin_token_path();
        let base = spawn_server(state).await;
        let client = reqwest::Client::new();

        // init → 200
        let resp = client
            .post(format!("{base}/api/vault/init"))
            .json(&serde_json::json!({"password": "test"}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // 重复 init → 409
        let resp = client
            .post(format!("{base}/api/vault/init"))
            .json(&serde_json::json!({"password": "test"}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);

        // open → 200
        let resp = client
            .post(format!("{base}/api/gateway/open"))
            .json(&serde_json::json!({"password": "test"}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // status → open
        let st: serde_json::Value = client
            .get(format!("{base}/api/vault/status"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(st["exists"], true);
        assert!(st["size"].as_u64().unwrap() > 0);
        assert_eq!(st["state"], "open");

        // close → 200
        let token = std::fs::read_to_string(admin_token_path).unwrap().trim().to_string();
        let resp = client
            .post(format!("{base}/api/gateway/close"))
            .json(&serde_json::json!({"admin_token": token}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // 未配 LLM → 503 带明确提示
        let resp = client
            .post(format!("{base}/api/query"))
            .json(&serde_json::json!({"question": "你好"}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(body["error"].as_str().unwrap().contains("MW_LLM_API_KEY"));

        // 闸门关闭 → 423
        std::env::set_var("MW_LLM_API_KEY", "sk-dummy");
        let resp = client
            .post(format!("{base}/api/query"))
            .json(&serde_json::json!({"question": "你好"}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::LOCKED);
        std::env::remove_var("MW_LLM_API_KEY");
    }
}
