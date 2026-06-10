use sqlx::{prelude::FromRow, SqlitePool};

#[derive(Debug, Clone, FromRow)]
pub struct Node {
    pub id: i64,
    pub name: String,
    pub region: String,
    pub public_ip: String,
    pub grpc_endpoint: String,
    pub status: String,
    pub last_seen_at: Option<String>,
    pub cpu_usage: f64,
    pub memory_usage: f64,
    pub load_average: f64,
    pub rx_bytes_total: i64,
    pub tx_bytes_total: i64,
    pub port_pool_min: i64,
    pub port_pool_max: i64,
    pub created_at: String,
    pub updated_at: String,
}

const NODE_COLUMNS: &str = "id, name, region, public_ip, grpc_endpoint, status, last_seen_at, \
    cpu_usage, memory_usage, load_average, rx_bytes_total, tx_bytes_total, \
    port_pool_min, port_pool_max, created_at, updated_at";

/// 允许的排序字段白名单。SQL 拼接前必须经此过滤。
pub const SORT_FIELDS: &[&str] = &["id", "name", "status", "region", "created_at", "updated_at"];

impl Node {
    pub async fn list_paged(
        pool: &SqlitePool,
        sort_field: &str,
        order_desc: bool,
        limit: i64,
        offset: i64,
    ) -> sqlx::Result<Vec<Self>> {
        // sort_field 必须来自调用方白名单过滤；不能直接接收用户输入。
        let order = if order_desc { "DESC" } else { "ASC" };
        let sql = format!(
            "SELECT {NODE_COLUMNS} FROM nodes WHERE deleted_at IS NULL \
             ORDER BY {sort_field} {order} LIMIT ? OFFSET ?"
        );
        sqlx::query_as::<_, Node>(&sql)
            .bind(limit)
            .bind(offset)
            .fetch_all(pool)
            .await
    }

    pub async fn count(pool: &SqlitePool) -> sqlx::Result<i64> {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM nodes WHERE deleted_at IS NULL")
            .fetch_one(pool)
            .await
    }

    pub async fn find_by_id(pool: &SqlitePool, id: i64) -> sqlx::Result<Option<Self>> {
        let sql = format!(
            "SELECT {NODE_COLUMNS} FROM nodes WHERE id = ? AND deleted_at IS NULL"
        );
        sqlx::query_as::<_, Node>(&sql)
            .bind(id)
            .fetch_optional(pool)
            .await
    }

    /// 规则导入按 node_name 映射跨实例节点用。
    pub async fn find_by_name(pool: &SqlitePool, name: &str) -> sqlx::Result<Option<Self>> {
        let sql = format!(
            "SELECT {NODE_COLUMNS} FROM nodes WHERE name = ? AND deleted_at IS NULL"
        );
        sqlx::query_as::<_, Node>(&sql)
            .bind(name)
            .fetch_optional(pool)
            .await
    }

    pub async fn create(
        pool: &SqlitePool,
        name: &str,
        region: &str,
        public_ip: &str,
        grpc_endpoint: &str,
        agent_token_hash: &str,
        port_pool_min: i64,
        port_pool_max: i64,
    ) -> sqlx::Result<i64> {
        let res = sqlx::query(
            "INSERT INTO nodes (name, region, public_ip, grpc_endpoint, agent_token_hash, \
                                port_pool_min, port_pool_max) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(name)
        .bind(region)
        .bind(public_ip)
        .bind(grpc_endpoint)
        .bind(agent_token_hash)
        .bind(port_pool_min)
        .bind(port_pool_max)
        .execute(pool)
        .await?;
        Ok(res.last_insert_rowid())
    }

    /// PATCH 语义：传 None 的字段保留旧值，由 COALESCE(?, col) 实现。
    /// 同时刷新 updated_at（应用层维护）。
    #[allow(clippy::too_many_arguments)]
    pub async fn update(
        pool: &SqlitePool,
        id: i64,
        name: Option<&str>,
        region: Option<&str>,
        public_ip: Option<&str>,
        grpc_endpoint: Option<&str>,
        port_pool_min: Option<i64>,
        port_pool_max: Option<i64>,
    ) -> sqlx::Result<u64> {
        let res = sqlx::query(
            "UPDATE nodes SET \
                name = COALESCE(?, name), \
                region = COALESCE(?, region), \
                public_ip = COALESCE(?, public_ip), \
                grpc_endpoint = COALESCE(?, grpc_endpoint), \
                port_pool_min = COALESCE(?, port_pool_min), \
                port_pool_max = COALESCE(?, port_pool_max), \
                updated_at = datetime('now') \
             WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(name)
        .bind(region)
        .bind(public_ip)
        .bind(grpc_endpoint)
        .bind(port_pool_min)
        .bind(port_pool_max)
        .bind(id)
        .execute(pool)
        .await?;
        Ok(res.rows_affected())
    }

    pub async fn soft_delete(pool: &SqlitePool, id: i64) -> sqlx::Result<u64> {
        let res = sqlx::query(
            "UPDATE nodes SET deleted_at = datetime('now'), updated_at = datetime('now') \
             WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(id)
        .execute(pool)
        .await?;
        Ok(res.rows_affected())
    }
}
