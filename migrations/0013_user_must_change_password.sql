-- 首登强制改密(对标 flux 的 requirePasswordChange)。
-- 1 = 用户下次登录前必须改密(admin 新建/重置密码时置位);0 = 无需。
-- 默认 0:存量用户已在用自己的密码,不受影响;仅新建/被重置的账号被强制。
-- 用 INTEGER 而非 BOOLEAN 保持 SQLite/PG 兼容(全库布尔统一用 0/1)。
ALTER TABLE users ADD COLUMN must_change_password INTEGER NOT NULL DEFAULT 0;
