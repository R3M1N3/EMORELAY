use anyhow::{Context, Result};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
    SqlitePool,
};
use std::str::FromStr;

pub async fn connect(database_url: &str) -> Result<SqlitePool> {
    ensure_parent_dir(database_url)?;

    let opts = SqliteConnectOptions::from_str(database_url)
        .with_context(|| format!("invalid PANEL_DATABASE_URL: {database_url}"))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(opts)
        .await
        .context("failed to connect to database")?;

    Ok(pool)
}

pub async fn run_migrations(pool: &SqlitePool) -> Result<()> {
    sqlx::migrate!("../../migrations")
        .run(pool)
        .await
        .context("migrations failed")?;
    Ok(())
}

/// 提取 sqlite:// 后的文件路径，递归创建父目录（如 ./data/emorelay.db 的 ./data）。
/// 排除 :memory: 与无父目录的情况。
fn ensure_parent_dir(database_url: &str) -> Result<()> {
    let Some(path) = database_url
        .strip_prefix("sqlite://")
        .or_else(|| database_url.strip_prefix("sqlite:"))
    else {
        return Ok(());
    };
    if path.starts_with(':') {
        return Ok(());
    }
    let path = std::path::Path::new(path);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create db parent dir: {}", parent.display()))?;
        }
    }
    Ok(())
}
