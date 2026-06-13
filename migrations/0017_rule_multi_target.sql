-- P2(对标 flux 多目标负载均衡): 一条规则可配多个目标 + 负载策略。
-- 向后兼容:target_host/target_port 仍是主目标(第 1 个);extra_targets 存额外目标
-- (JSON 数组 [{"host":..,"port":..}],NULL/空 = 单目标,行为不变)。
-- lb_strategy: fifo(主备故障转移,默认)/round(轮询)/rand(随机)/hash(按客户端 IP)。
-- 仅在目标数 > 1 时生效。TEXT 存 JSON 保 SQLite/PG 兼容(应用层解析,不入 SQL 查询)。
ALTER TABLE forward_rules ADD COLUMN extra_targets TEXT;
ALTER TABLE forward_rules ADD COLUMN lb_strategy TEXT NOT NULL DEFAULT 'fifo';
