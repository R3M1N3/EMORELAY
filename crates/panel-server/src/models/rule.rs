use sqlx::{prelude::FromRow, SqlitePool};

#[derive(Debug, Clone, FromRow)]
pub struct Rule {
    pub id: i64,
    pub user_id: i64,
    pub node_id: i64,
    pub name: String,
    pub protocol: String,
    pub listen_ip: String,
    pub listen_port: i64,
    pub target_host: String,
    pub target_port: i64,
    pub enabled: i64,
    pub rx_bytes: i64,
    pub tx_bytes: i64,
    pub connection_count: i64,
    pub bandwidth_profile_id: Option<i64>,
    /// 派生列:关联 profile 的 Mbps(活跃 profile);无关联/已删 → None。
    pub bandwidth_mbps: Option<i64>,
    pub tunnel_id: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

const RULE_COLUMNS: &str = "id, user_id, node_id, name, protocol, listen_ip, listen_port, \
    target_host, target_port, enabled, rx_bytes, tx_bytes, connection_count, \
    bandwidth_profile_id, \
    (SELECT bp.bandwidth_mbps FROM bandwidth_profiles bp \
        WHERE bp.id = forward_rules.bandwidth_profile_id AND bp.deleted_at IS NULL) AS bandwidth_mbps, \
    tunnel_id, created_at, updated_at";

/// 允许的排序字段白名单。值必须为 schema 真实列名且非敏感字段；
/// SQL 拼接前必须经此过滤。
pub const SORT_FIELDS: &[&str] = &[
    "id",
    "name",
    "node_id",
    "protocol",
    "listen_port",
    "enabled",
    "created_at",
    "updated_at",
];

impl Rule {
    #[allow(clippy::too_many_arguments)]
    pub async fn list_paged(
        pool: &SqlitePool,
        sort_field: &str,
        order_desc: bool,
        limit: i64,
        offset: i64,
        node_id: Option<i64>,
        protocol: Option<&str>,
        search: Option<&str>,
        restrict_user_id: Option<i64>,
    ) -> sqlx::Result<Vec<Self>> {
        let order = if order_desc { "DESC" } else { "ASC" };
        let mut where_parts = vec!["deleted_at IS NULL".to_string()];
        if node_id.is_some() {
            where_parts.push("node_id = ?".into());
        }
        if protocol.is_some() {
            where_parts.push("protocol = ?".into());
        }
        if search.is_some() {
            where_parts.push(
                "(name LIKE ? ESCAPE '\\' OR target_host LIKE ? ESCAPE '\\' OR CAST(listen_port AS TEXT) = ?)".into(),
            );
        }
        if restrict_user_id.is_some() {
            where_parts.push("user_id = ?".into());
        }
        let sql = format!(
            "SELECT {RULE_COLUMNS} FROM forward_rules WHERE {} ORDER BY {sort_field} {order} LIMIT ? OFFSET ?",
            where_parts.join(" AND ")
        );
        let mut q = sqlx::query_as::<_, Rule>(&sql);
        if let Some(nid) = node_id {
            q = q.bind(nid);
        }
        if let Some(p) = protocol {
            q = q.bind(p);
        }
        if let Some(s) = search {
            // 转义 \ % _ 防通配符污染;裸 s 那个 bind 是端口精确匹配,不转义。
            let like = format!("%{}%", crate::util::escape_like(s));
            q = q.bind(like.clone()).bind(like).bind(s.to_string());
        }
        if let Some(uid) = restrict_user_id {
            q = q.bind(uid);
        }
        q.bind(limit).bind(offset).fetch_all(pool).await
    }

    pub async fn count_filtered(
        pool: &SqlitePool,
        node_id: Option<i64>,
        protocol: Option<&str>,
        search: Option<&str>,
        restrict_user_id: Option<i64>,
    ) -> sqlx::Result<i64> {
        let mut where_parts = vec!["deleted_at IS NULL".to_string()];
        if node_id.is_some() {
            where_parts.push("node_id = ?".into());
        }
        if protocol.is_some() {
            where_parts.push("protocol = ?".into());
        }
        if search.is_some() {
            where_parts.push(
                "(name LIKE ? ESCAPE '\\' OR target_host LIKE ? ESCAPE '\\' OR CAST(listen_port AS TEXT) = ?)".into(),
            );
        }
        if restrict_user_id.is_some() {
            where_parts.push("user_id = ?".into());
        }
        let sql = format!(
            "SELECT COUNT(*) FROM forward_rules WHERE {}",
            where_parts.join(" AND ")
        );
        let mut q = sqlx::query_scalar::<_, i64>(&sql);
        if let Some(nid) = node_id {
            q = q.bind(nid);
        }
        if let Some(p) = protocol {
            q = q.bind(p);
        }
        if let Some(s) = search {
            // 转义 \ % _ 防通配符污染;裸 s 那个 bind 是端口精确匹配,不转义。
            let like = format!("%{}%", crate::util::escape_like(s));
            q = q.bind(like.clone()).bind(like).bind(s.to_string());
        }
        if let Some(uid) = restrict_user_id {
            q = q.bind(uid);
        }
        q.fetch_one(pool).await
    }

