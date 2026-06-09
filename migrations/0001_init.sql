-- EMORELAY 初始数据库 schema。
-- 注：项目尚未发布，本 migration 在 MVP 开发期允许直接修改（替代 sqlx-migrate
-- 通常推荐的 immutable migrations 原则）。一旦发布，后续修改必须走新 migration。
--
-- 设计原则：
--   1. 兼容性：优先 SQLite，结构应能在最小改动下迁移到 PostgreSQL。
--   2. 时间戳：所有 created_at / updated_at / *_at 用 TEXT 存 ISO8601 字符串。
--      DEFAULT (datetime('now')) 仅 SQLite 适用；迁移到 PG 时需改为 now()。
--   3. 布尔：用 INTEGER 0/1 存储（SQLite 无原生 BOOLEAN），迁移到 PG 时换 BOOLEAN。
--   4. 自增主键：SQLite 用 INTEGER PRIMARY KEY AUTOINCREMENT；
--      迁移到 PG 时换 GENERATED ALWAYS AS IDENTITY。
--   5. updated_at 不使用触发器维护，由应用层（panel-server）在每次 UPDATE 时显式赋值。
--      这换来 PG 迁移时不需要重写触发器。
--   6. 软删除：users / nodes / forward_rules 用 deleted_at；唯一约束用部分索引 + WHERE
--      deleted_at IS NULL 实现，保证活跃记录唯一同时允许历史复活。
--   7. 时间序列：rule_stats / node_stats 按 bucket_at 聚合，避免逐请求/逐心跳爆库；
--      bucket 粒度与保留期由应用层依据 system_settings.stats_retention_days 维护。
--   8. SQLite WAL 模式由 panel-server 启动时执行 PRAGMA journal_mode=WAL 开启；
--      WAL 是连接/进程级配置，不属于 schema，不放在本 migration 内。
--   9. 外键引用为声明性（SQLite 默认不强制）；panel-server 在每个连接执行
--      PRAGMA foreign_keys = ON 来启用强制。

-- ============================ users ============================
CREATE TABLE users (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    username        TEXT    NOT NULL,
    password_hash   TEXT    NOT NULL,
    role            TEXT    NOT NULL DEFAULT 'user'
                    CHECK (role IN ('admin', 'user')),
    created_at      TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT    NOT NULL DEFAULT (datetime('now')),
    deleted_at      TEXT
);
CREATE UNIQUE INDEX idx_users_username_active
    ON users (username) WHERE deleted_at IS NULL;
CREATE INDEX idx_users_role       ON users (role);
CREATE INDEX idx_users_deleted_at ON users (deleted_at);

-- ============================ nodes ============================
CREATE TABLE nodes (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    name                TEXT    NOT NULL,
    region              TEXT    NOT NULL DEFAULT '',
    public_ip           TEXT    NOT NULL DEFAULT '',
    grpc_endpoint       TEXT    NOT NULL DEFAULT '',
    agent_token_hash    TEXT    NOT NULL,
    status              TEXT    NOT NULL DEFAULT 'unknown'
                        CHECK (status IN ('online', 'offline', 'unknown')),
    last_seen_at        TEXT,
    cpu_usage           REAL    NOT NULL DEFAULT 0,
    memory_usage        REAL    NOT NULL DEFAULT 0,
    load_average        REAL    NOT NULL DEFAULT 0,
    rx_bytes_total      INTEGER NOT NULL DEFAULT 0,
    tx_bytes_total      INTEGER NOT NULL DEFAULT 0,
    -- 节点可分配端口池：用于 NAT 节点限制对外暴露的端口范围。
    -- forward_rules.listen_port 必须落在所属节点的 [port_pool_min, port_pool_max] 区间。
    port_pool_min       INTEGER NOT NULL DEFAULT 1
                        CHECK (port_pool_min BETWEEN 1 AND 65535),
    port_pool_max       INTEGER NOT NULL DEFAULT 65535
                        CHECK (port_pool_max BETWEEN 1 AND 65535),
    created_at          TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at          TEXT    NOT NULL DEFAULT (datetime('now')),
    deleted_at          TEXT,
    CHECK (port_pool_min <= port_pool_max)
);
CREATE UNIQUE INDEX idx_nodes_name_active
    ON nodes (name) WHERE deleted_at IS NULL;
CREATE INDEX idx_nodes_status     ON nodes (status);
CREATE INDEX idx_nodes_deleted_at ON nodes (deleted_at);

-- ============================ forward_rules ============================
CREATE TABLE forward_rules (
    id                      INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id                 INTEGER NOT NULL REFERENCES users(id),
    node_id                 INTEGER NOT NULL REFERENCES nodes(id),
    name                    TEXT    NOT NULL,
    protocol                TEXT    NOT NULL
                            CHECK (protocol IN ('tcp', 'udp', 'tcp_udp')),
    listen_ip               TEXT    NOT NULL DEFAULT '0.0.0.0',
    listen_port             INTEGER NOT NULL
                            CHECK (listen_port BETWEEN 1 AND 65535),
    target_host             TEXT    NOT NULL,
    target_port             INTEGER NOT NULL
                            CHECK (target_port BETWEEN 1 AND 65535),
    enabled                 INTEGER NOT NULL DEFAULT 1
                            CHECK (enabled IN (0, 1)),
    expires_at              TEXT,
    traffic_limit_bytes     INTEGER,
    bandwidth_limit_mbps    INTEGER,
    rx_bytes                INTEGER NOT NULL DEFAULT 0,
    tx_bytes                INTEGER NOT NULL DEFAULT 0,
    connection_count        INTEGER NOT NULL DEFAULT 0,
    created_at              TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at              TEXT    NOT NULL DEFAULT (datetime('now')),
    deleted_at              TEXT
);
-- 同一节点 + 同协议 + 同 listen_ip + 同 listen_port 不允许重复绑定（仅对活跃规则）。
CREATE UNIQUE INDEX idx_forward_rules_binding_active
    ON forward_rules (node_id, protocol, listen_ip, listen_port)
    WHERE deleted_at IS NULL;
