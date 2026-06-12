-- 隧道凭据轮换:记录最近一次 hop 证书签发下发时间(NULL = 未下发过,按 created_at 回落)。
-- 配合短有效期证书(30 天)与轮换 sweeper:超过阈值自动重签下发并重启隧道规则。
ALTER TABLE tunnels ADD COLUMN creds_rotated_at TEXT;
