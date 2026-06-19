//! P7: 节点/隧道使用授权(默认拒绝)。普通用户只能用被授权的节点/隧道;admin 不受限。
use sqlx::SqlitePool;

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct GrantedUser {
    pub id: i64,
    pub username: String,
    /// 仅隧道授权:该用户在该隧道下的转发条数上限(NULL=不限);节点授权恒 None。
    pub forward_rules_limit_in_tunnel: Option<i64>,
}

pub async fn node_granted(pool: &SqlitePool, user_id: i64, node_id: i64) -> sqlx::Result<bool> {
    let hit: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM user_node_grants WHERE user_id = ? AND node_id = ?")
            .bind(user_id)
            .bind(node_id)
            .fetch_optional(pool)
            .await?;
    Ok(hit.is_some())
}

pub async fn tunnel_granted(pool: &SqlitePool, user_id: i64, tunnel_id: i64) -> sqlx::Result<bool> {
    let hit: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM user_tunnel_grants WHERE user_id = ? AND tunnel_id = ?")
            .bind(user_id)
            .bind(tunnel_id)
            .fetch_optional(pool)
            .await?;
    Ok(hit.is_some())
}

pub async fn granted_node_ids(pool: &SqlitePool, user_id: i64) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar("SELECT node_id FROM user_node_grants WHERE user_id = ?")
        .bind(user_id)
        .fetch_all(pool)
        .await
}

pub async fn granted_tunnel_ids(pool: &SqlitePool, user_id: i64) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar("SELECT tunnel_id FROM user_tunnel_grants WHERE user_id = ?")
        .bind(user_id)
        .fetch_all(pool)
        .await
}

/// 该用户在该隧道下的转发条数上限(NULL = 不限或未授权)。创建规则时校验用。
pub async fn tunnel_grant_num(
    pool: &SqlitePool,
    user_id: i64,
    tunnel_id: i64,
) -> sqlx::Result<Option<i64>> {
    let row: Option<Option<i64>> = sqlx::query_scalar(
        "SELECT forward_rules_limit_in_tunnel FROM user_tunnel_grants \
         WHERE user_id = ? AND tunnel_id = ?",
    )
    .bind(user_id)
    .bind(tunnel_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.flatten())
}

/// 用户的隧道授权 + 每隧道转发条数上限(编辑用户时回显)。
pub async fn granted_tunnel_grants(
    pool: &SqlitePool,
    user_id: i64,
) -> sqlx::Result<Vec<(i64, Option<i64>)>> {
    sqlx::query_as(
        "SELECT tunnel_id, forward_rules_limit_in_tunnel FROM user_tunnel_grants WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
}

/// 入参去重(复合主键下重复 id 会冲突成 500,按输入容错处理)。
fn dedup(ids: &[i64]) -> Vec<i64> {
    let mut v = ids.to_vec();
    v.sort_unstable();
    v.dedup();
    v
}

/// 隧道授权对 (tunnel_id, num) 按 tunnel_id 去重(复合主键冲突防护),保留首次出现的 num。
fn dedup_grants(grants: &[(i64, Option<i64>)]) -> Vec<(i64, Option<i64>)> {
    let mut seen = std::collections::HashSet::new();
    grants
        .iter()
        .filter(|(tid, _)| seen.insert(*tid))
        .copied()
        .collect()
}

/// 全量替换某用户的节点授权(事务:删旧 + 插新)。无效/已软删的 node_id 静默跳过。
pub async fn set_node_grants(pool: &SqlitePool, user_id: i64, node_ids: &[i64]) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM user_node_grants WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    for nid in dedup(node_ids) {
        sqlx::query(
            "INSERT INTO user_node_grants (user_id, node_id) \
             SELECT ?, ? WHERE EXISTS (SELECT 1 FROM nodes WHERE id = ? AND deleted_at IS NULL)",
        )
        .bind(user_id)
        .bind(nid)
        .bind(nid)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

/// 全量替换某用户的隧道授权(含每隧道转发条数上限 num)。无效/已软删的 tunnel_id 静默跳过。
/// grants 每项 = (tunnel_id, 该隧道转发条数上限;None = 不限)。
pub async fn set_tunnel_grants(
    pool: &SqlitePool,
    user_id: i64,
    grants: &[(i64, Option<i64>)],
) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM user_tunnel_grants WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    for (tid, num) in dedup_grants(grants) {
        sqlx::query(
            "INSERT INTO user_tunnel_grants (user_id, tunnel_id, forward_rules_limit_in_tunnel) \
             SELECT ?, ?, ? WHERE EXISTS (SELECT 1 FROM tunnels WHERE id = ? AND deleted_at IS NULL)",
        )
        .bind(user_id)
        .bind(tid)
        .bind(num)
        .bind(tid)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

/// 反向:某节点被授权给哪些用户(节点详情页"已授权用户")。
pub async fn users_for_node(pool: &SqlitePool, node_id: i64) -> sqlx::Result<Vec<GrantedUser>> {
    sqlx::query_as(
        // 节点授权无 per-隧道 上限,补 NULL 列对齐 GrantedUser(CAST 显式定型避免解码歧义)。
        "SELECT u.id, u.username, CAST(NULL AS INTEGER) AS forward_rules_limit_in_tunnel \
         FROM user_node_grants g \
         JOIN users u ON u.id = g.user_id \
         WHERE g.node_id = ? AND u.deleted_at IS NULL ORDER BY u.username",
    )
    .bind(node_id)
    .fetch_all(pool)
    .await
}

pub async fn users_for_tunnel(pool: &SqlitePool, tunnel_id: i64) -> sqlx::Result<Vec<GrantedUser>> {
    sqlx::query_as(
        "SELECT u.id, u.username, g.forward_rules_limit_in_tunnel FROM user_tunnel_grants g \
         JOIN users u ON u.id = g.user_id \
         WHERE g.tunnel_id = ? AND u.deleted_at IS NULL ORDER BY u.username",
    )
    .bind(tunnel_id)
    .fetch_all(pool)
    .await
}
