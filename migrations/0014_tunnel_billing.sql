-- P1(对标 flux): 隧道级流量倍率 + 单/双向计费。
-- 中转成本不同的隧道(直连 vs 三跳)可设不同倍率;计费时换算,原始 rule_stats 不变。
-- 默认值 = 当前行为(倍率 1.0、双向 rx+tx),存量隧道语义不变。
-- traffic_ratio: 计费乘数(0=免费,1=原样,2=双倍...);REAL 兼容 SQLite/PG。
-- billing_mode: 2=双向(rx+tx),1=单向(只计较大方向,防刷)。
ALTER TABLE tunnels ADD COLUMN traffic_ratio REAL NOT NULL DEFAULT 1.0;
ALTER TABLE tunnels ADD COLUMN billing_mode INTEGER NOT NULL DEFAULT 2;
