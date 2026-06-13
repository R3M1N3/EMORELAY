// 集成测试共享 helper。每个 tests/*.rs 顶部 `mod common;` 引入。
// cargo 不会把 tests/common/ 当独立 test binary(只有顶层 .rs 才是),所以无 warning。

use anyhow::Result;
use axum::body::{to_bytes, Body};
use axum::http::{Method, Request, Response, StatusCode};
use axum::Router;
use panel_server::{
    auth::password::hash_password,
    config::Config,
    db,
    grpc::{dispatcher::CommandDispatcher, session::SessionRegistry},
    models::user::User,
    routes,
    state::AppState,
};
use serde_json::Value;
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

pub struct TestApp {
    pub state: AppState,
    pub app: Router,
    pub admin_token: String,
    pub admin_user_id: i64,
    // 测试退出时 TempDir drop 删 db 文件;字段持有保持生命周期。
    _temp: TempDir,
}

pub async fn make_app() -> Result<TestApp> {
    // 每个测试一个独立 tempdir + 文件 sqlite,避免 in-memory 在 pool 多连接下各自隔离的坑。
    let temp = tempfile::tempdir()?;
    let db_path = temp.path().join("emorelay-test.db");
    // SQLite URL 在 Windows 上的反斜杠需转正斜杠,sqlx URL parser 才认。
    let url = format!(
        "sqlite://{}",
        db_path.display().to_string().replace('\\', "/")
    );

    let pool = db::connect(&url).await?;
    db::run_migrations(&pool).await?;

    let config = Config {
        bind_addr: "0.0.0.0:0".into(),
        database_url: url,
        jwt_secret: "test-jwt-secret-must-be-at-least-32-chars-long".into(),
        jwt_expiry_hours: 24,
        cors_origin: "http://localhost".into(),
        grpc_bind_addr: "0.0.0.0:0".into(),
        grpc_tls_cert: None,
        grpc_tls_key: None,
        grpc_tls_client_ca: None,
        panel_data_dir: temp.path().display().to_string().replace('\\', "/"),
        panel_public_base_url: None,
        dev_disable_mtls: true,
        panel_public_host: None,
    };
    let tls_dir = format!("{}/tls", temp.path().display().to_string().replace('\\', "/"));
    let ca = panel_server::tls::ca::bootstrap_ca(&tls_dir, None)?;
    let crl = std::sync::Arc::new(panel_server::tls::crl::Crl::new());
    let state = AppState {
        config,
        pool: pool.clone(),
        sessions: Arc::new(SessionRegistry::new()),
        dispatcher: Arc::new(CommandDispatcher::new()),
        ca,
        crl,
        node_events: Arc::new(tokio::sync::broadcast::channel(256).0),
    };

    // 直接创建 admin(跳过 bootstrap 的 env 依赖)。
    let hash = hash_password("admin-test-password")?;
    let admin_user_id = User::create(&pool, "admin", &hash, "admin", None, None, false).await?;

    let app = routes::router(state.clone());
    let admin_token = login(&app, "admin", "admin-test-password").await?;

    Ok(TestApp {
        state,
        app,
        admin_token,
        admin_user_id,
        _temp: temp,
    })
}

/// 直接在 DB 里建一个普通用户(跳过 /api/users 端点的 admin-only 限制),返回 token。
/// 仅供 F8 user-role 测试使用。
pub async fn make_user_token(
    app: &TestApp,
    username: &str,
    password: &str,
) -> Result<(i64, String)> {
    let hash = hash_password(password)?;
    let user_id = User::create(&app.state.pool, username, &hash, "user", None, None, false).await?;
    let token = login(&app.app, username, password).await?;
    Ok((user_id, token))
}

/// P7: 直接写授权表(绕过 admin API)。common 按 test binary 各自编译,部分 binary 不用会
/// 报 dead_code,显式 allow。
#[allow(dead_code)]
pub async fn grant_node(app: &TestApp, user_id: i64, node_id: i64) {
    sqlx::query("INSERT INTO user_node_grants (user_id, node_id) VALUES (?, ?)")
        .bind(user_id)
        .bind(node_id)
        .execute(&app.state.pool)
        .await
        .unwrap();
}

#[allow(dead_code)]
pub async fn grant_tunnel(app: &TestApp, user_id: i64, tunnel_id: i64) {
    sqlx::query("INSERT INTO user_tunnel_grants (user_id, tunnel_id) VALUES (?, ?)")
        .bind(user_id)
        .bind(tunnel_id)
        .execute(&app.state.pool)
        .await
        .unwrap();
}

async fn login(app: &Router, username: &str, password: &str) -> Result<String> {
    let body = serde_json::json!({ "username": username, "password": password });
    // login 路由带 per-IP governor;oneshot 无 ConnectInfo,靠转发头喂 IP。
    let req = Request::post("/api/auth/login")
        .header("content-type", "application/json")
        .header("x-forwarded-for", "127.0.0.1")
        .body(Body::from(serde_json::to_vec(&body)?))?;
    let (status, value) = send(app.clone(), req).await?;
    assert_eq!(status, StatusCode::OK, "login failed: {value}");
    Ok(value["token"]
        .as_str()
        .expect("missing token in login response")
        .to_string())
}

pub async fn send(app: Router, req: Request<Body>) -> Result<(StatusCode, Value)> {
    let resp: Response<Body> = app.oneshot(req).await?;
    let status = resp.status();
    // 测试场景下 body 不会超过 1MiB。
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await?;
    let value: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    Ok((status, value))
}

/// 同 send,但额外返回响应头(测试需要校验自定义 header 时用)。
#[allow(dead_code)]
pub async fn send_with_headers(
    app: Router,
    req: Request<Body>,
) -> Result<(StatusCode, axum::http::HeaderMap, Value)> {
    let resp: Response<Body> = app.oneshot(req).await?;
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await?;
    let value: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    Ok((status, headers, value))
}

pub fn auth_req(
    method: Method,
    path: &str,
    token: &str,
    body: Option<Value>,
) -> Result<Request<Body>> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {token}"));
    let body = match body {
        Some(v) => {
            builder = builder.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&v)?)
        }
        None => Body::empty(),
    };
    Ok(builder.body(body)?)
}
