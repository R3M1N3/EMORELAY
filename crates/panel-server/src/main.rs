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

    let state = AppState {
        config: config.clone(),
        pool,
        sessions: Arc::new(SessionRegistry::new()),
        dispatcher: Arc::new(CommandDispatcher::new()),
    };

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

    // 周期扫 expired 规则 (即使没有 stats 上报也能兜底)。任务在 select! 外 spawn,
    // 当 http/grpc 任一退出时整个进程关停,sweep task 随 tokio runtime 一起 drop。
    grpc::service::spawn_expiry_sweeper(state.clone());

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
