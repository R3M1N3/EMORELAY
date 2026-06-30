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
    /// 并发连接上限(仅 TCP)。None = 不限。
    pub max_connections: Option<i64>,
    /// P2 多目标:额外目标 JSON 数组 [{host,port}];None/空 = 单目标。
    pub extra_targets: Option<String>,
    /// 负载策略 fifo/round/rand/hash;默认 fifo。仅目标数 > 1 时生效。
    pub lb_strategy: String,
    /// realm-parity:是否向上游发送 PROXY protocol v1 头(0/1)。仅非隧道 TCP relay 生效。
    pub send_proxy_protocol: i64,
    pub created_at: String,
    pub updated_at: String,
}

const RULE_COLUMNS: &str = "id, user_id, node_id, name, protocol, listen_ip, listen_port, \
    target_host, target_port, enabled, rx_bytes, tx_bytes, connection_count, \
    bandwidth_profile_id, \
    (SELECT bp.bandwidth_mbps FROM bandwidth_profiles bp \
        WHERE bp.id = forward_rules.bandwidth_profile_id AND bp.deleted_at IS NULL) AS bandwidth_mbps, \
    tunnel_id, max_connections, extra_targets, lb_strategy, send_proxy_protocol, created_at, updated_at";

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

/// forward_rules 列表/计数共用的 WHERE 片段(含 deleted_at IS NULL)。
/// **片段顺序必须与 [`bind_rule_filters!`] 的 bind 顺序严格一致**——SQLite `?` 是位置绑定,
/// 顺序错位会静默把值绑到错误的列。新增筛选项必须同时改本函数与该宏。
fn rule_filter_where(
    node_id: Option<i64>,
    protocol: Option<&str>,
    search: Option<&str>,
    restrict_user_id: Option<i64>,
    user_id: Option<i64>,
    enabled: Option<bool>,
) -> String {
    let mut parts = vec!["deleted_at IS NULL"];
    if node_id.is_some() {
        parts.push("node_id = ?");
    }
    if protocol.is_some() {
        parts.push("protocol = ?");
    }
    if search.is_some() {
        parts.push("(name LIKE ? ESCAPE '\\' OR target_host LIKE ? ESCAPE '\\' OR CAST(listen_port AS TEXT) = ?)");
    }
    if restrict_user_id.is_some() {
        parts.push("user_id = ?");
    }
    if user_id.is_some() {
        parts.push("user_id = ?");
    }
    if enabled.is_some() {
        parts.push("enabled = ?");
    }
    parts.join(" AND ")
}

