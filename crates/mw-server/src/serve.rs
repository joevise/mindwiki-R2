//! Web 服务骨架：密钥闸门远程管理端点（Step 4 扩展为完整 Web 界面）。
//!
//! POST /api/gateway/open   {password}     → 开闸（主密码，更强保护）
//! POST /api/gateway/close  {admin_token}  → 一键远程关闭（admin token 防 DoS）
//! GET  /api/gateway/state                 → {state, closed_at, active_sessions}

use anyhow::{bail, Context, Result};
use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use mw_crypto::{GatewayState, KeyGateway};
use mw_store::Vault;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub struct AppState {
    pub gateway: KeyGateway,
    pub admin_token: String,
}

#[derive(Deserialize)]
struct OpenRequest {
    password: String,
}

#[derive(Deserialize)]
struct CloseRequest {
    admin_token: String,
}

#[derive(Serialize)]
struct StateResponse {
    state: &'static str,
    closed_at: Option<String>,
    active_sessions: usize,
}

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/gateway/open", post(open_handler))
        .route("/api/gateway/close", post(close_handler))
        .route("/api/gateway/state", get(state_handler))
        .with_state(state)
}

async fn open_handler(
    State(s): State<Arc<AppState>>,
    Json(req): Json<OpenRequest>,
) -> impl IntoResponse {
    match s.gateway.open(&req.password) {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"state": "open"}))).into_response(),
        Err(e) => (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

async fn close_handler(
    State(s): State<Arc<AppState>>,
    Json(req): Json<CloseRequest>,
) -> impl IntoResponse {
    if req.admin_token != s.admin_token {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "invalid admin token"})),
        )
            .into_response();
    }
    s.gateway.close();
    tracing::warn!("gateway closed remotely — all sessions terminated, keys zeroized");
    (
        StatusCode::OK,
        Json(serde_json::json!({"state": "closed"})),
    )
        .into_response()
}

async fn state_handler(State(s): State<Arc<AppState>>) -> Json<StateResponse> {
    let state = match s.gateway.state() {
        GatewayState::Open => "open",
        GatewayState::Closed => "closed",
    };
    let closed_at = s.gateway.closed_at.lock().unwrap().map(|i| format!("{i:?}"));
    Json(StateResponse {
        state,
        closed_at,
        active_sessions: s.gateway.active_sessions(),
    })
}

/// 从磁盘容器构建服务状态：gateway 初始为关闭态（等 /api/gateway/open 解锁）
pub fn load_state(vault: &Vault) -> Result<Arc<AppState>> {
    let data = std::fs::read(vault.container_path()).context("read vault container")?;
    let c = mw_store::container::decode(&data)?;
    let gateway = KeyGateway::from_container(c.salt, c.verify_token);
    let admin_token = vault.ensure_admin_token()?;
    Ok(Arc::new(AppState {
        gateway,
        admin_token,
    }))
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

    #[tokio::test]
    async fn remote_close_endpoint() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::open(tmp.path()).unwrap();
        let gw = KeyGateway::new().unwrap();
        vault.init(&gw, "pw-A").unwrap();
        gw.close();

        let state = load_state(&vault).unwrap();
        let token = state.admin_token.clone();
        assert_eq!(state.gateway.state(), GatewayState::Closed);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve(listener, state));
        let client = reqwest::Client::new();
        let base = format!("http://{addr}");

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
}
