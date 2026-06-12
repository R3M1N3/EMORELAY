-- P7: 节点/隧道使用授权(默认拒绝)。
-- 普通用户只能在被授权的节点上建规则、关联被授权的隧道;admin 不受限。
-- 撤销授权不影响存量规则(保留运行,仅禁止新建),由应用层保证。
-- 复合主键天然唯一且覆盖 user_id 前缀查询;额外按 node_id/tunnel_id 建索引覆盖反向
-- 查询(某节点/隧道被授权给哪些用户,详情页用)。
-- 兼容性:沿用 SQLite 风格 TEXT datetime,迁移 PG 时 datetime('now')->now()。

CREATE TABLE user_node_grants (
    user_id    INTEGER NOT NULL REFERENCES users(id),
    node_id    INTEGER NOT NULL REFERENCES nodes(id),
    created_at TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (user_id, node_id)
);
CREATE INDEX idx_user_node_grants_node ON user_node_grants (node_id);

CREATE TABLE user_tunnel_grants (
    user_id    INTEGER NOT NULL REFERENCES users(id),
    tunnel_id  INTEGER NOT NULL REFERENCES tunnels(id),
    created_at TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (user_id, tunnel_id)
);
CREATE INDEX idx_user_tunnel_grants_tunnel ON user_tunnel_grants (tunnel_id);