CREATE INDEX idx_forward_rules_user_id     ON forward_rules (user_id);
CREATE INDEX idx_forward_rules_node_id     ON forward_rules (node_id);
CREATE INDEX idx_forward_rules_enabled     ON forward_rules (enabled);
CREATE INDEX idx_forward_rules_listen_port ON forward_rules (listen_port);
CREATE INDEX idx_forward_rules_deleted_at  ON forward_rules (deleted_at);
CREATE INDEX idx_forward_rules_created_at  ON forward_rules (created_at);

-- ============================ rule_stats ============================
-- bucket_at 是聚合窗口起点（ISO8601），唯一约束确保每个规则每个窗口只一行。
CREATE TABLE rule_stats (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    rule_id             INTEGER NOT NULL REFERENCES forward_rules(id),
    bucket_at           TEXT    NOT NULL,
    rx_bytes            INTEGER NOT NULL DEFAULT 0,
    tx_bytes            INTEGER NOT NULL DEFAULT 0,
    connection_count    INTEGER NOT NULL DEFAULT 0,
    error_count         INTEGER NOT NULL DEFAULT 0,
    created_at          TEXT    NOT NULL DEFAULT (datetime('now'))
);
CREATE UNIQUE INDEX idx_rule_stats_rule_bucket
    ON rule_stats (rule_id, bucket_at);
CREATE INDEX idx_rule_stats_bucket_at ON rule_stats (bucket_at);

-- ============================ node_stats ============================
CREATE TABLE node_stats (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    node_id         INTEGER NOT NULL REFERENCES nodes(id),
    bucket_at       TEXT    NOT NULL,
    cpu_usage       REAL    NOT NULL DEFAULT 0,
    memory_usage    REAL    NOT NULL DEFAULT 0,
    load_average    REAL    NOT NULL DEFAULT 0,
    rx_bytes        INTEGER NOT NULL DEFAULT 0,
    tx_bytes        INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT    NOT NULL DEFAULT (datetime('now'))
);
CREATE UNIQUE INDEX idx_node_stats_node_bucket
    ON node_stats (node_id, bucket_at);
CREATE INDEX idx_node_stats_bucket_at ON node_stats (bucket_at);

-- ============================ audit_logs ============================
-- 记录所有写操作（plan.md 第九节"所有危险 API 记录审计日志"）。
-- actor_user_id 为 NULL 表示系统/未登录操作；action 用点分命名空间，如
-- 'auth.login' / 'rule.create' / 'node.delete' / 'rule.enable'。
CREATE TABLE audit_logs (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    actor_user_id   INTEGER REFERENCES users(id),
    actor_ip        TEXT,
    action          TEXT    NOT NULL,
    target_type     TEXT,
    target_id       INTEGER,
    payload         TEXT,
    result          TEXT    NOT NULL DEFAULT 'success'
                    CHECK (result IN ('success', 'failure')),
    error_message   TEXT,
    created_at      TEXT    NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX idx_audit_logs_actor_user_id ON audit_logs (actor_user_id);
CREATE INDEX idx_audit_logs_action        ON audit_logs (action);
CREATE INDEX idx_audit_logs_target        ON audit_logs (target_type, target_id);
CREATE INDEX idx_audit_logs_created_at    ON audit_logs (created_at);

-- ============================ system_settings ============================
-- 单表 K/V，value 为 TEXT；如需复杂结构则在应用层 JSON 编码。
CREATE TABLE system_settings (
    key         TEXT PRIMARY KEY,
    value       TEXT NOT NULL,
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- 默认配置：保留端口黑名单（plan.md 第九节）。空字符串表示"不限"。
INSERT INTO system_settings (key, value) VALUES
    ('reserved_ports',               '[22, 80, 443, 3306, 5432]'),
    ('default_traffic_limit_bytes',  ''),
    ('default_bandwidth_limit_mbps', ''),
    ('stats_retention_days',         '30'),
    ('agent_control_endpoint',       '');

-- ============================ agent_sessions ============================
-- 每条 Agent gRPC 会话一行；session_token_hash 是该会话临时凭据的哈希。
-- closed_at 为 NULL 表示会话仍在线；按 last_seen_at 判活。
CREATE TABLE agent_sessions (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    node_id             INTEGER NOT NULL REFERENCES nodes(id),
    session_token_hash  TEXT    NOT NULL,
    remote_addr         TEXT,
    connected_at        TEXT    NOT NULL DEFAULT (datetime('now')),
    last_seen_at        TEXT    NOT NULL DEFAULT (datetime('now')),
    closed_at           TEXT
);
CREATE INDEX idx_agent_sessions_node_id   ON agent_sessions (node_id);
CREATE INDEX idx_agent_sessions_closed_at ON agent_sessions (closed_at);
