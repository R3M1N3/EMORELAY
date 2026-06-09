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
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
}

impl User {
    pub async fn find_by_username(pool: &SqlitePool, username: &str) -> sqlx::Result<Option<Self>> {
        sqlx::query_as::<_, User>(
            "SELECT id, username, password_hash, role, expires_at, traffic_limit_bytes_30d, \
                 period_used_bytes_cached, period_used_calculated_at, \
                 created_at, updated_at, deleted_at \
             FROM users WHERE username = ? AND deleted_at IS NULL",
        )
        .bind(username)
        .fetch_optional(pool)
        .await
    }

    pub async fn find_by_id(pool: &SqlitePool, id: i64) -> sqlx::Result<Option<Self>> {
        sqlx::query_as::<_, User>(
            "SELECT id, username, password_hash, role, expires_at, traffic_limit_bytes_30d, \
                 period_used_bytes_cached, period_used_calculated_at, \
                 created_at, updated_at, deleted_at \
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
    ) -> sqlx::Result<i64> {
        let res = sqlx::query(
            "INSERT INTO users (username, password_hash, role, expires_at, traffic_limit_bytes_30d) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(username)
        .bind(password_hash)
        .bind(role)
        .bind(expires_at)
        .bind(traffic_limit_bytes_30d)
        .execute(pool)
        .await?;
        Ok(res.last_insert_rowid())
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
                 period_used_bytes_cached, period_used_calculated_at, \
                 created_at, updated_at, deleted_at \
             FROM users WHERE deleted_at IS NULL \
             ORDER BY id DESC LIMIT ? OFFSET ?",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await
    }

    pub async fn count(pool: &SqlitePool) -> sqlx::Result<i64> {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM users WHERE deleted_at IS NULL",
        )
        .fetch_one(pool)
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
        let res = sqlx::query(
            "UPDATE users SET \
                password_hash = COALESCE(?1, password_hash), \
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