    pub async fn find_by_id(pool: &SqlitePool, id: i64) -> sqlx::Result<Option<Self>> {
        let sql = format!(
            "SELECT {RULE_COLUMNS} FROM forward_rules WHERE id = ? AND deleted_at IS NULL"
        );
        sqlx::query_as::<_, Rule>(&sql)
            .bind(id)
            .fetch_optional(pool)
            .await
    }

    /// 列出某节点下所有未软删的规则。Agent 重连 reconcile 用。
    pub async fn list_active_for_node(
        pool: &SqlitePool,
        node_id: i64,
    ) -> sqlx::Result<Vec<Self>> {
        let sql = format!(
            "SELECT {RULE_COLUMNS} FROM forward_rules WHERE node_id = ? AND deleted_at IS NULL ORDER BY id"
        );
        sqlx::query_as::<_, Rule>(&sql)
            .bind(node_id)
            .fetch_all(pool)
            .await
    }

    /// 关联某隧道的全部活跃规则(数据面 split 下发/reconcile 用)。
    pub async fn list_active_for_tunnel(
        pool: &SqlitePool,
        tunnel_id: i64,
    ) -> sqlx::Result<Vec<Self>> {
        let sql = format!(
            "SELECT {RULE_COLUMNS} FROM forward_rules WHERE tunnel_id = ? AND deleted_at IS NULL ORDER BY id"
        );
        sqlx::query_as::<_, Rule>(&sql).bind(tunnel_id).fetch_all(pool).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        pool: &SqlitePool,
        user_id: i64,
        node_id: i64,
        name: &str,
        protocol: &str,
        listen_ip: &str,
        listen_port: i64,
        target_host: &str,
        target_port: i64,
        bandwidth_profile_id: Option<i64>,
        tunnel_id: Option<i64>,
    ) -> sqlx::Result<i64> {
        let res = sqlx::query(
            "INSERT INTO forward_rules \
                (user_id, node_id, name, protocol, listen_ip, listen_port, \
                 target_host, target_port, bandwidth_profile_id, tunnel_id) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(user_id)
        .bind(node_id)
        .bind(name)
        .bind(protocol)
        .bind(listen_ip)
        .bind(listen_port)
        .bind(target_host)
        .bind(target_port)
        .bind(bandwidth_profile_id)
        .bind(tunnel_id)
        .execute(pool)
        .await?;
        Ok(res.last_insert_rowid())
    }

    /// PATCH 语义：None 字段保留旧值;bandwidth_profile_id 传 0 = 解除关联。
    /// 改 protocol / node_id 需要 delete+create。
    #[allow(clippy::too_many_arguments)]
    pub async fn update_fields(
        pool: &SqlitePool,
        id: i64,
        name: Option<&str>,
        listen_ip: Option<&str>,
        listen_port: Option<i64>,
        target_host: Option<&str>,
        target_port: Option<i64>,
        bandwidth_profile_id: Option<i64>,
    ) -> sqlx::Result<u64> {
        let res = sqlx::query(
            "UPDATE forward_rules SET \
                name = COALESCE(?1, name), \
                listen_ip = COALESCE(?2, listen_ip), \
                listen_port = COALESCE(?3, listen_port), \
                target_host = COALESCE(?4, target_host), \
                target_port = COALESCE(?5, target_port), \
                bandwidth_profile_id = CASE \
                    WHEN ?6 IS NULL THEN bandwidth_profile_id \
                    WHEN ?6 = 0 THEN NULL \
                    ELSE ?6 END, \
                updated_at = datetime('now') \
             WHERE id = ?7 AND deleted_at IS NULL",
        )
        .bind(name)
        .bind(listen_ip)
        .bind(listen_port)
        .bind(target_host)
        .bind(target_port)
        .bind(bandwidth_profile_id)
        .bind(id)
        .execute(pool)
        .await?;
        Ok(res.rows_affected())
    }

    pub async fn set_enabled(pool: &SqlitePool, id: i64, enabled: bool) -> sqlx::Result<u64> {
        let res = sqlx::query(
            "UPDATE forward_rules SET enabled = ?, updated_at = datetime('now') \
             WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(if enabled { 1_i64 } else { 0 })
        .bind(id)
        .execute(pool)
        .await?;
        Ok(res.rows_affected())
    }

    pub async fn soft_delete(pool: &SqlitePool, id: i64) -> sqlx::Result<u64> {
        let res = sqlx::query(
            "UPDATE forward_rules SET deleted_at = datetime('now'), updated_at = datetime('now') \
             WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(id)
        .execute(pool)
        .await?;
        Ok(res.rows_affected())
    }
}
