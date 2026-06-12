-- P10a: 规则级并发连接数上限(仅 TCP 生效;UDP 无连接语义不适用)。
-- NULL = 不限;>0 = Agent 端达到上限时拒绝新连接。admin 管控资产,普通用户不可自配。
ALTER TABLE forward_rules ADD COLUMN max_connections INTEGER;
