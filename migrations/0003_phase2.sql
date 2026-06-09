-- migrations/0003_phase2.sql
-- Phase 2 加法部分：限制语义从 forward_rules 搬往 users / bandwidth_profiles。
-- 减法（DROP 三列）在 0004_drop_rule_limits.sql，等代码侧不再引用后执行。
-- PG 迁移路径：ALTER TABLE ... ADD COLUMN 与部分唯一索引语法一致；
-- datetime('now') 换 now()；INTEGER 布尔/外键语义不变。

-- users 扩展：到期 + 滚动 30 天流量配额 + 用量缓存
ALTER TABLE users ADD COLUMN expires_at TEXT;
ALTER TABLE users ADD COLUMN traffic_limit_bytes_30d INTEGER;
ALTER TABLE users ADD COLUMN period_used_bytes_cached INTEGER NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN period_used_calculated_at TEXT;

-- 限速 profile（独立路由可复用）
CREATE TABLE bandwidth_profiles (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    name            TEXT    NOT NULL,
    bandwidth_mbps  INTEGER NOT NULL CHECK (bandwidth_mbps > 0),
    description     TEXT    NOT NULL DEFAULT '',
    created_at      TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT    NOT NULL DEFAULT (datetime('now')),
    deleted_at      TEXT
);
CREATE UNIQUE INDEX idx_bandwidth_profiles_name_active
    ON bandwidth_profiles (name) WHERE deleted_at IS NULL;

ALTER TABLE forward_rules
    ADD COLUMN bandwidth_profile_id INTEGER REFERENCES bandwidth_profiles(id);
CREATE INDEX idx_forward_rules_bandwidth_profile_id
    ON forward_rules (bandwidth_profile_id);
