use sqlx::{prelude::FromRow, SqlitePool};

#[derive(Debug, Clone, FromRow)]
pub struct Tunnel {
    pub id: i64,
    pub name: String,
    pub transport: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct TunnelHop {
    pub id: i64,
    pub tunnel_id: i64,
    pub ordinal: i64,
    pub node_id: i64,
    pub inter_port: Option<i64>,
    pub created_at: String,
}

const TUNNEL_COLS: &str = "id, name, transport, status, created_at, updated_at";
const HOP_COLS: &str = "id, tunnel_id, ordinal, node_id, inter_port, created_at";

impl Tunnel {
    /// 事务建隧道 + N 跳。hops = &[(ordinal, node_id, inter_port)]。
    pub async fn create_with_hops(
        pool: &SqlitePool,
        name: &str,
        transport: &str,
        hops: &[(i64, i64, Option<i64>)],
    ) -> sqlx::Result<i64> {
        let mut tx = pool.begin().await?;
        let res = sqlx::query("INSERT INTO tunnels (name, transport) VALUES (?, ?)")
            .bind(name)
            .bind(transport)
            .execute(&mut *tx)
            .await?;
        let tunnel_id = res.last_insert_rowid();
        for (ordinal, node_id, inter_port) in hops {
            sqlx::query(
                "INSERT INTO tunnel_hops (tunnel_id, ordinal, node_id, inter_port) VALUES (?, ?, ?, ?)",
            )
            .bind(tunnel_id)
            .bind(ordinal)
            .bind(node_id)
            .bind(inter_port)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(tunnel_id)
    }

    pub async fn find_by_id(pool: &SqlitePool, id: i64) -> sqlx::Result<Option<Self>> {
        let sql = format!("SELECT {TUNNEL_COLS} FROM tunnels WHERE id = ? AND deleted_at IS NULL");
        sqlx::query_as(&sql).bind(id).fetch_optional(pool).await
    }

    pub async fn list_paged(pool: &SqlitePool, limit: i64, offset: i64) -> sqlx::Result<Vec<Self>> {
        let sql = format!(
            "SELECT {TUNNEL_COLS} FROM tunnels WHERE deleted_at IS NULL ORDER BY id DESC LIMIT ? OFFSET ?"
        );
        sqlx::query_as(&sql).bind(limit).bind(offset).fetch_all(pool).await
    }

    pub async fn count(pool: &SqlitePool) -> sqlx::Result<i64> {
        sqlx::query_scalar("SELECT COUNT(*) FROM tunnels WHERE deleted_at IS NULL")
            .fetch_one(pool)
            .await
    }

    pub async fn update_name(pool: &SqlitePool, id: i64, name: &str) -> sqlx::Result<u64> {
        let res = sqlx::query(
            "UPDATE tunnels SET name = ?, updated_at = datetime('now') WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(name)
        .bind(id)
        .execute(pool)
        .await?;
        Ok(res.rows_affected())
    }

    pub async fn soft_delete(pool: &SqlitePool, id: i64) -> sqlx::Result<u64> {
        let res = sqlx::query(
            "UPDATE tunnels SET deleted_at = datetime('now'), updated_at = datetime('now') \
             WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(id)
        .execute(pool)
        .await?;
        Ok(res.rows_affected())
    }

    /// 引用该隧道的活跃业务规则数(删除保护用)。
    pub async fn active_rule_refs(pool: &SqlitePool, id: i64) -> sqlx::Result<i64> {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM forward_rules WHERE tunnel_id = ? AND deleted_at IS NULL",
        )
        .bind(id)
        .fetch_one(pool)
        .await
    }
}

impl TunnelHop {
    pub async fn list_for_tunnel(pool: &SqlitePool, tunnel_id: i64) -> sqlx::Result<Vec<Self>> {
        let sql = format!(
            "SELECT {HOP_COLS} FROM tunnel_hops WHERE tunnel_id = ? ORDER BY ordinal"
        );
        sqlx::query_as(&sql).bind(tunnel_id).fetch_all(pool).await
    }

    /// 该节点是否参与任一活跃隧道(删节点保护用;隧道软删时其 hops 视为失效)。
    pub async fn node_in_active_tunnel(pool: &SqlitePool, node_id: i64) -> sqlx::Result<bool> {
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM tunnel_hops th \
             JOIN tunnels t ON t.id = th.tunnel_id \
             WHERE th.node_id = ? AND t.deleted_at IS NULL",
        )
        .bind(node_id)
        .fetch_one(pool)
        .await?;
        Ok(n > 0)
    }
}
