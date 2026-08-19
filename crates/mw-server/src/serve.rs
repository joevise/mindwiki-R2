//! Web 服务：内嵌单页界面 + 密钥闸门远程管理 + vault 操作 API + Wiki 浏览器。
//!
//! GET  /                     → 内嵌 webui.html（单页界面）
//! POST /api/gateway/open     {password}      → 开闸 + 创建长驻解密会话
//! POST /api/gateway/close    {admin_token}   → 一键远程关闭 + 销毁解密会话
//! GET  /api/gateway/state                    → {state, closed_at, active_sessions}
//! POST /api/vault/init       {password}      → 创建 vault（已存在 409）
//! GET  /api/vault/status                     → {exists, size, state: sealed|open}
//! POST /api/ingest           multipart .md   → 复用解密会话 + Agent 入库 + 即时封印
//! POST /api/wiki/import      multipart .zip  → 现成 Wiki 整包导入（结构保留，不调 LLM）
//! POST /api/query            {question}      → 复用解密会话 + Agent 查询（有变更才封印）
//! GET  /api/wiki/tree                        → 文件树 JSON（排除 .git）
//! GET  /api/wiki/page?path=                  → {path, content}（防路径穿越）
//! GET  /api/wiki/graph                       → {nodes, edges}（frontmatter type + wikilink）
//! GET  /api/llm/config                       → {provider, base_url, model, api_key_masked}
//! POST /api/llm/config       {provider, base_url, api_key, model} → 保存 appconfig.json + 热生效
//! POST /api/ingest/stream    multipart .md   → SSE：tool_call/message/done 实时入库进度
//! POST /api/chat             {message}       → SSE：多轮聊天（message/done）
//! POST /api/chat/reset                       → 清空聊天会话
//! DELETE /api/wiki/entry     {path, mode}    → 删除文件/目录（quick 直接删；smart 先删再 Agent 清理引用）

use anyhow::{bail, Context, Result};
use axum::{
    extract::{DefaultBodyLimit, Multipart, Query, State},
    http::StatusCode,
    response::{sse::Event as SseEvent, Html, IntoResponse, Response, Sse},
    routing::{delete, get, post},
    Json, Router,
};
use mw_crypto::{GatewayState, KeyGateway};
use mw_store::Vault;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

const WEBUI: &str = include_str!("webui.html");

pub struct AppState {
    pub vault: Vault,
    /// Arc 包装：handler 里克隆出来后跨 await 使用
    pub gateway: RwLock<Arc<KeyGateway>>,
    pub admin_token: RwLock<String>,
    pub skills_root: PathBuf,
    /// 每 vault 一把锁：同时只允许一个写操作（ingest/query/open）在工作。
    /// 后续多 vault 时可按 vault_id 建锁表。
    pub vault_lock: Arc<tokio::sync::Mutex<()>>,
    /// 长驻解密会话：解锁即解密（open 创建），锁定即销毁（close 置 None）。
    /// ingest/query/browse 全部复用它的 work_dir；明文不落盘。
    pub current_session: tokio::sync::RwLock<Option<mw_store::DecryptedSession>>,
    /// LLM 配置：启动时 load_or_env(appconfig.json)，POST /api/llm/config 热更新
    pub llm: RwLock<mw_agent::LlmConfig>,
    /// appconfig.json 路径（vault 根目录下）
    pub config_path: PathBuf,
    /// 多轮聊天会话：解锁后懒建复用，锁定/改模型时清空
    pub chat: tokio::sync::Mutex<Option<mw_agent::ChatSession>>,
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
        .route(
            "/api/wiki/import",
            post(import_handler).layer(DefaultBodyLimit::max(MAX_IMPORT_BYTES)),
        )
        .route("/api/query", post(query_handler))
        .route("/api/wiki/tree", get(tree_handler))
        .route("/api/wiki/page", get(page_handler))
        .route("/api/wiki/graph", get(graph_handler))
        .route(
            "/api/llm/config",
            get(get_llm_config_handler).post(post_llm_config_handler),
        )
        .route("/api/ingest/stream", post(ingest_stream_handler))
        .route("/api/chat", post(chat_handler))
        .route("/api/chat/reset", post(chat_reset_handler))
        .route("/api/wiki/entry", delete(delete_entry_handler))
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

/// LLM 未配置 → 503（先于闸门检查，给用户更明确的配置提示）。
/// 以 AppState.llm 为准；为空时惰性从 appconfig.json / 环境变量补水（兼容启动后才配置的场景）。
fn check_llm(s: &Arc<AppState>) -> Result<(), Response> {
    if !s.llm.read().unwrap().api_key.is_empty() {
        return Ok(());
    }
    let fresh = mw_agent::LlmConfig::load_or_env(&s.config_path);
    if !fresh.api_key.is_empty() {
        *s.llm.write().unwrap() = fresh;
        return Ok(());
    }
    Err(err(
        StatusCode::SERVICE_UNAVAILABLE,
        "未配置 LLM：请在设置页填写模型配置并保存，或设置环境变量 MW_LLM_API_KEY（可选 MW_LLM_PROVIDER / MW_LLM_BASE_URL / MW_LLM_MODEL）后重启 mindwiki serve",
    ))
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

/// 确保长驻解密会话存在（闸门已开但会话缺失时补建，如服务重启后状态不一致）。
/// 返回读守卫；调用方持有期间会话不会被销毁。
async fn ensure_session(
    s: &Arc<AppState>,
) -> Result<tokio::sync::RwLockReadGuard<'_, Option<mw_store::DecryptedSession>>, Response> {
    if s.current_session.read().await.is_some() {
        return Ok(s.current_session.read().await);
    }
    let gw = gateway(s);
    let mut slot = s.current_session.write().await;
    if slot.is_none() {
        match s.vault.open_session(&gw) {
            Ok(session) => *slot = Some(session),
            Err(e) => {
                return Err(err(
                    StatusCode::LOCKED,
                    format!("打开解密会话失败：{e}"),
                ))
            }
        }
    }
    Ok(slot.downgrade())
}

async fn open_handler(
    State(s): State<Arc<AppState>>,
    Json(req): Json<OpenRequest>,
) -> impl IntoResponse {
    if !s.vault.exists() {
        return err(StatusCode::CONFLICT, "vault 不存在：请先创建知识库（POST /api/vault/init）");
    }
    match gateway(&s).open(&req.password) {
        Ok(()) => {
            // 解锁即解密：创建长驻会话存入 AppState（旧会话先销毁）
            let _lock = s.vault_lock.lock().await;
            let gw = gateway(&s);
            let mut slot = s.current_session.write().await;
            *slot = None;
            match s.vault.open_session(&gw) {
                Ok(session) => {
                    *slot = Some(session);
                    (StatusCode::OK, Json(serde_json::json!({"state": "open"}))).into_response()
                }
                Err(e) => err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("解密会话创建失败：{e}"),
                ),
            }
        }
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
    // 锁定即销毁：不 seal 未提交变更（ingest 已即时 seal）
    *s.current_session.write().await = None;
    // 聊天会话一并清空（持有解密目录上下文，不能跨锁定存活）
    *s.chat.lock().await = None;
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

/// multipart 提取 .md 文件（ingest / ingest_stream 共用）
async fn parse_md_multipart(mut multipart: Multipart) -> Result<(String, Vec<u8>), Response> {
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
                                content = Some(b.to_vec());
                            }
                            Err(e) => return Err(err(StatusCode::BAD_REQUEST, e.to_string())),
                        }
                    }
                }
            }
            Ok(None) => break,
            Err(e) => return Err(err(StatusCode::BAD_REQUEST, e.to_string())),
        }
    }
    match (filename, content) {
        (Some(f), Some(c)) => Ok((f, c)),
        _ => Err(err(
            StatusCode::BAD_REQUEST,
            "请上传 .md 文件（multipart 字段带 filename）",
        )),
    }
}

