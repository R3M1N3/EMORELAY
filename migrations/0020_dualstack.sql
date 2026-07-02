-- v4/v6 双栈支持：规则级出站地址族偏好 + 节点网络能力上报。
-- remote_af: "auto"(默认,不过滤) / "v4"(仅 IPv4 出站) / "v6"(仅 IPv6 出站)。
-- NOT NULL DEFAULT 'auto'：存量规则行为不变。PG 兼容。
ALTER TABLE forward_rules ADD COLUMN remote_af TEXT NOT NULL DEFAULT 'auto';
-- 节点 IPv4/IPv6 网络能力(Agent heartbeat 上报)。NULL=未知(旧 agent),0=无,1=有。
ALTER TABLE nodes ADD COLUMN has_ipv4 INTEGER;
ALTER TABLE nodes ADD COLUMN has_ipv6 INTEGER;
