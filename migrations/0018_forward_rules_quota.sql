-- 对标 flux User.num / UserTunnel.num:用户/隧道级"可创建转发规则条数"配额。
-- users.forward_rules_quota:该用户名下可创建的转发规则总数上限;NULL = 不限(存量用户不受影响)。
-- user_tunnel_grants.forward_rules_limit_in_tunnel:该用户在该隧道下可建的转发规则数上限;
--   NULL = 不限(仅受 users.forward_rules_quota 全局约束)。
-- 仅在创建规则时由应用层 COUNT 对比校验(软删规则不计入);0/负值在 routes 层归一为"不限"。
-- 兼容性:沿用可空 INTEGER,迁移 PG 无需改动。
ALTER TABLE users ADD COLUMN forward_rules_quota INTEGER;
ALTER TABLE user_tunnel_grants ADD COLUMN forward_rules_limit_in_tunnel INTEGER;
