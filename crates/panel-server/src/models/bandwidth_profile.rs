use sqlx::{prelude::FromRow, SqlitePool};

#[derive(Debug, Clone, FromRow)]
pub struct BandwidthProfile {
    pub id: i64,
    pub name: String,
    pub bandwidth_mbps: i64,
    pub description: String,
    pub created_at: String,
    pub updated_at: String,
}

const COLUMNS: &str = "id, name, bandwidth_mbps, description, created_at, updated_at";

impl BandwidthProfile {
    pub async fn list_paged(pool: &SqlitePool, limit: i64, offset: i64) -> sqlx::Result<Vec<Self>> {
        let sql = format!(
            "SELECT {COLUMNS} FROM bandwidth_profiles WHERE deleted_at IS NULL \
             ORDER BY id DESC LIMIT ? OFFSET ?"
        );
        sqlx::query_as(&sql).bind(limit).bind(offset).fetch_all(pool).await
    }

    pub async fn count(pool: &SqlitePool) -> sqlx::Result<i64> {
        sqlx::query_scalar("SELECT COUNT(*) FROM bandwidth_profiles WHERE deleted_at IS NULL")
            .fetch_one(pool)
            .await
    }

    pub async fn find_by_id(pool: &SqlitePool, id: i64) -> sqlx::Result<Option<Self>> {
        let sql = format!(
            "SELECT {COLUMNS} FROM bandwidth_profiles WHERE id = ? AND deleted_at IS NULL"
        );
        sqlx::query_as(&sql).bind(id).fetch_optional(pool).await
    }

    pub async fn find_by_name(pool: &SqlitePool, name: &str) -> sqlx::Result<Option<Self>> {
        let sql = format!(
            "SELECT {COLUMNS} FROM bandwidth_profiles WHERE name = ? AND deleted_at IS NULL"
        );
        sqlx::query_as(&sql).bind(name).fetch_optional(pool).await
    }

    pub async fn create(
        pool: &SqlitePool,
        name: &str,
        bandwidth_mbps: i64,
        description: &str,
    ) -> sqlx::Result<i64> {
        let res = sqlx::query(
            "INSERT INTO bandwidth_profiles (name, bandwidth_mbps, description) VALUES (?, ?, ?)",
        )
        .bind(name)
        .bind(bandwidth_mbps)
        .bind(description)
        .execute(pool)
        .await?;
        Ok(res.last_insert_rowid())
    }

    pub async fn update_fields(
        pool: &SqlitePool,
        id: i64,
        name: Option<&str>,
        bandwidth_mbps: Option<i64>,
        description: Option<&str>,
    ) -> sqlx::Result<u64> {
        let res = sqlx::query(
            "UPDATE bandwidth_profiles SET \
                name = COALESCE(?, name), \
                bandwidth_mbps = COALESCE(?, bandwidth_mbps), \
                description = COALESCE(?, description), \
                updated_at = datetime('now') \
             WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(name)
        .bind(bandwidth_mbps)
        .bind(description)
        .bind(id)
        .execute(pool)
        .await?;
        Ok(res.rows_affected())
    }

    pub async fn soft_delete(pool: &SqlitePool, id: i64) -> sqlx::Result<u64> {
        let res = sqlx::query(
            "UPDATE bandwidth_profiles SET deleted_at = datetime('now'), updated_at = datetime('now') \
             WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(id)
        .execute(pool)
        .await?;
        Ok(res.rows_affected())
    }

    /// 活跃规则对该 profile 的引用数(删除保护用)。
    pub async fn active_rule_refs(pool: &SqlitePool, id: i64) -> sqlx::Result<i64> {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM forward_rules \
             WHERE bandwidth_profile_id = ? AND deleted_at IS NULL",
        )
        .bind(id)
        .fetch_one(pool)
        .await
    }
}