/// 把 [`rule_filter_where`] 对应的过滤值按同序 bind 到 query builder(list_paged/count_filtered 共用)。
/// bind 顺序必须与 `rule_filter_where` 的片段顺序一致。
macro_rules! bind_rule_filters {
    ($q:expr, $node_id:expr, $protocol:expr, $search:expr, $restrict_user_id:expr, $user_id:expr, $enabled:expr) => {{
        let mut q = $q;
        if let Some(nid) = $node_id {
            q = q.bind(nid);
        }
        if let Some(p) = $protocol {
            q = q.bind(p);
        }
        if let Some(s) = $search {
            // 转义 \ % _ 防通配符污染;裸 s 那个 bind 是端口精确匹配,不转义。
            let like = format!("%{}%", crate::util::escape_like(s));
            q = q.bind(like.clone()).bind(like).bind(s.to_string());
        }
        if let Some(uid) = $restrict_user_id {
            q = q.bind(uid);
        }
        if let Some(uid) = $user_id {
            q = q.bind(uid);
        }
        if let Some(en) = $enabled {
            q = q.bind(en);
        }
        q
    }};
}

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
        user_id: Option<i64>,
        enabled: Option<bool>,
    ) -> sqlx::Result<Vec<Self>> {
        let order = if order_desc { "DESC" } else { "ASC" };
        let where_sql =
            rule_filter_where(node_id, protocol, search, restrict_user_id, user_id, enabled);
        let sql = format!(
            "SELECT {RULE_COLUMNS} FROM forward_rules WHERE {where_sql} ORDER BY {sort_field} {order} LIMIT ? OFFSET ?"
        );
        let q = bind_rule_filters!(
            sqlx::query_as::<_, Rule>(&sql),
            node_id,
            protocol,
            search,
            restrict_user_id,
            user_id,
            enabled
        );
        q.bind(limit).bind(offset).fetch_all(pool).await
    }

    pub async fn count_filtered(
        pool: &SqlitePool,
        node_id: Option<i64>,
        protocol: Option<&str>,
        search: Option<&str>,
        restrict_user_id: Option<i64>,
        user_id: Option<i64>,
        enabled: Option<bool>,
    ) -> sqlx::Result<i64> {
        let where_sql =
            rule_filter_where(node_id, protocol, search, restrict_user_id, user_id, enabled);
        let sql = format!("SELECT COUNT(*) FROM forward_rules WHERE {where_sql}");
        let q = bind_rule_filters!(
            sqlx::query_scalar::<_, i64>(&sql),
            node_id,
            protocol,
            search,
            restrict_user_id,
            user_id,
            enabled
        );
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

    /// 列出某节点下所有未软删的规则。Agent 重连 reconcile 用(结果即 reconcile 的 keep_ids)。
    ///
    /// **不变量(勿加 `enabled` 过滤)**:keep_ids 必须包含全部「未软删」规则,含
    /// disabled-but-not-deleted。否则 sweeper / `user_quota` / `bandwidth_profiles` 等
    /// **不持 per-node 锁**的裸 `dispatch_rule_apply` 所重发的已存在规则,会被 reconcile
    /// 的 keep_ids 漏掉、当孤儿误删(退化为 code-review 修复的 #1 竞态)。enabled 状态由
    /// ApplyRule 的 payload 表达,不在 reconcile 的存活集判定。
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
        max_connections: Option<i64>,
    ) -> sqlx::Result<i64> {
        let res = sqlx::query(
            "INSERT INTO forward_rules \
                (user_id, node_id, name, protocol, listen_ip, listen_port, \
                 target_host, target_port, bandwidth_profile_id, tunnel_id, max_connections) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        .bind(max_connections)
        .execute(pool)
        .await?;
        Ok(res.last_insert_rowid())
    }

    /// 设置「向上游发送 PROXY protocol」开关(0/1)。单独成方法,避免改 create/update_fields 签名。
    pub async fn set_send_proxy_protocol(
        pool: &SqlitePool,
        id: i64,
        enabled: bool,
    ) -> sqlx::Result<u64> {
        let res = sqlx::query(
            "UPDATE forward_rules SET send_proxy_protocol = ?, updated_at = datetime('now') \
             WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(i64::from(enabled))
        .bind(id)
        .execute(pool)
        .await?;
        Ok(res.rows_affected())
    }

    /// 设置多目标:extra_targets(JSON,None=清空)+ lb_strategy。
    /// 单独成方法,避免改 create/update_fields 签名波及多处调用点(同 P2 倍率/月重置取舍)。
    pub async fn set_targets(
        pool: &SqlitePool,
        id: i64,
        extra_targets: Option<&str>,
        lb_strategy: &str,
    ) -> sqlx::Result<u64> {
        let res = sqlx::query(
            "UPDATE forward_rules SET extra_targets = ?, lb_strategy = ?, \
                 updated_at = datetime('now') \
             WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(extra_targets)
        .bind(lb_strategy)
        .bind(id)
        .execute(pool)
        .await?;
        Ok(res.rows_affected())
    }

    /// PATCH 语义：None 字段保留旧值;bandwidth_profile_id / max_connections 传 0 = 清除。
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
        max_connections: Option<i64>,
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
                max_connections = CASE \
                    WHEN ?7 IS NULL THEN max_connections \
                    WHEN ?7 = 0 THEN NULL \
                    ELSE ?7 END, \
                updated_at = datetime('now') \
             WHERE id = ?8 AND deleted_at IS NULL",
        )
        .bind(name)
        .bind(listen_ip)
        .bind(listen_port)
        .bind(target_host)
        .bind(target_port)
        .bind(bandwidth_profile_id)
        .bind(max_connections)
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
