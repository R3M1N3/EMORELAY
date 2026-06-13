use anyhow::{Context, Result};
use axum::http::{header, HeaderValue, Method};
use panel_server::{
    auth, bootstrap, config::Config, db, grpc,
    grpc::{dispatcher::CommandDispatcher, session::SessionRegistry},
    routes, state::AppState,
};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::signal;
use tower_http::cors::CorsLayer;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let config = Config::from_env()?;
    info!(http = %config.bind_addr, grpc = %config.grpc_bind_addr, "panel-server starting");

    let pool = db::connect(&config.database_url).await?;
    db::run_migrations(&pool).await?;
    bootstrap::ensure_admin_user(&pool).await?;
    bootstrap::seed_default_settings(&pool).await?;
    let _ = auth::password::dummy_hash();
    info!("database ready");

    let dist_dir = std::path::PathBuf::from(&config.panel_data_dir).join("agent-dist");
    if let Err(e) = tokio::fs::create_dir_all(&dist_dir).await {
        tracing::warn!(error = ?e, path = ?dist_dir, "failed to ensure agent-dist dir");
    }

    let tls_dir = std::path::PathBuf::from(&config.panel_data_dir)
        .join("tls")
        .display()
        .to_string();
    let ca = panel_server::tls::ca::bootstrap_ca(&tls_dir, config.panel_public_host.as_deref())?;
    let crl_path = format!("{tls_dir}/crl.json");
    let crl = std::sync::Arc::new(panel_server::tls::crl::Crl::load(&crl_path));
    info!(mtls = !config.dev_disable_mtls, "tls ready");

    let state = AppState {
        config: config.clone(),
        pool,
        sessions: Arc::new(SessionRegistry::new()),
        dispatcher: Arc::new(CommandDispatcher::new()),
        ca,
        crl,
        node_events: Arc::new(tokio::sync::broadcast::channel(256).0),
    };

    // P3a 存量迁移:活跃但无证书的节点(P1/P2 创建)自动签发 client cert。
    // 管理员需到面板「轮换凭据」拿明文重装 Agent(升级 P3a = fleet-wide 重装)。
    match panel_server::models::node::Node::find_active_without_cert(&state.pool).await {
        Ok(ids) => {
            for nid in ids {
                if let Ok(issued) = panel_server::tls::issue::issue_client_cert(&state.ca, nid) {
                    let _ = panel_server::models::node::Node::set_cert_meta(
                        &state.pool,
                        nid,
                        &issued.serial,
                        &issued.fingerprint,
                    )
                    .await;
                    panel_server::audit::record(
                        &state.pool,
                        None,
                        "node.mtls_credentials_issued",
                        Some("node"),
                        Some(nid),
                        None,
                        true,
                        None,
                    )
                    .await;
                    tracing::warn!(
                        node_id = nid,
                        "issued mTLS cert for legacy node; rotate to get plaintext for reinstall"
                    );
                }
            }
        }
        Err(e) => tracing::warn!(error = ?e, "legacy node cert migration query failed"),
    }

    let cors_origin: HeaderValue = config
        .cors_origin
        .parse()
        .with_context(|| format!("invalid PANEL_CORS_ORIGIN: {}", config.cors_origin))?;
    let cors = CorsLayer::new()
        .allow_origin(cors_origin)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION]);

    let http_app = routes::router(state.clone()).layer(cors);
    let http_listener = TcpListener::bind(&config.bind_addr)
        .await
        .with_context(|| format!("failed to bind http {}", config.bind_addr))?;
    info!(addr = %config.bind_addr, "http listening");

    let grpc_addr: SocketAddr = config
        .grpc_bind_addr
        .parse()
        .with_context(|| format!("invalid PANEL_GRPC_BIND_ADDR: {}", config.grpc_bind_addr))?;

    // into_make_service_with_connect_info 让 ActorIp extractor 在没有反代 header 时
    // 能拿到 TCP 对端 IP(直连场景),反代场景仍优先看 X-Real-IP / X-Forwarded-For。
    let make_service = http_app.into_make_service_with_connect_info::<SocketAddr>();
    let http_task = tokio::spawn(async move {
        axum::serve(http_listener, make_service)
            .with_graceful_shutdown(shutdown_signal())
            .await
            .context("http server")
    });
    let grpc_state = state.clone();
    let grpc_task = tokio::spawn(async move { grpc::serve(grpc_state, grpc_addr).await });

    // 用户级到期(60s)与 30 天配额(300s)双 tick sweeper;随 tokio runtime 一起 drop。
    panel_server::sweeper::user_quota::spawn_user_quota_sweeper(state.clone());
    // 时序统计保留清理(默认每小时,按 stats_retention_days 删旧分钟桶)。
    panel_server::sweeper::stats_retention::spawn_stats_retention_sweeper(state.clone());
    // 节点掉线检测(默认 30s 扫一次,心跳超 120s 置 offline 并发 webhook)。
    panel_server::sweeper::node_offline::spawn_node_offline_sweeper(state.clone());
    // 隧道凭据轮换(默认每小时扫,签发超 20 天重签下发并重启隧道规则)。
    panel_server::sweeper::tunnel_creds::spawn_tunnel_creds_sweeper(state.clone());

    tokio::select! {
        res = http_task => match res {
            Ok(Ok(())) => info!("http server stopped"),
            Ok(Err(e)) => error!(error = ?e, "http server crashed"),
            Err(e) => error!(error = ?e, "http task join error"),
        },
        res = grpc_task => match res {
            Ok(Ok(())) => info!("grpc server stopped"),
            Ok(Err(e)) => error!(error = ?e, "grpc server crashed"),
            Err(e) => error!(error = ?e, "grpc task join error"),
        },
    }

    info!("panel-server stopped");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => info!("SIGINT received"),
        _ = terminate => info!("SIGTERM received"),
    }
}
