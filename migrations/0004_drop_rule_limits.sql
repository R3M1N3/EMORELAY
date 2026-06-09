-- migrations/0004_drop_rule_limits.sql
-- Phase 2 减法:规则级限制三列下线(语义已迁移至 users.expires_at /
-- users.traffic_limit_bytes_30d / forward_rules.bandwidth_profile_id)。
-- SQLite 3.35+ 原生 DROP COLUMN(sqlx bundled sqlite 满足);PG 语法一致。
ALTER TABLE forward_rules DROP COLUMN expires_at;
ALTER TABLE forward_rules DROP COLUMN traffic_limit_bytes;
ALTER TABLE forward_rules DROP COLUMN bandwidth_limit_mbps;

-- 孤儿配置 key:其语义依附于已删除的规则级字段。
DELETE FROM system_settings
WHERE key IN ('default_traffic_limit_bytes', 'default_bandwidth_limit_mbps');

-- 用户级 sweeper(Task 5)按 expires_at 扫描;部分索引只覆盖设了到期的行。
CREATE INDEX idx_users_expires_at ON users (expires_at) WHERE expires_at IS NOT NULL;