async fn ingest_handler(
    State(s): State<Arc<AppState>>,
    multipart: Multipart,
) -> impl IntoResponse {
    if let Err(r) = check_llm(&s) {
        return r;
    }
    let gw = gateway(&s);
    if let Err(r) = check_gate(&gw) {
        return r;
    }
    let (filename, content) = match parse_md_multipart(multipart).await {
        Ok(v) => v,
        Err(r) => return r,
    };

    let _lock = s.vault_lock.lock().await;
    let guard = match ensure_session(&s).await {
        Ok(g) => g,
        Err(r) => return r,
    };
    let session = guard.as_ref().unwrap();
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

    let agent = mw_agent::WikiAgent::with_llm(
        &s.skills_root,
        &work,
        s.llm.read().unwrap().clone(),
    );
    let prompt = ingest_prompt(&rel, &filename);
    let answer = match agent.ask(&prompt).await {
        Ok(a) => a,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("Agent 入库失败：{e}")),
    };

    let after = snapshot(&work);
    let files: Vec<String> = diff_snapshots(&before, &after);
    // 即时 seal 更新容器，但长驻会话保持存活（后续 ingest/query/browse 复用）
    if let Err(e) = s.vault.seal_session(&gw, session) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, format!("封印失败：{e}"));
    }
    (StatusCode::OK, Json(serde_json::json!({"answer": answer, "files": files}))).into_response()
}

/// 入库 prompt（ingest / ingest_stream 共用）
fn ingest_prompt(rel: &str, filename: &str) -> String {
    format!(
        "将上传文件 {rel}（原始文件名 {filename}）入库到知识库，work_dir 即知识库根目录。规则：先检查根目录是否有 index.md——若无（全新知识库），先用 wiki-init 技能初始化 Wiki，然后用 wiki-ingest 技能入库该文件；若已有则直接 wiki-ingest。不要向用户提问确认，直接执行到底。完成后简述初始化与入库结果（建了哪些页面/类型）。"
    )
}

/// Wiki 压缩包上传上限（50MB）
const MAX_IMPORT_BYTES: usize = 50 * 1024 * 1024;

/// 垃圾路径：.git/ .obsidian/ .trash/ __MACOSX/ 目录与 .DS_Store / Thumbs.db 文件
fn is_import_junk(rel: &Path) -> bool {
    rel.components().any(|c| {
        matches!(c, std::path::Component::Normal(n)
            if matches!(n.to_str(), Some(".git" | ".obsidian" | ".trash" | "__MACOSX" | ".DS_Store" | "Thumbs.db")))
    })
}

/// POST /api/wiki/import：现成 Wiki 整包导入（结构保留、不萃取、不调 LLM）。
/// multipart 字段 file = 一个 .zip（≤50MB）；同名覆盖；git commit 后 seal + 重建长驻会话。
async fn import_handler(
    State(s): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let gw = gateway(&s);
    if let Err(r) = check_gate(&gw) {
        return r;
    }
    let mut data: Option<Vec<u8>> = None;
    loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                if field.file_name().is_some_and(|n| n.ends_with(".zip")) {
                    match field.bytes().await {
                        Ok(b) => {
                            if b.len() > MAX_IMPORT_BYTES {
                                return err(
                                    StatusCode::PAYLOAD_TOO_LARGE,
                                    "压缩包超过 50MB 上限",
                                );
                            }
                            data = Some(b.to_vec());
                        }
                        Err(e) => return err(e.status(), e.body_text()),
                    }
                }
            }
            Ok(None) => break,
            Err(e) => return err(e.status(), e.body_text()),
        }
    }
    let bytes = match data {
        Some(b) => b,
        None => {
            return err(
                StatusCode::BAD_REQUEST,
                "请上传 .zip 文件（multipart 字段带 filename）",
            )
        }
    };

    let _lock = s.vault_lock.lock().await;
    let mut slot = s.current_session.write().await;
    if slot.is_none() {
        return err(
            StatusCode::LOCKED,
            "知识库已锁定：请先解锁（POST /api/gateway/open）",
        );
    }
    let session = slot.as_ref().unwrap();
    let work = session.work_dir().to_path_buf();

    let mut archive = match zip::ZipArchive::new(std::io::Cursor::new(bytes)) {
        Ok(a) => a,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("zip 解析失败：{e}")),
    };
    let mut imported: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for i in 0..archive.len() {
        let mut entry = match archive.by_index(i) {
            Ok(e) => e,
            Err(e) => return err(StatusCode::BAD_REQUEST, format!("zip 读取失败：{e}")),
        };
        let name = entry.name().replace('\\', "/");
        let rel = Path::new(&name);
        // sanitize：拒绝 .. / 绝对路径 / 符号链接 entry
        if rel.is_absolute()
            || rel.components().any(|c| {
                matches!(
                    c,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                )
            })
        {
            return err(
                StatusCode::BAD_REQUEST,
                format!("压缩包包含恶意路径：{name}"),
            );
        }
        if entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
            return err(
                StatusCode::BAD_REQUEST,
                format!("压缩包包含符号链接：{name}"),
            );
        }
        if is_import_junk(rel) {
            skipped.push(name);
            continue;
        }
        let dst = work.join(rel);
        if entry.is_dir() {
            if let Err(e) = std::fs::create_dir_all(&dst) {
                return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
            }
            continue;
        }
        if let Some(p) = dst.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        let mut buf = Vec::new();
        if let Err(e) = std::io::Read::read_to_end(&mut entry, &mut buf) {
            return err(StatusCode::BAD_REQUEST, format!("zip 解压失败：{e}"));
        }
        // 同名覆盖：导入是显式用户动作
        if let Err(e) = std::fs::write(&dst, &buf) {
            return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
        imported.push(name);
    }

    if let Err(e) = session.git_commit(&format!("Import wiki bundle: {} files", imported.len())) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, format!("git 提交失败：{e}"));
    }
    if let Err(e) = s.vault.seal_session(&gw, session) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, format!("封印失败：{e}"));
    }
    // seal 后重建长驻会话（旧会话销毁，open_session 新建放回 AppState）
    *slot = None;
    match s.vault.open_session(&gw) {
        Ok(new_session) => *slot = Some(new_session),
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("会话重建失败：{e}"),
            )
        }
    }
    drop(slot);
    // 聊天会话绑定了旧 work_dir，作废待下轮懒建
    *s.chat.lock().await = None;

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "imported": imported,
            "skipped": skipped,
            "committed": true,
        })),
    )
        .into_response()
}

/* ============ 删除（快速 + 智能） ============ */

#[derive(Deserialize)]
struct DeleteRequest {
    path: String,
    mode: String,
}

/// 保护名单：检索/协议/日志核心文件不可删
const PROTECTED_PATHS: [&str; 3] = ["index.md", "schema.md", "log.md"];

/// 递归统计目标包含的文件数（文件→1，目录→整树文件数）
fn count_files(p: &Path) -> usize {
    if p.is_file() {
        return 1;
    }
    let mut n = 0;
    let mut stack = vec![p.to_path_buf()];
    while let Some(d) = stack.pop() {
        if let Ok(entries) = std::fs::read_dir(&d) {
            for entry in entries.flatten() {
                match entry.file_type() {
                    Ok(t) if t.is_dir() => stack.push(entry.path()),
                    Ok(t) if t.is_file() => n += 1,
                    _ => {}
                }
            }
        }
    }
    n
}

