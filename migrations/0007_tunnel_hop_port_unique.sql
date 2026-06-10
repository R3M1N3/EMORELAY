-- 0007: tunnel_hop inter_port 并发兜底。同一节点上同一 inter_port 不可被两个 hop 占用
-- (并发建隧道复用同节点时,check-then-insert 竞态的第二条 INSERT 撞此唯一索引 → 回滚 → 400)。
-- entry hop 的 inter_port 为 NULL,部分索引不约束 NULL,允许多个 entry。PG 语法一致。
CREATE UNIQUE INDEX idx_tunnel_hops_node_inter_port
    ON tunnel_hops (node_id, inter_port) WHERE inter_port IS NOT NULL;
