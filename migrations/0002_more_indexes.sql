-- P2 补全 created_at 索引,与 forward_rules / audit_logs 一致。
-- 适用场景: "最近 N 天创建"类聚合查询 / dashboard 时间段统计。
-- IF NOT EXISTS 让 0001 跑过的库重复跑这一条不报错。
CREATE INDEX IF NOT EXISTS idx_users_created_at ON users (created_at);
CREATE INDEX IF NOT EXISTS idx_nodes_created_at ON nodes (created_at);
