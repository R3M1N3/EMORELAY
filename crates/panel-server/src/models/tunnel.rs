use sqlx::{prelude::FromRow, SqlitePool};

#[derive(Debug, Clone, FromRow)]
pub struct Tunnel {
    pub id: i64,
    pub name: String,
    pub transport: String,
    pub status: String,
    /// 计费倍率(默认 1.0);billing_mode: 2=双向 rx+tx, 1=单向(较大方向)。
    pub traffic_ratio: f64,
    pub billing_mode: i64,
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

const TUNNEL_COLS: &str =
    "id, name, transport, status, traffic_ratio, billing_mode, created_at, updated_at";
const HOP_COLS: &str = "id, tunnel_id, ordinal, node_id, inter_port, created_at";

impl Tunnel {
    /// 事务建隧道 + N 跳。hops = &[(ordinal, node_id, inter_port)]。
    /// traffic_ratio / billing_mode 取 DB 默认(1.0 / 2 双向);非默认由调用方
    /// 随后 update_fields 覆盖,避免改本方法签名波及大量调用点。
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

    /// 部分更新 name / traffic_ratio / billing_mode(None 不变)。
    pub async fn update_fields(
        pool: &SqlitePool,
        id: i64,
        name: Option<&str>,
        traffic_ratio: Option<f64>,
        billing_mode: Option<i64>,
    ) -> sqlx::Result<u64> {
        let res = sqlx::query(
            "UPDATE tunnels SET \
                name = COALESCE(?1, name), \
                traffic_ratio = COALESCE(?2, traffic_ratio), \
                billing_mode = COALESCE(?3, billing_mode), \
                updated_at = datetime('now') \
             WHERE id = ?4 AND deleted_at IS NULL",
        )
        .bind(name)
        .bind(traffic_ratio)
        .bind(billing_mode)
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

    /// hop 心跳聚合(spec §5.1:最近 30s 有心跳 = 该 hop 存活)。
    /// 全部存活 → up;全部超窗 → down;部分 → degraded;无 hop → unknown(防御)。
    pub async fn compute_status(pool: &SqlitePool, id: i64) -> sqlx::Result<String> {
        let (total, alive): (i64, i64) = sqlx::query_as(
            "SELECT COUNT(*), \
                COALESCE(SUM(CASE WHEN n.last_seen_at IS NOT NULL \
                    AND n.last_seen_at >= datetime('now', '-30 seconds') THEN 1 ELSE 0 END), 0) \
             FROM tunnel_hops th JOIN nodes n ON n.id = th.node_id \
             WHERE th.tunnel_id = ?",
        )
        .bind(id)
        .fetch_one(pool)
        .await?;
        Ok(match (total, alive) {
            (0, _) => "unknown",
            (t, a) if a == t => "up",
            (_, 0) => "down",
            _ => "degraded",
        }
        .to_string())
    }

    pub async fn set_status(pool: &SqlitePool, id: i64, status: &str) -> sqlx::Result<u64> {
        let res = sqlx::query(
            "UPDATE tunnels SET status = ?, updated_at = datetime('now') \
             WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(status)
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

    /// 该节点参与的活跃隧道 id 列表(reconcile 用)。
    pub async fn list_tunnel_ids_for_node(pool: &SqlitePool, node_id: i64) -> sqlx::Result<Vec<i64>> {
        sqlx::query_scalar(
            "SELECT DISTINCT t.id FROM tunnel_hops th \
             JOIN tunnels t ON t.id = th.tunnel_id \
             WHERE th.node_id = ? AND t.deleted_at IS NULL ORDER BY t.id",
        )
        .bind(node_id)
        .fetch_all(pool)
        .await
    }

    /// 该节点在指定隧道里的 hop 行(凭据 ordinal 用)。
    pub async fn find_for_node(
        pool: &SqlitePool,
        tunnel_id: i64,
        node_id: i64,
    ) -> sqlx::Result<Option<Self>> {
        let sql = format!(
            "SELECT {HOP_COLS} FROM tunnel_hops WHERE tunnel_id = ? AND node_id = ? LIMIT 1"
        );
        sqlx::query_as(&sql).bind(tunnel_id).bind(node_id).fetch_optional(pool).await
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
