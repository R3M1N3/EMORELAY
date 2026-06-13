use sqlx::{prelude::FromRow, SqlitePool};

#[derive(Debug, Clone, FromRow)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub password_hash: String,
    pub role: String,
    pub expires_at: Option<String>,
    pub traffic_limit_bytes_30d: Option<i64>,
    pub period_used_bytes_cached: i64,
    pub period_used_calculated_at: Option<String>,
    /// 首登强制改密标志:1 = 下次登录前必须改密(admin 新建/重置时置位)。
    pub must_change_password: i64,
    /// 月度重置日(1-31);NULL = 滚动 30 天窗口。
    pub quota_reset_day: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
}

impl User {
    pub async fn find_by_username(pool: &SqlitePool, username: &str) -> sqlx::Result<Option<Self>> {
        sqlx::query_as::<_, User>(
            "SELECT id, username, password_hash, role, expires_at, traffic_limit_bytes_30d, \
                 period_used_bytes_cached, period_used_calculated_at, must_change_password, \
                 quota_reset_day, created_at, updated_at, deleted_at \
             FROM users WHERE username = ? AND deleted_at IS NULL",
        )
        .bind(username)
        .fetch_optional(pool)
        .await
    }

    pub async fn find_by_id(pool: &SqlitePool, id: i64) -> sqlx::Result<Option<Self>> {
        sqlx::query_as::<_, User>(
            "SELECT id, username, password_hash, role, expires_at, traffic_limit_bytes_30d, \
                 period_used_bytes_cached, period_used_calculated_at, must_change_password, \
                 quota_reset_day, created_at, updated_at, deleted_at \
             FROM users WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(id)
        .fetch_optional(pool)
        .await
    }

    pub async fn create(
        pool: &SqlitePool,
        username: &str,
        password_hash: &str,
        role: &str,
        expires_at: Option<&str>,
        traffic_limit_bytes_30d: Option<i64>,
        must_change_password: bool,
    ) -> sqlx::Result<i64> {
        let res = sqlx::query(
            "INSERT INTO users \
                 (username, password_hash, role, expires_at, traffic_limit_bytes_30d, must_change_password) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(username)
        .bind(password_hash)
        .bind(role)
        .bind(expires_at)
        .bind(traffic_limit_bytes_30d)
        .bind(i64::from(must_change_password))
        .execute(pool)
        .await?;
        Ok(res.last_insert_rowid())
    }

    /// 自助改密成功后清除强制改密标志(同时写入新 hash)。
    /// 单独成方法而非走 update():改密是用户自助路径,与 admin 的 update 语义不同。
    pub async fn change_password_self(
        pool: &SqlitePool,
        id: i64,
        new_password_hash: &str,
    ) -> sqlx::Result<u64> {
        let res = sqlx::query(
            "UPDATE users SET password_hash = ?, must_change_password = 0, \
                 updated_at = datetime('now') \
             WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(new_password_hash)
        .bind(id)
        .execute(pool)
        .await?;
        Ok(res.rows_affected())
    }

    pub async fn count_admins(pool: &SqlitePool) -> sqlx::Result<i64> {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM users WHERE role = 'admin' AND deleted_at IS NULL",
        )
        .fetch_one(pool)
        .await
    }

    pub async fn list_paged(
        pool: &SqlitePool,
        limit: i64,
        offset: i64,
    ) -> sqlx::Result<Vec<Self>> {
        sqlx::query_as::<_, User>(
            "SELECT id, username, password_hash, role, expires_at, traffic_limit_bytes_30d, \
                 period_used_bytes_cached, period_used_calculated_at, must_change_password, \
                 quota_reset_day, created_at, updated_at, deleted_at \
             FROM users WHERE deleted_at IS NULL \
             ORDER BY id DESC LIMIT ? OFFSET ?",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await
    }

    /// 部分更新:None 字段不变,Some 字段写入。updated_at 由本方法刷新。
    /// 置空协议:expires_at 传 "" 清除;traffic_limit_bytes_30d 传 0 清除。
    pub async fn update(
        pool: &SqlitePool,
        id: i64,
        password_hash: Option<&str>,
        role: Option<&str>,
        expires_at: Option<&str>,
        traffic_limit_bytes_30d: Option<i64>,
    ) -> sqlx::Result<u64> {
        // admin 重置密码(?1 非空)时一并置 must_change_password=1:admin 设的是临时密码,
        // 强制用户首登改成自己的;其余字段更新不动该标志。
        let res = sqlx::query(
            "UPDATE users SET \
                password_hash = COALESCE(?1, password_hash), \
                must_change_password = CASE WHEN ?1 IS NULL THEN must_change_password ELSE 1 END, \
                role = COALESCE(?2, role), \
                expires_at = CASE \
                    WHEN ?3 IS NULL THEN expires_at \
                    WHEN ?3 = '' THEN NULL \
                    ELSE ?3 END, \
                traffic_limit_bytes_30d = CASE \
                    WHEN ?4 IS NULL THEN traffic_limit_bytes_30d \
                    WHEN ?4 = 0 THEN NULL \
                    ELSE ?4 END, \
                updated_at = datetime('now') \
             WHERE id = ?5 AND deleted_at IS NULL",
        )
        .bind(password_hash)
        .bind(role)
        .bind(expires_at)
        .bind(traffic_limit_bytes_30d)
        .bind(id)
        .execute(pool)
        .await?;
        Ok(res.rows_affected())
    }

    /// 设置/清除月度重置日。day=Some(1..=31) 启用月度模式;day=None 回滚动 30 天。
    pub async fn set_quota_reset_day(
        pool: &SqlitePool,
        id: i64,
        day: Option<i64>,
    ) -> sqlx::Result<u64> {
        let res = sqlx::query(
            "UPDATE users SET quota_reset_day = ?, updated_at = datetime('now') \
             WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(day)
        .bind(id)
        .execute(pool)
        .await?;
        Ok(res.rows_affected())
    }

    pub async fn soft_delete(pool: &SqlitePool, id: i64) -> sqlx::Result<u64> {
        let res = sqlx::query(
            "UPDATE users SET deleted_at = datetime('now'), updated_at = datetime('now') \
             WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(id)
        .execute(pool)
        .await?;
        Ok(res.rows_affected())
    }

    /// 排除指定 id 后还剩多少 admin。用于在删除 / 改 role 前确保至少留一个 admin。
    pub async fn count_admins_excluding(pool: &SqlitePool, exclude_id: i64) -> sqlx::Result<i64> {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM users \
             WHERE role = 'admin' AND deleted_at IS NULL AND id != ?",
        )
        .bind(exclude_id)
        .fetch_one(pool)
        .await
    }
}