/// LLM 是否可用（惰性从 appconfig.json / 环境变量补水），不可用时不报错、由调用方降级
fn llm_ready(s: &Arc<AppState>) -> bool {
    if !s.llm.read().unwrap().api_key.is_empty() {
        return true;
    }
    let fresh = mw_agent::LlmConfig::load_or_env(&s.config_path);
    if !fresh.api_key.is_empty() {
        *s.llm.write().unwrap() = fresh;
        return true;
    }
    false
}

/// 智能删除后清理 prompt（agent 只做引用清理，不重新萃取）
fn delete_cleanup_prompt(path: &str) -> String {
    let stem = Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());
    format!(
        "文件 {path} 已从知识库删除，work_dir 即知识库根目录。请扫描全库（grep 搜索 [[{stem}]] 引用），清理所有悬空链接（直接移除该 wikilink 或标注\"已删除\"），更新 index.md 检索路由（如有该页条目），如 log.md 需要追加删除记录则追加。完成后简述清理了哪些文件。不要向用户提问。"
    )
}

/// DELETE /api/wiki/entry {path, mode:quick|smart}
/// 共同逻辑：路径 sanitize（canonicalize 前缀校验）+ 保护名单 + 闸门 + vault_lock。
/// quick：删文件/目录 + git commit + seal + 重建会话。
/// smart：先删再 WikiAgent 清理引用（无 LLM 配置降级 quick，响应带 degraded:true）。
async fn delete_entry_handler(
    State(s): State<Arc<AppState>>,
    Json(req): Json<DeleteRequest>,
) -> Response {
    let gw = gateway(&s);
    if let Err(r) = check_gate(&gw) {
        return r;
    }
    let rel = req.path.replace('\\', "/");
    let rel_path = Path::new(&rel);
    if rel.trim().is_empty()
        || rel_path.is_absolute()
        || rel_path.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir | std::path::Component::RootDir
            )
        })
    {
        return err(StatusCode::BAD_REQUEST, "非法路径：越出知识库目录");
    }
    if PROTECTED_PATHS.contains(&rel.as_str()) {
        return err(
            StatusCode::BAD_REQUEST,
            format!("{rel} 是知识库核心文件（index/schema/log），不可删除"),
        );
    }
    if req.mode != "quick" && req.mode != "smart" {
        return err(StatusCode::BAD_REQUEST, "mode 仅支持 quick 或 smart");
    }

    let _lock = s.vault_lock.lock().await;
    let mut slot = s.current_session.write().await;
    if slot.is_none() {
        return err(
            StatusCode::LOCKED,
            "知识库已锁定：请先解锁（POST /api/gateway/open）",
        );
    }
    let session = slot.as_ref().unwrap();
    let work = session.work_dir().to_path_buf();

    let canon_root = match work.canonicalize() {
        Ok(r) => r,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let target = match canon_root.join(rel_path).canonicalize() {
        Ok(t) => t,
        Err(_) => return err(StatusCode::NOT_FOUND, "目标不存在"),
    };
    if !target.starts_with(&canon_root) {
        return err(StatusCode::BAD_REQUEST, "非法路径：越出知识库目录");
    }

    let files_removed = count_files(&target);
    let rm = if target.is_dir() {
        std::fs::remove_dir_all(&target)
    } else {
        std::fs::remove_file(&target)
    };
    if let Err(e) = rm {
        return err(StatusCode::INTERNAL_SERVER_ERROR, format!("删除失败：{e}"));
    }
    if let Err(e) = session.git_commit(&format!("Delete: {rel}")) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, format!("git 提交失败：{e}"));
    }

    // smart 且无 LLM 配置 → 降级 quick
    let want_smart = req.mode == "smart";
    let degraded = want_smart && !llm_ready(&s);

    let mut answer: Option<String> = None;
    let mut files_touched: Vec<String> = Vec::new();
    if want_smart && !degraded {
        let before = snapshot(&work);
        let agent = mw_agent::WikiAgent::with_llm(
            &s.skills_root,
            &work,
            s.llm.read().unwrap().clone(),
        );
        match agent.ask(&delete_cleanup_prompt(&rel)).await {
            Ok(a) => {
                let after = snapshot(&work);
                files_touched = diff_snapshots(&before, &after);
                if !files_touched.is_empty() {
                    if let Err(e) =
                        session.git_commit(&format!("Cleanup references after delete: {rel}"))
                    {
                        return err(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!("git 提交失败：{e}"),
                        );
                    }
                }
                answer = Some(a);
            }
            Err(e) => {
                // 删除已生效：先 seal 重建保持一致，再报错
                if let Err(se) = s.vault.seal_session(&gw, session) {
                    return err(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("封印失败：{se}"),
                    );
                }
                *slot = None;
                if let Ok(new_session) = s.vault.open_session(&gw) {
                    *slot = Some(new_session);
                }
                drop(slot);
                *s.chat.lock().await = None;
                return err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("文件已删除，但 Agent 引用清理失败：{e}"),
                );
            }
        }
    }

    // seal + 重建长驻会话（同 import：旧会话销毁，open_session 新建放回 AppState）
    if let Err(e) = s.vault.seal_session(&gw, session) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, format!("封印失败：{e}"));
    }
    *slot = None;
    match s.vault.open_session(&gw) {
        Ok(new_session) => *slot = Some(new_session),
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("会话重建失败：{e}"),
            )
        }
    }
    drop(slot);
    // 聊天会话绑定了旧 work_dir，作废待下轮懒建
    *s.chat.lock().await = None;

    let mut body = serde_json::json!({
        "deleted": true,
        "files_removed": files_removed,
    });
    if degraded {
        body["degraded"] = serde_json::json!(true);
    }
    if let Some(a) = answer {
        body["answer"] = serde_json::json!(a);
        body["files_touched"] = serde_json::json!(files_touched);
    }
    (StatusCode::OK, Json(body)).into_response()
}

async fn query_handler(
    State(s): State<Arc<AppState>>,
    Json(req): Json<QueryRequest>,
) -> impl IntoResponse {
    if let Err(r) = check_llm(&s) {
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
    let guard = match ensure_session(&s).await {
        Ok(g) => g,
        Err(r) => return r,
    };
    let session = guard.as_ref().unwrap();
    let work = session.work_dir().to_path_buf();
    let before = snapshot(&work);

    let agent = mw_agent::WikiAgent::with_llm(
        &s.skills_root,
        &work,
        s.llm.read().unwrap().clone(),
    );
    let prompt = format!("使用 wiki-query 技能回答问题：{}", req.question);
    let answer = match agent.ask(&prompt).await {
        Ok(a) => a,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("Agent 查询失败：{e}")),
    };

    // 默认只读：agent 在 wiki/ 产生新内容（用户选择沉淀答案）才 seal
    let after = snapshot(&work);
    let files: Vec<String> = diff_snapshots(&before, &after);
    let mut sealed = false;
    if !files.is_empty() {
        if let Err(e) = s.vault.seal_session(&gw, session) {
            return err(StatusCode::INTERNAL_SERVER_ERROR, format!("封印失败：{e}"));
        }
        sealed = true;
    }
    (StatusCode::OK, Json(serde_json::json!({"answer": answer, "sealed": sealed, "files": files})))
        .into_response()
}

