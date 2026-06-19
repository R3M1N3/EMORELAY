-- realm-parity:规则级「向上游发送 PROXY protocol v1 头」开关(透传真实客户端 IP)。
-- 仅非隧道 TCP relay 生效;admin 管控字段。0 = 关(默认,存量规则不受影响);1 = 开。
-- NOT NULL DEFAULT 0:ALTER ADD 要求带默认值;PG 兼容。
ALTER TABLE forward_rules ADD COLUMN send_proxy_protocol INTEGER NOT NULL DEFAULT 0;
