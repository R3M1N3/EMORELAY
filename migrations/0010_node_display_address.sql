-- P8: 节点双地址。public_ip 语义收敛为「接入地址」(域名/IP,Agent 与隧道 hop 互联实际
-- 使用,被 dial 的 hop 必填);新增 display_address「展示地址」(可选,对普通用户展示的
-- 入口地址,空则回落接入地址)。覆盖 NAT + DDNS 场景:接入走 DDNS 域名,展示给用户落地 IP。
ALTER TABLE nodes ADD COLUMN display_address TEXT NOT NULL DEFAULT '';