#[derive(Deserialize)]
struct PageParams {
    path: String,
}

/// 浏览类 API 公共前置：闸门开 + 会话在 → 返回 work_dir（只读操作，不进 vault_lock）
async fn browse_work_dir(s: &Arc<AppState>) -> Result<PathBuf, Response> {
    let gw = gateway(s);
    check_gate(&gw)?;
    let guard = s.current_session.read().await;
    match guard.as_ref() {
        Some(session) => Ok(session.work_dir().to_path_buf()),
        None => Err(err(
            StatusCode::LOCKED,
            "知识库已锁定：请先解锁（POST /api/gateway/open）",
        )),
    }
}

async fn tree_handler(State(s): State<Arc<AppState>>) -> Response {
    let work = match browse_work_dir(&s).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    Json(build_tree(&work)).into_response()
}

async fn page_handler(
    State(s): State<Arc<AppState>>,
    Query(params): Query<PageParams>,
) -> Response {
    let work = match browse_work_dir(&s).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    let path = match resolve_page(&work, &params.path) {
        Ok(p) => p,
        Err(r) => return r,
    };
    match std::fs::read_to_string(&path) {
        Ok(content) => Json(serde_json::json!({"path": params.path, "content": content}))
            .into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn graph_handler(State(s): State<Arc<AppState>>) -> Response {
    let work = match browse_work_dir(&s).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    Json(build_graph(&work)).into_response()
}

/* ============ LLM 设置 ============ */

#[derive(Deserialize)]
struct LlmConfigRequest {
    provider: String,
    base_url: String,
    api_key: String,
    model: String,
}

async fn get_llm_config_handler(State(s): State<Arc<AppState>>) -> Response {
    let gw = gateway(&s);
    if let Err(r) = check_gate(&gw) {
        return r;
    }
    let llm = s.llm.read().unwrap().clone();
    Json(serde_json::json!({
        "provider": llm.provider,
        "base_url": llm.base_url,
        "model": llm.model,
        "api_key_masked": llm.masked_key(),
    }))
    .into_response()
}

async fn post_llm_config_handler(
    State(s): State<Arc<AppState>>,
    Json(req): Json<LlmConfigRequest>,
) -> Response {
    let gw = gateway(&s);
    if let Err(r) = check_gate(&gw) {
        return r;
    }
    if req.provider != "openai_compat" && req.provider != "anthropic" {
        return err(
            StatusCode::BAD_REQUEST,
            "provider 仅支持 openai_compat 或 anthropic",
        );
    }
    if req.base_url.trim().is_empty() || req.api_key.trim().is_empty() || req.model.trim().is_empty()
    {
        return err(
            StatusCode::BAD_REQUEST,
            "base_url / api_key / model 均不能为空",
        );
    }
    let llm = mw_agent::LlmConfig {
        provider: req.provider,
        base_url: req.base_url.trim().to_string(),
        api_key: req.api_key.trim().to_string(),
        model: req.model.trim().to_string(),
    };
    if let Err(e) = llm.save(&s.config_path) {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("保存 appconfig.json 失败：{e}"),
        );
    }
    *s.llm.write().unwrap() = llm;
    // 模型变了：旧聊天会话作废，下轮用新配置重建
    *s.chat.lock().await = None;
    (StatusCode::OK, Json(serde_json::json!({"saved": true}))).into_response()
}

/* ============ SSE 基础设施 ============ */

type SseItem = Result<SseEvent, std::convert::Infallible>;
type SseTx = tokio::sync::mpsc::Sender<SseItem>;

fn sse_channel() -> (SseTx, Sse<impl futures_util::Stream<Item = SseItem>>) {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<SseItem>(64);
    let stream = futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx));
    (tx, Sse::new(stream))
}

async fn send_json(tx: &SseTx, v: serde_json::Value) {
    let _ = tx.send(Ok(SseEvent::default().data(v.to_string()))).await;
}

/// AgentEvent → SSE JSON 映射：ToolCall→tool_call（detail 截 80 字符）、
/// MessageUpdate→message、Error→error；Done 由收尾逻辑统一发（需附带 answer/files），其余跳过。
fn agent_event_json(ev: &mw_agent::AgentEvent) -> Option<serde_json::Value> {
    use mw_agent::AgentEvent as E;
    match ev {
        E::ToolCall { name, arguments } => Some(serde_json::json!({
            "type": "tool_call",
            "name": name,
            "detail": arguments.chars().take(80).collect::<String>(),
        })),
        E::MessageUpdate(text) => Some(serde_json::json!({"type": "message", "text": text})),
        E::Error(msg) => Some(serde_json::json!({"type": "error", "error": msg})),
        _ => None,
    }
}

/// 事件泵：并发驱动 agent future + 广播 receiver，事件实时写入 SSE 通道。
/// 返回 (future 输出, 是否已转发过 error 事件)。
async fn pump_events<F: std::future::Future>(
    tx: &SseTx,
    mut rx: tokio::sync::broadcast::Receiver<mw_agent::AgentEvent>,
    fut: F,
) -> (F::Output, bool) {
    use tokio::sync::broadcast::error::RecvError;
    tokio::pin!(fut);
    let mut sent_error = false;
    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(ev) => {
                    if matches!(ev, mw_agent::AgentEvent::Error(_)) {
                        sent_error = true;
                    }
                    if let Some(j) = agent_event_json(&ev) {
                        send_json(tx, j).await;
                    }
                }
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return (fut.await, sent_error),
            },
            out = &mut fut => {
                // future 完成前排干残留事件（Done 前的 tool_call/message 等）
                while let Ok(ev) = rx.try_recv() {
                    if matches!(ev, mw_agent::AgentEvent::Error(_)) {
                        sent_error = true;
                    }
                    if let Some(j) = agent_event_json(&ev) {
                        send_json(tx, j).await;
                    }
                }
                return (out, sent_error);
            }
        }
    }
}

/* ============ 流式入库 / 聊天 ============ */

async fn ingest_stream_handler(State(s): State<Arc<AppState>>, multipart: Multipart) -> Response {
    if let Err(r) = check_llm(&s) {
        return r;
    }
    let gw = gateway(&s);
    if let Err(r) = check_gate(&gw) {
        return r;
    }
    let (filename, content) = match parse_md_multipart(multipart).await {
        Ok(v) => v,
        Err(r) => return r,
    };

    let (tx, sse) = sse_channel();
    tokio::spawn(async move {
        let _lock = s.vault_lock.lock().await;
        let guard = match ensure_session(&s).await {
            Ok(g) => g,
            Err(_) => {
                send_json(&tx, serde_json::json!({"type": "error", "error": "打开解密会话失败"}))
                    .await;
                return;
            }
        };
        let session = guard.as_ref().unwrap();
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
            send_json(&tx, serde_json::json!({"type": "error", "error": e.to_string()})).await;
            return;
        }

        let agent = mw_agent::WikiAgent::with_llm(
            &s.skills_root,
            &work,
            s.llm.read().unwrap().clone(),
        );
        let (rx, handle) = agent.ask_with_events(&ingest_prompt(&rel, &filename));
        let (out, sent_error) = pump_events(&tx, rx, handle).await;
        let answer = match out {
            Ok(Ok(text)) => text,
            Ok(Err(e)) => {
                if !sent_error {
                    send_json(&tx, serde_json::json!({"type": "error", "error": e})).await;
                }
                return;
            }
            Err(e) => {
                if !sent_error {
                    send_json(&tx, serde_json::json!({"type": "error", "error": e.to_string()}))
                        .await;
                }
                return;
            }
        };

        let after = snapshot(&work);
        let files: Vec<String> = diff_snapshots(&before, &after);
        // 即时 seal 更新容器，长驻会话保持存活
        if let Err(e) = s.vault.seal_session(&gw, session) {
            send_json(
                &tx,
                serde_json::json!({"type": "error", "error": format!("封印失败：{e}")}),
            )
            .await;
            return;
        }
        send_json(
            &tx,
            serde_json::json!({"type": "done", "answer": answer, "files": files}),
        )
        .await;
    });
    sse.into_response()
}

