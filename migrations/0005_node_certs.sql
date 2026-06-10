-- migrations/0005_node_certs.sql
-- P3a:节点 mTLS 客户端证书元数据。DB 只存 serial + fingerprint(审计 + 吊销),
-- 绝不存私钥明文(明文仅在创建/轮换响应里一次性返回)。
-- PG 迁移:ADD COLUMN 语法一致。
ALTER TABLE nodes ADD COLUMN cert_serial TEXT;
ALTER TABLE nodes ADD COLUMN cert_fingerprint TEXT;
CREATE INDEX idx_nodes_cert_fingerprint ON nodes (cert_fingerprint)
    WHERE cert_fingerprint IS NOT NULL;
