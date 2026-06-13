-- P1(对标 flux flow_reset_time): 月度固定日流量重置(可选,与滚动 30 天并存)。
-- NULL = 沿用滚动 30 天窗口(默认,存量用户不受影响);1-31 = 每月该日 0 点起算本期用量
-- (月末容错:取 min(该值, 当月天数))。计费口径仍是 traffic_limit_bytes_30d 那个上限值。
ALTER TABLE users ADD COLUMN quota_reset_day INTEGER;