#[derive(Deserialize)]
struct ChatRequest {
    message: String,
}

async fn chat_handler(
    State(s): State<Arc<AppState>>,
    Json(req): Json<ChatRequest>,
) -> Response {
    if let Err(r) = check_llm(&s) {
        return r;
    }
    let gw = gateway(&s);
    if let Err(r) = check_gate(&gw) {
        return r;
    }
    if req.message.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "message 不能为空");
    }

    let (tx, sse) = sse_channel();
    tokio::spawn(async move {
        let mut guard = s.chat.lock().await;
        if guard.is_none() {
            // 懒建：绑定当前解密会话的 work_dir
            let sess_guard = match ensure_session(&s).await {
                Ok(g) => g,
                Err(_) => {
                    send_json(
                        &tx,
                        serde_json::json!({"type": "error", "error": "打开解密会话失败"}),
                    )
                    .await;
                    return;
                }
            };
            let work = sess_guard.as_ref().unwrap().work_dir().to_path_buf();
            drop(sess_guard);
            // 人格层：vault 根目录 mindrule.txt 存在则注入聊天 system_prompt
            let mindrule = std::fs::read_to_string(s.vault.root.join("mindrule.txt")).ok();
            let agent = mw_agent::WikiAgent::with_llm(
                &s.skills_root,
                &work,
                s.llm.read().unwrap().clone(),
            )
            .with_mindrule(mindrule);
            let created = agent
                .build_chat_config()
                .map_err(|e| e.to_string())
                .and_then(mw_agent::ChatSession::new);
            match created {
                Ok(cs) => *guard = Some(cs),
                Err(e) => {
                    send_json(&tx, serde_json::json!({"type": "error", "error": e})).await;
                    return;
                }
            }
        }
        let session = guard.as_mut().unwrap();
        let rx = session.subscribe();
        let (out, sent_error) = pump_events(&tx, rx, session.send(&req.message)).await;
        match out {
            Ok(text) => {
                send_json(&tx, serde_json::json!({"type": "done", "answer": text})).await;
            }
            Err(e) => {
                if !sent_error {
                    send_json(&tx, serde_json::json!({"type": "error", "error": e})).await;
                }
            }
        }
    });
    sse.into_response()
}

async fn chat_reset_handler(State(s): State<Arc<AppState>>) -> Response {
    let gw = gateway(&s);
    if let Err(r) = check_gate(&gw) {
        return r;
    }
    *s.chat.lock().await = None;
    (StatusCode::OK, Json(serde_json::json!({"reset": true}))).into_response()
}

/// 文件树节点：{name, path, type, children}；排除 .git/.gitkeep；目录排序在前
fn build_tree(root: &Path) -> serde_json::Value {
    tree_node(root, root)
}

fn tree_node(dir: &Path, root: &Path) -> serde_json::Value {
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == ".git" || name == ".gitkeep" {
                continue;
            }
            let path = entry.path();
            let rel = path
                .strip_prefix(root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            match entry.file_type() {
                Ok(t) if t.is_dir() => dirs.push(tree_node(&path, root)),
                Ok(t) if t.is_file() => files.push(
                    serde_json::json!({"name": name, "path": rel, "type": "file"}),
                ),
                _ => {}
            }
        }
    }
    let by_name = |a: &serde_json::Value, b: &serde_json::Value| {
        a["name"].as_str().unwrap_or("").cmp(b["name"].as_str().unwrap_or(""))
    };
    dirs.sort_by(by_name);
    files.sort_by(by_name);
    dirs.extend(files);
    let (name, rel) = if dir == root {
        (String::new(), String::new())
    } else {
        (
            dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            dir.strip_prefix(root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default(),
        )
    };
    serde_json::json!({"name": name, "path": rel, "type": "dir", "children": dirs})
}

/// 防路径穿越：canonicalize 后必须在 work_dir 内且是文件
fn resolve_page(root: &Path, rel: &str) -> Result<PathBuf, Response> {
    let canon_root = root
        .canonicalize()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let rel_path = Path::new(rel);
    if rel_path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir | std::path::Component::RootDir))
    {
        return Err(err(StatusCode::BAD_REQUEST, "非法路径：越出知识库目录"));
    }
    let canon = canon_root
        .join(rel_path)
        .canonicalize()
        .map_err(|_| err(StatusCode::NOT_FOUND, "页面不存在"))?;
    if !canon.starts_with(&canon_root) {
        return Err(err(StatusCode::BAD_REQUEST, "非法路径：越出知识库目录"));
    }
    if !canon.is_file() {
        return Err(err(StatusCode::BAD_REQUEST, "目标不是文件"));
    }
    Ok(canon)
}

