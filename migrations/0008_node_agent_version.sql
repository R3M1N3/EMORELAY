-- 节点 Agent 上报的版本号(register 时落库)。空串 = 尚未注册过。
ALTER TABLE nodes ADD COLUMN agent_version TEXT NOT NULL DEFAULT '';
