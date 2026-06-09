use anyhow::{Context, Result};
use sqlx::SqlitePool;
use std::env;
use tracing::info;

use crate::auth::password::hash_password;
use crate::models::user::User;

/// 启动时保证至少存在一个 admin。已有 admin 则跳过；
/// 否则用 PANEL_BOOTSTRAP_ADMIN_USERNAME (默认 "admin") +
/// PANEL_BOOTSTRAP_ADMIN_PASSWORD (必需) 创建一个。
pub async fn ensure_admin_user(pool: &SqlitePool) -> Result<()> {
    let count = User::count_admins(pool).await.context("count admins")?;
    if count > 0 {
        return Ok(());
    }
    let username =
        env::var("PANEL_BOOTSTRAP_ADMIN_USERNAME").unwrap_or_else(|_| "admin".to_string());
    let password = env::var("PANEL_BOOTSTRAP_ADMIN_PASSWORD").context(
        "no admin exists; set PANEL_BOOTSTRAP_ADMIN_PASSWORD to seed the first admin user",
    )?;
    if password.is_empty() {
        anyhow::bail!("PANEL_BOOTSTRAP_ADMIN_PASSWORD is empty; refusing to create admin");
    }
    let hash = hash_password(&password).context("hash bootstrap admin password")?;
    User::create(pool, &username, &hash, "admin", None, None)
        .await
        .context("insert bootstrap admin")?;
    info!(username = %username, "bootstrap admin user created");
    Ok(())
}

/// 对历史 DB(在新 key 加入之前迁过)兜底插入默认设置。
/// 不覆盖管理员已设值,使用 INSERT OR IGNORE。
pub async fn seed_default_settings(pool: &SqlitePool) -> Result<()> {
    let defaults: &[(&str, &str)] = &[
        ("agent_control_endpoint", ""),
    ];
    for (k, v) in defaults {
        sqlx::query("INSERT OR IGNORE INTO system_settings (key, value) VALUES (?, ?)")
            .bind(k)
            .bind(v)
            .execute(pool)
            .await
            .with_context(|| format!("seed default setting {k}"))?;
    }
    Ok(())
}