/// frontmatter `type:` 行解析（仅文件开头 --- 块内）
fn parse_frontmatter_type(content: &str) -> Option<String> {
    let mut lines = content.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    for line in lines {
        let line = line.trim();
        if line == "---" {
            break;
        }
        if let Some(v) = line.strip_prefix("type:") {
            let v = v.trim().trim_matches('"').trim_matches('\'');
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// 提取正文所有 [[wikilink]] 目标（去 #anchor 和 |text，取 basename stem）
fn wikilink_targets(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = content;
    while let Some(i) = rest.find("[[") {
        rest = &rest[i + 2..];
        let Some(j) = rest.find("]]") else { break };
        let raw = &rest[..j];
        rest = &rest[j + 2..];
        let target = raw.split('#').next().unwrap_or("").split('|').next().unwrap_or("");
        let base = target.rsplit('/').next().unwrap_or("").trim();
        let stem = base.strip_suffix(".md").unwrap_or(base).trim();
        if !stem.is_empty() {
            out.push(stem.to_string());
        }
    }
    out
}

/// 图谱：nodes = index.md + wiki/**/*.md；edges = wikilink（悬空链接跳过）
fn build_graph(root: &Path) -> serde_json::Value {
    let mut files: Vec<PathBuf> = Vec::new();
    let index = root.join("index.md");
    if index.is_file() {
        files.push(index);
    }
    let mut stack = vec![root.join("wiki")];
    while let Some(d) = stack.pop() {
        if let Ok(entries) = std::fs::read_dir(&d) {
            for entry in entries.flatten() {
                let path = entry.path();
                match entry.file_type() {
                    Ok(t) if t.is_dir() => stack.push(path),
                    Ok(t) if t.is_file() => {
                        if path.extension().is_some_and(|e| e == "md") {
                            files.push(path);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    files.sort();

    let rel_of = |p: &Path| {
        p.strip_prefix(root)
            .map(|r| r.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default()
    };
    let mut nodes = Vec::new();
    let mut id_by_stem: HashMap<String, String> = HashMap::new();
    let mut contents: Vec<(String, String)> = Vec::new();
    for f in &files {
        let id = rel_of(f);
        let stem = f
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let content = std::fs::read_to_string(f).unwrap_or_default();
        let ty = if f.file_name().is_some_and(|n| n == "index.md") {
            "index".to_string()
        } else {
            parse_frontmatter_type(&content).unwrap_or_else(|| "page".to_string())
        };
        nodes.push(serde_json::json!({"id": id, "label": stem, "type": ty}));
        id_by_stem.entry(stem).or_insert_with(|| id.clone());
        contents.push((id, content));
    }

    let mut edges = Vec::new();
    for (from, content) in &contents {
        for target in wikilink_targets(content) {
            if let Some(to) = id_by_stem.get(&target) {
                if to != from {
                    edges.push(serde_json::json!({"from": from, "to": to}));
                }
            }
        }
    }
    serde_json::json!({"nodes": nodes, "edges": edges})
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
    let config_path = vault.root.join("appconfig.json");
    let llm = mw_agent::LlmConfig::load_or_env(&config_path);
    Ok(Arc::new(AppState {
        vault,
        gateway: RwLock::new(Arc::new(gateway)),
        admin_token: RwLock::new(admin_token),
        skills_root,
        vault_lock: Arc::new(tokio::sync::Mutex::new(())),
        current_session: tokio::sync::RwLock::new(None),
        llm: RwLock::new(llm),
        config_path,
        chat: tokio::sync::Mutex::new(None),
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

    /// 预写 appconfig.json（假 key + 秒失败的 base_url），init vault 并起服务
    async fn spawn_unlocked_with_fake_llm(
        tmp: &tempfile::TempDir,
    ) -> (Arc<AppState>, String, reqwest::Client) {
        std::fs::write(
            tmp.path().join("appconfig.json"),
            serde_json::json!({
                "llm": {
                    "provider": "openai_compat",
                    "base_url": "http://127.0.0.1:1",
                    "api_key": "sk-dummy-9999",
                    "model": "test-model"
                }
            })
            .to_string(),
        )
        .unwrap();
        let vault = Vault::open(tmp.path()).unwrap();
        let gw = KeyGateway::new().unwrap();
        vault.init(&gw, "pw-A").unwrap();
        gw.close();
        let state = load_state(vault, tmp.path().to_path_buf()).unwrap();
        let base = spawn_server(state.clone()).await;
        let client = reqwest::Client::new();
        let resp = client
            .post(format!("{base}/api/gateway/open"))
            .json(&serde_json::json!({"password": "pw-A"}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        (state, base, client)
    }

    #[tokio::test]
    async fn llm_config_endpoint_gated() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::open(tmp.path()).unwrap();
        let gw = KeyGateway::new().unwrap();
        vault.init(&gw, "pw-A").unwrap();
        gw.close();
        let state = load_state(vault, tmp.path().to_path_buf()).unwrap();
        let base = spawn_server(state.clone()).await;
        let client = reqwest::Client::new();

        // 未解锁 → 423
        let resp = client
            .get(format!("{base}/api/llm/config"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::LOCKED);
        let resp = client
            .post(format!("{base}/api/llm/config"))
            .json(&serde_json::json!({
                "provider": "openai_compat", "base_url": "https://x", "api_key": "k", "model": "m"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::LOCKED);

        // 解锁
        let resp = client
            .post(format!("{base}/api/gateway/open"))
            .json(&serde_json::json!({"password": "pw-A"}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // 字段校验：空 key → 400；非法 provider → 400
        let resp = client
            .post(format!("{base}/api/llm/config"))
            .json(&serde_json::json!({
                "provider": "openai_compat", "base_url": "https://x", "api_key": "", "model": "m"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let resp = client
            .post(format!("{base}/api/llm/config"))
            .json(&serde_json::json!({
                "provider": "bogus", "base_url": "https://x", "api_key": "k", "model": "m"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // 正常保存 → 200 + appconfig.json 落盘（600）+ 热更新
        let resp = client
            .post(format!("{base}/api/llm/config"))
            .json(&serde_json::json!({
                "provider": "openai_compat",
                "base_url": "https://openrouter.ai/api/v1",
                "api_key": "sk-or-abcd1234",
                "model": "deepseek/deepseek-v4-flash"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(tmp.path().join("appconfig.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
        assert_eq!(state.llm.read().unwrap().api_key, "sk-or-abcd1234");

        // GET → masked key（不含完整 key，含尾 4 位）
        let resp = client
            .get(format!("{base}/api/llm/config"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["provider"], "openai_compat");
        assert_eq!(body["base_url"], "https://openrouter.ai/api/v1");
        assert_eq!(body["model"], "deepseek/deepseek-v4-flash");
        let masked = body["api_key_masked"].as_str().unwrap();
        assert!(!masked.contains("abcd"));
        assert!(masked.ends_with("1234"));
        assert!(body.get("api_key").is_none());
    }

    #[tokio::test]
    async fn ingest_stream_endpoint() {
        let tmp = tempfile::tempdir().unwrap();
        let (_state, base, client) = spawn_unlocked_with_fake_llm(&tmp).await;

        // multipart 手动构造（假 key + 不可达 base_url → agent 必失败 → error 事件收尾）
        let boundary = "MWTESTBOUNDARY";
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"test.md\"\r\nContent-Type: text/markdown\r\n\r\n# 测试\n\n[[页面甲]]\n\r\n--{boundary}--\r\n"
        );
        let resp = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            client
                .post(format!("{base}/api/ingest/stream"))
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(body)
                .send(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp.headers()["content-type"].to_str().unwrap().to_string();
        assert!(ct.contains("text/event-stream"), "content-type: {ct}");
        let text = tokio::time::timeout(std::time::Duration::from_secs(120), resp.text())
            .await
            .unwrap()
            .unwrap();
        assert!(text.contains("data: "), "body: {text}");
        assert!(
            text.contains("\"type\":\"done\"") || text.contains("\"type\":\"error\""),
            "body: {text}"
        );
    }

    #[tokio::test]
    async fn chat_requires_unlock() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("appconfig.json"),
            serde_json::json!({
                "llm": {"provider": "openai_compat", "base_url": "http://127.0.0.1:1",
                        "api_key": "sk-dummy-9999", "model": "test-model"}
            })
            .to_string(),
        )
        .unwrap();
        let vault = Vault::open(tmp.path()).unwrap();
        let gw = KeyGateway::new().unwrap();
        vault.init(&gw, "pw-A").unwrap();
        gw.close();
        let state = load_state(vault, tmp.path().to_path_buf()).unwrap();
        let base = spawn_server(state).await;
        let client = reqwest::Client::new();

        let resp = client
            .post(format!("{base}/api/chat"))
            .json(&serde_json::json!({"message": "你好"}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::LOCKED);
        let resp = client
            .post(format!("{base}/api/chat/reset"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::LOCKED);
    }

    #[tokio::test]
    async fn chat_reset_clears() {
        let tmp = tempfile::tempdir().unwrap();
        let (state, base, client) = spawn_unlocked_with_fake_llm(&tmp).await;

        // 塞一个真实聊天会话进去（离线构造：不发起任何网络请求）
        let agent = mw_agent::WikiAgent::with_llm(
            tmp.path(),
            tmp.path(),
            state.llm.read().unwrap().clone(),
        );
        let cfg = agent.build_chat_config().unwrap();
        *state.chat.lock().await = Some(mw_agent::ChatSession::new(cfg).unwrap());
        assert!(state.chat.lock().await.is_some());

        let resp = client
            .post(format!("{base}/api/chat/reset"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(state.chat.lock().await.is_none());
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

    fn write_file(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    #[test]
    fn graph_parsing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_file(root, "index.md", "# 索引\n\n入口：[[概念A]]\n");
        write_file(
            root,
            "wiki/概念A.md",
            "---\ntype: concept\n---\n# 概念A\n\n关联 [[概念B]] 与 [[不存在的页面]]\n",
        );
        write_file(root, "wiki/概念B.md", "# 概念B\n\n回到 [[概念A#小节|概念A]]\n");

        let g = build_graph(root);
        let nodes = g["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 3);
        let ty = |id: &str| {
            nodes
                .iter()
                .find(|n| n["id"] == id)
                .unwrap()["type"]
                .as_str()
                .unwrap()
                .to_string()
        };
        assert_eq!(ty("index.md"), "index");
        assert_eq!(ty("wiki/概念A.md"), "concept");
        assert_eq!(ty("wiki/概念B.md"), "page");

        let edges: Vec<(String, String)> = g["edges"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    e["from"].as_str().unwrap().to_string(),
                    e["to"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        assert!(edges.contains(&("index.md".into(), "wiki/概念A.md".into())));
        assert!(edges.contains(&("wiki/概念A.md".into(), "wiki/概念B.md".into())));
        // [[概念A#小节|概念A]] → basename stem 解析回概念A
        assert!(edges.contains(&("wiki/概念B.md".into(), "wiki/概念A.md".into())));
        // 悬空链接跳过
        assert!(!edges.iter().any(|(_, to)| to.contains("不存在")));
    }

    #[test]
    fn tree_excludes_git() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_file(root, ".git/config", "[core]");
        write_file(root, ".git/refs/heads/main", "abc");
        write_file(root, ".gitkeep", "");
        write_file(root, "index.md", "# 索引");
        write_file(root, "wiki/概念A.md", "# A");

        let tree = build_tree(root);
        let s = tree.to_string();
        assert!(!s.contains(".git"));
        assert!(s.contains("index.md"));
        assert!(s.contains("概念A.md"));
        // 目录排序在前
        let children = tree["children"].as_array().unwrap();
        assert_eq!(children[0]["type"], "dir");
        assert_eq!(children[0]["name"], "wiki");
    }

    #[test]
    fn page_path_traversal_blocked() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_file(root, "wiki/a.md", "hello");

        assert!(resolve_page(root, "wiki/a.md").is_ok());
        let bad = resolve_page(root, "../etc/passwd").unwrap_err();
        assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
        let bad2 = resolve_page(root, "../../etc/passwd").unwrap_err();
        assert_eq!(bad2.status(), StatusCode::BAD_REQUEST);
        let missing = resolve_page(root, "wiki/nope.md").unwrap_err();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn browse_requires_unlock() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::open(tmp.path()).unwrap();
        let gw = Arc::new(KeyGateway::new().unwrap());
        vault.init(&gw, "pw-A").unwrap();
        gw.close();

        let state = load_state(vault, tmp.path().to_path_buf()).unwrap();
        let base = spawn_server(state).await;

        for path in ["/api/wiki/tree", "/api/wiki/graph", "/api/wiki/page?path=index.md"] {
            let resp = reqwest::get(format!("{base}{path}")).await.unwrap();
            assert_eq!(resp.status(), StatusCode::LOCKED, "{path}");
        }
    }

    #[tokio::test]
    async fn open_creates_session_close_destroys() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::open(tmp.path()).unwrap();
        let gw = Arc::new(KeyGateway::new().unwrap());
        vault.init(&gw, "pw-A").unwrap();
        gw.close();

        let state = load_state(vault, tmp.path().to_path_buf()).unwrap();
        let token = state.admin_token.read().unwrap().clone();
        let app = state.clone();
        let base = spawn_server(state).await;
        let client = reqwest::Client::new();

        // 解锁 → 长驻会话创建
        assert!(app.current_session.read().await.is_none());
        let resp = client
            .post(format!("{base}/api/gateway/open"))
            .json(&serde_json::json!({"password": "pw-A"}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let work_dir = {
            let guard = app.current_session.read().await;
            assert!(guard.is_some());
            guard.as_ref().unwrap().work_dir().to_path_buf()
        };
        assert!(work_dir.exists());

        // 会话内写文件 → tree/page 可见（复用同一会话 work_dir）
        write_file(&work_dir, "index.md", "# 首页 [[页面甲]]");
        write_file(&work_dir, "wiki/页面甲.md", "---\ntype: concept\n---\n# 页面甲");
        let tree: serde_json::Value = client
            .get(format!("{base}/api/wiki/tree"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(tree.to_string().contains("页面甲.md"));
        let page: serde_json::Value = client
            .get(format!("{base}/api/wiki/page?path=wiki/页面甲.md"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(page["content"].as_str().unwrap().contains("页面甲"));
        let graph: serde_json::Value = client
            .get(format!("{base}/api/wiki/graph"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(graph["nodes"].as_array().unwrap().len(), 2);
        assert_eq!(graph["edges"].as_array().unwrap().len(), 1);

        // 路径穿越被拦截
        let resp = client
            .get(format!("{base}/api/wiki/page?path=../etc/passwd"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // 锁定 → 会话销毁、临时目录删除、browse 回 423
        let resp = client
            .post(format!("{base}/api/gateway/close"))
            .json(&serde_json::json!({"admin_token": token}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(app.current_session.read().await.is_none());
        assert!(!work_dir.exists());
        let resp = client
            .get(format!("{base}/api/wiki/tree"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::LOCKED);
    }

    /// 内存构造 zip（zip::ZipWriter）
    fn make_zip(files: &[(&str, &str)]) -> Vec<u8> {
        use std::io::Write as _;
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        for (name, content) in files {
            w.start_file(*name, opts).unwrap();
            w.write_all(content.as_bytes()).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    /// multipart 手工构造 zip 上传体
    fn zip_multipart(boundary: &str, zip: &[u8]) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"wiki.zip\"\r\nContent-Type: application/zip\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(zip);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        body
    }

    async fn post_zip(client: &reqwest::Client, base: &str, zip: &[u8]) -> reqwest::Response {
        let boundary = "MWZIPBOUNDARY";
        client
            .post(format!("{base}/api/wiki/import"))
            .header(
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(zip_multipart(boundary, zip))
            .send()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn import_zip_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let (state, base, client) = spawn_unlocked_with_fake_llm(&tmp).await;

        let zip = make_zip(&[
            ("wiki/A.md", "# A\n\n链接到 [[B]]\n"),
            ("wiki/B.md", "# B\n"),
            ("sources/x.md", "# X\n"),
            (".git/junk", "junk"),
            (".obsidian/config", "{}"),
        ]);
        let resp = post_zip(&client, &base, &zip).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["committed"], true);
        let imported: Vec<String> = body["imported"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(imported.len(), 3);
        assert!(imported.contains(&"wiki/A.md".to_string()));
        assert!(imported.contains(&"sources/x.md".to_string()));
        let skipped: Vec<String> = body["skipped"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert!(skipped.contains(&".git/junk".to_string()));
        assert!(skipped.contains(&".obsidian/config".to_string()));

        // 文件树含 A.md / x.md，不含 .git / .obsidian
        let tree: serde_json::Value = client
            .get(format!("{base}/api/wiki/tree"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let s = tree.to_string();
        assert!(s.contains("A.md"));
        assert!(s.contains("x.md"));
        assert!(!s.contains(".git"));
        assert!(!s.contains(".obsidian"));

        // 图谱有 A→B 边
        let graph: serde_json::Value = client
            .get(format!("{base}/api/wiki/graph"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let edges: Vec<(String, String)> = graph["edges"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    e["from"].as_str().unwrap().to_string(),
                    e["to"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        assert!(edges.contains(&("wiki/A.md".into(), "wiki/B.md".into())));

        // git 历史含导入提交
        {
            let guard = state.current_session.read().await;
            let log = guard.as_ref().unwrap().git_log().unwrap();
            assert!(
                log.iter().any(|m| m.contains("Import wiki bundle: 3 files")),
                "git log: {log:?}"
            );
        }

        // 锁定后重开：数据还在
        let token = state.admin_token.read().unwrap().clone();
        let resp = client
            .post(format!("{base}/api/gateway/close"))
            .json(&serde_json::json!({"admin_token": token}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let resp = client
            .post(format!("{base}/api/gateway/open"))
            .json(&serde_json::json!({"password": "pw-A"}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let tree: serde_json::Value = client
            .get(format!("{base}/api/wiki/tree"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let s = tree.to_string();
        assert!(s.contains("A.md"));
        assert!(s.contains("x.md"));
    }

    #[tokio::test]
    async fn import_rejects_path_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let (state, base, client) = spawn_unlocked_with_fake_llm(&tmp).await;

        let zip = make_zip(&[("wiki/ok.md", "# ok\n"), ("../../etc/evil", "evil")]);
        let resp = post_zip(&client, &base, &zip).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(body["error"].as_str().unwrap().contains("etc/evil"));

        // 拒绝后 work_dir 不应出现穿越产物
        let guard = state.current_session.read().await;
        let work = guard.as_ref().unwrap().work_dir();
        assert!(!work.join("etc").exists());
    }

    #[tokio::test]
    async fn import_requires_unlock() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::open(tmp.path()).unwrap();
        let gw = KeyGateway::new().unwrap();
        vault.init(&gw, "pw-A").unwrap();
        gw.close();
        let state = load_state(vault, tmp.path().to_path_buf()).unwrap();
        let base = spawn_server(state).await;
        let client = reqwest::Client::new();

        let zip = make_zip(&[("wiki/A.md", "# A\n")]);
        let resp = post_zip(&client, &base, &zip).await;
        assert_eq!(resp.status(), StatusCode::LOCKED);
    }

    /* ============ Step 8：删除功能 ============ */

    async fn delete_entry(
        client: &reqwest::Client,
        base: &str,
        path: &str,
        mode: &str,
    ) -> reqwest::Response {
        client
            .delete(format!("{base}/api/wiki/entry"))
            .json(&serde_json::json!({"path": path, "mode": mode}))
            .send()
            .await
            .unwrap()
    }

    /// 在当前会话 work_dir 写文件并提交
    async fn seed_files(state: &Arc<AppState>, files: &[(&str, &str)]) {
        let guard = state.current_session.read().await;
        let session = guard.as_ref().unwrap();
        let work = session.work_dir().to_path_buf();
        for (rel, content) in files {
            write_file(&work, rel, content);
        }
        session.git_commit("seed").unwrap();
    }

    #[tokio::test]
    async fn delete_quick_removes_and_commits() {
        let tmp = tempfile::tempdir().unwrap();
        let (state, base, client) = spawn_unlocked_with_fake_llm(&tmp).await;
        seed_files(
            &state,
            &[
                ("index.md", "# 索引 [[A]]"),
                ("wiki/A.md", "# A\n\n链接 [[B]]\n"),
                ("wiki/B.md", "# B\n"),
            ],
        )
        .await;

        let resp = delete_entry(&client, &base, "wiki/A.md", "quick").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["deleted"], true);
        assert_eq!(body["files_removed"], 1);
        assert!(body.get("degraded").is_none());

        // 文件树无此文件，B 还在
        let tree: serde_json::Value = client
            .get(format!("{base}/api/wiki/tree"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let s = tree.to_string();
        assert!(!s.contains("A.md"), "tree: {s}");
        assert!(s.contains("B.md"));

        // git log 有 Delete 提交
        {
            let guard = state.current_session.read().await;
            let log = guard.as_ref().unwrap().git_log().unwrap();
            assert!(
                log.iter().any(|m| m.contains("Delete: wiki/A.md")),
                "git log: {log:?}"
            );
        }

        // 锁定重开：数据还是删了
        let token = state.admin_token.read().unwrap().clone();
        let resp = client
            .post(format!("{base}/api/gateway/close"))
            .json(&serde_json::json!({"admin_token": token}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let resp = client
            .post(format!("{base}/api/gateway/open"))
            .json(&serde_json::json!({"password": "pw-A"}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let tree: serde_json::Value = client
            .get(format!("{base}/api/wiki/tree"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let s = tree.to_string();
        assert!(!s.contains("A.md"), "tree after reopen: {s}");
        assert!(s.contains("B.md"));
    }

    #[tokio::test]
    async fn delete_protected_paths_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let (_state, base, client) = spawn_unlocked_with_fake_llm(&tmp).await;
        for path in ["index.md", "schema.md", "log.md"] {
            let resp = delete_entry(&client, &base, path, "quick").await;
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{path}");
            let body: serde_json::Value = resp.json().await.unwrap();
            assert!(body["error"].as_str().unwrap().contains("不可删除"));
        }
    }

    #[tokio::test]
    async fn delete_requires_unlock() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::open(tmp.path()).unwrap();
        let gw = KeyGateway::new().unwrap();
        vault.init(&gw, "pw-A").unwrap();
        gw.close();
        let state = load_state(vault, tmp.path().to_path_buf()).unwrap();
        let base = spawn_server(state).await;
        let client = reqwest::Client::new();

        let resp = delete_entry(&client, &base, "wiki/A.md", "quick").await;
        assert_eq!(resp.status(), StatusCode::LOCKED);
    }

    #[tokio::test]
    async fn delete_smart_degrades_without_llm() {
        std::env::remove_var("MW_LLM_API_KEY");
        let tmp = tempfile::tempdir().unwrap();
        // 不写 appconfig.json：无 LLM 配置
        let vault = Vault::open(tmp.path()).unwrap();
        let gw = KeyGateway::new().unwrap();
        vault.init(&gw, "pw-A").unwrap();
        gw.close();
        let state = load_state(vault, tmp.path().to_path_buf()).unwrap();
        let base = spawn_server(state.clone()).await;
        let client = reqwest::Client::new();
        let resp = client
            .post(format!("{base}/api/gateway/open"))
            .json(&serde_json::json!({"password": "pw-A"}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        seed_files(
            &state,
            &[("index.md", "# 索引"), ("wiki/A.md", "# A\n")],
        )
        .await;

        let resp = delete_entry(&client, &base, "wiki/A.md", "smart").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["deleted"], true);
        assert_eq!(body["files_removed"], 1);
        assert_eq!(body["degraded"], true);
        assert!(body.get("answer").is_none());

        // 确实删掉了
        let tree: serde_json::Value = client
            .get(format!("{base}/api/wiki/tree"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(!tree.to_string().contains("A.md"));
    }

    #[tokio::test]
    async fn delete_path_traversal_blocked() {
        let tmp = tempfile::tempdir().unwrap();
        let (state, base, client) = spawn_unlocked_with_fake_llm(&tmp).await;
        seed_files(&state, &[("wiki/A.md", "# A\n")]).await;

        for path in ["../etc/passwd", "../../etc/passwd", "/etc/passwd"] {
            let resp = delete_entry(&client, &base, path, "quick").await;
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{path}");
        }
        // work_dir 未被破坏
        let guard = state.current_session.read().await;
        let work = guard.as_ref().unwrap().work_dir();
        assert!(work.join("wiki/A.md").exists());
    }
}
