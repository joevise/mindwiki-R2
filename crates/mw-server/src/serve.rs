//! Web 服务：内嵌单页界面 + 密钥闸门远程管理 + vault 操作 API + Wiki 浏览器。
//!
//! GET  /                     → 内嵌 webui.html（单页界面）
//! POST /api/gateway/open     {password}      → 开闸 + 创建长驻解密会话
//! POST /api/gateway/close    {admin_token}   → 一键远程关闭 + 销毁解密会话
//! GET  /api/gateway/state                    → {state, closed_at, active_sessions}
//! POST /api/vault/init       {password}      → 创建 vault（已存在 409）
//! GET  /api/vault/status                     → {exists, size, state: sealed|open}
//! POST /api/ingest           multipart .md   → 复用解密会话 + Agent 入库 + 即时封印
//! POST /api/query            {question}      → 复用解密会话 + Agent 查询（有变更才封印）
//! GET  /api/wiki/tree                        → 文件树 JSON（排除 .git）
//! GET  /api/wiki/page?path=                  → {path, content}（防路径穿越）
//! GET  /api/wiki/graph                       → {nodes, edges}（frontmatter type + wikilink）

use anyhow::{bail, Context, Result};
use axum::{
    extract::{Multipart, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
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
        .route("/api/wiki/tree", get(tree_handler))
        .route("/api/wiki/page", get(page_handler))
        .route("/api/wiki/graph", get(graph_handler))
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

    let agent = mw_agent::WikiAgent::new(&s.skills_root, &work);
    let prompt = format!(
        "将上传文件 {rel}（原始文件名 {filename}）入库到知识库，work_dir 即知识库根目录。规则：先检查根目录是否有 index.md——若无（全新知识库），先用 wiki-init 技能初始化 Wiki，然后用 wiki-ingest 技能入库该文件；若已有则直接 wiki-ingest。不要向用户提问确认，直接执行到底。完成后简述初始化与入库结果（建了哪些页面/类型）。"
    );
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
    let guard = match ensure_session(&s).await {
        Ok(g) => g,
        Err(r) => return r,
    };
    let session = guard.as_ref().unwrap();
    let work = session.work_dir().to_path_buf();
    let before = snapshot(&work);

    let agent = mw_agent::WikiAgent::new(&s.skills_root, &work);
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
    Ok(Arc::new(AppState {
        vault,
        gateway: RwLock::new(Arc::new(gateway)),
        admin_token: RwLock::new(admin_token),
        skills_root,
        vault_lock: Arc::new(tokio::sync::Mutex::new(())),
        current_session: tokio::sync::RwLock::new(None),
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
}
