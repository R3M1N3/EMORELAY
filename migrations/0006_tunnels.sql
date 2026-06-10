-- migrations/0006_tunnels.sql
-- P3b 多跳隧道:tunnels(隧道定义) + tunnel_hops(有序跳) + forward_rules.tunnel_id(业务规则关联)。
-- PG 迁移:ADD COLUMN / CREATE TABLE / 部分唯一索引语法一致;datetime('now')→now()。
CREATE TABLE tunnels (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT    NOT NULL,
    transport   TEXT    NOT NULL CHECK (transport IN ('tcp', 'tls', 'wss')),
    status      TEXT    NOT NULL DEFAULT 'unknown'
                CHECK (status IN ('up', 'degraded', 'down', 'unknown')),
    created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    deleted_at  TEXT
);
CREATE UNIQUE INDEX idx_tunnels_name_active
    ON tunnels (name) WHERE deleted_at IS NULL;

CREATE TABLE tunnel_hops (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    tunnel_id   INTEGER NOT NULL REFERENCES tunnels(id),
    ordinal     INTEGER NOT NULL CHECK (ordinal >= 0),
    node_id     INTEGER NOT NULL REFERENCES nodes(id),
    -- 该 hop 被上一跳连入时监听的端口;ordinal 0(入口)为 NULL(它监听业务 listen_port)。
    inter_port  INTEGER CHECK (inter_port IS NULL OR (inter_port BETWEEN 1 AND 65535)),
    created_at  TEXT    NOT NULL DEFAULT (datetime('now'))
);
CREATE UNIQUE INDEX idx_tunnel_hops_tunnel_ordinal ON tunnel_hops (tunnel_id, ordinal);
CREATE INDEX idx_tunnel_hops_node_id ON tunnel_hops (node_id);

ALTER TABLE forward_rules ADD COLUMN tunnel_id INTEGER REFERENCES tunnels(id);
CREATE INDEX idx_forward_rules_tunnel_id ON forward_rules (tunnel_id);
