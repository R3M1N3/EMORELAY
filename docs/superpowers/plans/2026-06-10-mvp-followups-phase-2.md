# Phase 2 · 用户与限速重构 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 实现 Spec §3 的 5 项结构性重构：端口自动分配 / 到期搬到用户 / 流量配额到用户（滚动 30 天）/ 限速独立路由（bandwidth_profiles + Agent token bucket）/ 规则导入导出，并把规则级 `expires_at` / `traffic_limit_bytes` / `bandwidth_limit_mbps` 全链路下线。

**Architecture:** 两个 migration（0003 加法 / 0004 减法）把限制语义从 forward_rules 搬到 users + bandwidth_profiles；panel-server 新增 user_quota sweeper（双 interval）取代规则级 expiry sweeper / auto_stop；protobuf Rule 重排（reserved 8-10，新增 bandwidth_mbps=11）；Agent 新增 limit/token_bucket 并接入 TCP/UDP relay hot path；REST 新增 bandwidth-profiles CRUD 与 rules export/import；前端新增 /bandwidth-profiles 页并改造 Users / Rules / RuleDetail / Settings。

**Tech Stack:** Rust (Axum + SQLx/SQLite + tonic/prost + tokio time) / React 19 + TS + Tailwind 4 / vitest + tower::ServiceExt 集成测试。

**与 spec 的三处落地差异（已核实代码现状，按此执行）：**
1. spec §3.2 写 `migrations/0002_phase2.sql`，但 `0002_more_indexes.sql` 已存在（users/nodes created_at 索引也已由它完成）。本计划用 **`0003_phase2.sql`（纯加法）+ `0004_drop_rule_limits.sql`（纯减法）**，使 Task 1 与 Task 4 各自独立可编译交付。
2. spec §3.4 登录代码引用 `ApiError::Unauthorized("account_expired".into())`，但现有 `ApiError::Unauthorized` 是无参变体。本计划在 error.rs **新增 `UnauthorizedMsg(String)` 变体**（401 + 自定义 message），不动现有调用点。
3. spec §3.6 export 有 `tunnel_id` query 参数；隧道是 P3 功能，P2 **不实现该参数**（导出 JSON 中 `tunnel_name` 字段恒为 null 以保持格式前向兼容；import 遇到非空 tunnel_name 报 error，按 spec 执行）。
4. spec §3.5 写 `burst = rate / 5`；本计划用 **`burst = max(rate/5, 65536)`**——低速率（<2.6Mbps）下纯 rate/5 的 burst 小于 UDP 最大单包 65535，`try_acquire` 将永久拒绝导致大包全丢；64KB 下限保证任何速率下最大 UDP 包都有机会放行。

**全局约定：**
- 时间一律按 **UTC** 解释与存储，格式 `YYYY-MM-DD HH:MM:SS`（与 `datetime('now')`、rule_stats.bucket_at 一致，可直接字符串比较）。前端 datetime-local 输入在 label 标注 (UTC)。
- PATCH 置空协议（现有 COALESCE 模式不支持 NULL）：`expires_at` 传 `""` = 清除；`traffic_limit_bytes_30d` 传 `0` = 清除；`bandwidth_profile_id` 传 `0` = 解除关联。
- 每个 Task 完成后：跑该 Task 的验证命令 → commit → spawn 子代理 review（CLAUDE.md 流程）→ 通过才进下一 Task。

---

## 文件结构（变更面）

**Create:**
- `migrations/0003_phase2.sql` — users 4 列 + bandwidth_profiles 表 + forward_rules.bandwidth_profile_id（加法）
- `migrations/0004_drop_rule_limits.sql` — forward_rules DROP 三列 + 清理两个孤儿 settings key（减法）
- `crates/panel-server/src/models/bandwidth_profile.rs` — BandwidthProfile model
- `crates/panel-server/src/routes/bandwidth_profiles.rs` — CRUD + 引用保护
- `crates/panel-server/src/routes/rules_io.rs` — export / import
- `crates/panel-server/src/sweeper/mod.rs` + `sweeper/user_quota.rs` — 用户到期/配额 sweeper
- `crates/panel-server/tests/api_bandwidth_profiles.rs`
- `crates/panel-server/tests/api_rules_port_alloc.rs`
- `crates/panel-server/tests/api_rules_io.rs`
- `crates/panel-server/tests/user_quota_sweeper.rs`
- `crates/node-agent/src/limit/mod.rs` + `limit/token_bucket.rs` — token bucket
- `web/src/pages/BandwidthProfiles.tsx`
- `web/src/lib/quota.ts` + `web/src/lib/quota.test.ts` — 30d 用量进度条配色纯函数

**Modify:**
- `crates/common/proto/control.proto` — Rule 字段 8/9/10 → reserved；新增 `int64 bandwidth_mbps = 11`
- `crates/panel-server/src/models/user.rs` — 4 新字段 + update 置空协议
- `crates/panel-server/src/models/rule.rs` — 删三字段；加 bandwidth_profile_id + bandwidth_mbps 派生列（子查询）
- `crates/panel-server/src/routes/users.rs` — DTO/校验/列表 SQL 扩展
- `crates/panel-server/src/routes/auth.rs` — login 过期拒绝
- `crates/panel-server/src/routes/rules.rs` — DTO 删三字段加 bandwidth_profile_id；listen_port Option + allocate_port
- `crates/panel-server/src/routes/system.rs` — ALLOWED 删两个孤儿 key
- `crates/panel-server/src/routes/mod.rs` — 注册 bandwidth-profiles / export / import 路由
- `crates/panel-server/src/error.rs` — `UnauthorizedMsg(String)` 变体
- `crates/panel-server/src/util.rs` — `normalize_datetime`
- `crates/panel-server/src/grpc/commands.rs` — rule_to_proto 用 bandwidth_mbps
- `crates/panel-server/src/grpc/service.rs` — 删 auto_stop_if_exceeded / spawn_expiry_sweeper
- `crates/panel-server/src/main.rs` — 换 spawn user_quota sweeper
- `crates/panel-server/src/lib.rs` — `pub mod sweeper`
- `crates/panel-server/tests/api_rules.rs` / `tests/api_users.rs` / `tests/agent_e2e.rs` — 字段同步
- `crates/node-agent/src/main.rs` — `mod limit`
- `crates/node-agent/src/manager.rs` — 创建 TokenBucket 传入 relay
- `crates/node-agent/src/relay/tcp.rs` — bridge 限速分支（chunk 循环）
- `crates/node-agent/src/relay/udp.rs` — try_acquire 丢包
- `crates/node-agent/src/relay/traits.rs` — 删除（QuotaGuard 占位被 token_bucket 取代）
- `crates/node-agent/src/relay/mod.rs` — 去 traits 声明
- `crates/node-agent/src/store.rs` — RuleJson 字段同步（serde default 兼容旧状态文件）
- `web/src/lib/api.ts` — 类型与端点扩展
- `web/src/pages/Users.tsx` / `Rules.tsx` / `RuleDetail.tsx` / `Settings.tsx` / `Login.tsx` / `App.tsx`
- `scripts/seed-dev.py` — 删三字段
- `.env.example` — 删 PANEL_EXPIRY_SWEEP_SECS；加 PANEL_USER_EXPIRY_SWEEP_SECS / PANEL_USER_QUOTA_SWEEP_SECS
- `docs/api.md` / `README.md` / `docs/deployment.md` / `plan.md` 附录

---

## Task 1: Migration 0003（加法）+ User/Rule model 扩展

**Files:**
- Create: `migrations/0003_phase2.sql`
- Modify: `crates/panel-server/src/models/user.rs`
- Modify: `crates/panel-server/src/models/rule.rs`
- Test: 现有 `cargo test --workspace` 全绿即为本 Task 验收（纯加法不破坏行为）

- [ ] **Step 1: 写 migration 0003**

```sql
-- migrations/0003_phase2.sql
-- Phase 2 加法部分：限制语义从 forward_rules 搬往 users / bandwidth_profiles。
-- 减法（DROP 三列）在 0004_drop_rule_limits.sql，等代码侧不再引用后执行。
-- PG 迁移路径：ALTER TABLE ... ADD COLUMN 与部分唯一索引语法一致；
-- datetime('now') 换 now()；INTEGER 布尔/外键语义不变。

-- users 扩展：到期 + 滚动 30 天流量配额 + 用量缓存
ALTER TABLE users ADD COLUMN expires_at TEXT;
ALTER TABLE users ADD COLUMN traffic_limit_bytes_30d INTEGER;
ALTER TABLE users ADD COLUMN period_used_bytes_cached INTEGER NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN period_used_calculated_at TEXT;

-- 限速 profile（独立路由可复用）
CREATE TABLE bandwidth_profiles (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    name            TEXT    NOT NULL,
    bandwidth_mbps  INTEGER NOT NULL CHECK (bandwidth_mbps > 0),
    description     TEXT    NOT NULL DEFAULT '',
    created_at      TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT    NOT NULL DEFAULT (datetime('now')),
    deleted_at      TEXT
);
CREATE UNIQUE INDEX idx_bandwidth_profiles_name_active
    ON bandwidth_profiles (name) WHERE deleted_at IS NULL;

ALTER TABLE forward_rules
    ADD COLUMN bandwidth_profile_id INTEGER REFERENCES bandwidth_profiles(id);
CREATE INDEX idx_forward_rules_bandwidth_profile_id
    ON forward_rules (bandwidth_profile_id);
```

- [ ] **Step 2: 扩展 User struct + SELECT 列 + update 置空协议**

`crates/panel-server/src/models/user.rs` 全部 4 处 SELECT 列表（find_by_username / find_by_id / list_paged）把
`id, username, password_hash, role, created_at, updated_at, deleted_at`
替换为
`id, username, password_hash, role, expires_at, traffic_limit_bytes_30d, period_used_bytes_cached, period_used_calculated_at, created_at, updated_at, deleted_at`

struct 加字段（放在 `role` 之后）：

```rust
    pub expires_at: Option<String>,
    pub traffic_limit_bytes_30d: Option<i64>,
    pub period_used_bytes_cached: i64,
    pub period_used_calculated_at: Option<String>,
```

`update` 方法改为支持置空协议（None=不改；expires_at 传 ""=清除；limit 传 0=清除）：

```rust
    /// 部分更新:None 字段不变,Some 字段写入。updated_at 由本方法刷新。
    /// 置空协议:expires_at 传 "" 清除;traffic_limit_bytes_30d 传 0 清除。
    pub async fn update(
        pool: &SqlitePool,
        id: i64,
        password_hash: Option<&str>,
        role: Option<&str>,
        expires_at: Option<&str>,
        traffic_limit_bytes_30d: Option<i64>,
    ) -> sqlx::Result<u64> {
        let res = sqlx::query(
            "UPDATE users SET \
                password_hash = COALESCE(?, password_hash), \
                role = COALESCE(?, role), \
                expires_at = CASE \
                    WHEN ?3 IS NULL THEN expires_at \
                    WHEN ?3 = '' THEN NULL \
                    ELSE ?3 END, \
                traffic_limit_bytes_30d = CASE \
                    WHEN ?4 IS NULL THEN traffic_limit_bytes_30d \
                    WHEN ?4 = 0 THEN NULL \
                    ELSE ?4 END, \
                updated_at = datetime('now') \
             WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(password_hash)
        .bind(role)
        .bind(expires_at)
        .bind(traffic_limit_bytes_30d)
        .bind(id)
        .execute(pool)
        .await?;
        Ok(res.rows_affected())
    }
```

注意：SQLite 位置参数 `?3`/`?4` 与无序 `?` 混用不可靠，**统一全部改为 `?1`-`?5` 显式编号**（`password_hash`=?1, `role`=?2, `expires_at`=?3, `traffic_limit_bytes_30d`=?4, `id`=?5）。

`create` 方法不动（新用户两字段由 routes 层在 create 后跟一次 update，或直接扩展 create——为最少改动**扩展 create**）：

```rust
    pub async fn create(
        pool: &SqlitePool,
        username: &str,
        password_hash: &str,
        role: &str,
        expires_at: Option<&str>,
        traffic_limit_bytes_30d: Option<i64>,
    ) -> sqlx::Result<i64> {
        let res = sqlx::query(
            "INSERT INTO users (username, password_hash, role, expires_at, traffic_limit_bytes_30d) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(username)
        .bind(password_hash)
        .bind(role)
        .bind(expires_at)
        .bind(traffic_limit_bytes_30d)
        .execute(pool)
        .await?;
        Ok(res.last_insert_rowid())
    }
```

同步修所有 `User::create` / `User::update` 调用点编译错误（已 grep 确认仅这四处）：
- `routes/users.rs:162` create → `User::create(&state.pool, username, &hash, &req.role, None, None)`（Task 2 再接真值）
- `routes/users.rs::update` 的 `User::update(...)` 调用 → 尾部补 `, None, None`（Task 2 再接真值）
- `bootstrap.rs:26` → `User::create(pool, &username, &hash, "admin", None, None)`
- `tests/common/mod.rs:66` 与 `:88` 两处 `User::create(...)` 补 `, None, None`

- [ ] **Step 3: Rule struct 加 bandwidth_profile_id + bandwidth_mbps 派生列**

`crates/panel-server/src/models/rule.rs`：

struct 在 `connection_count` 之后加：

```rust
    pub bandwidth_profile_id: Option<i64>,
    /// 派生列:关联 profile 的 Mbps(活跃 profile);无关联/已删 → None。
    pub bandwidth_mbps: Option<i64>,
```

`RULE_COLUMNS` 改为（保持现有列 + 追加两列；本 Task 还不删三旧列）：

```rust
const RULE_COLUMNS: &str = "id, user_id, node_id, name, protocol, listen_ip, listen_port, \
    target_host, target_port, enabled, expires_at, traffic_limit_bytes, bandwidth_limit_mbps, \
    rx_bytes, tx_bytes, connection_count, bandwidth_profile_id, \
    (SELECT bp.bandwidth_mbps FROM bandwidth_profiles bp \
        WHERE bp.id = forward_rules.bandwidth_profile_id AND bp.deleted_at IS NULL) AS bandwidth_mbps, \
    created_at, updated_at";
```

- [ ] **Step 4: 跑测试验证全绿**

Run: `cargo test --workspace`
Expected: 全部 PASS（47 panel-server + 3 node-agent 及其余）。migration 0003 在每个测试的 `run_migrations` 中自动执行。

- [ ] **Step 5: Commit**

```bash
git add migrations/0003_phase2.sql crates/panel-server/src/models/user.rs crates/panel-server/src/models/rule.rs crates/panel-server/src/routes/users.rs crates/panel-server/tests/common/mod.rs
git commit -m "feat(db): phase2 additive migration - user quota fields + bandwidth_profiles"
```

## Task 2: users API 扩展 + 登录到期拒绝

**Files:**
- Modify: `crates/panel-server/src/error.rs`、`crates/panel-server/src/util.rs`
- Modify: `crates/panel-server/src/routes/users.rs`、`crates/panel-server/src/routes/auth.rs`
- Test: `crates/panel-server/tests/api_users.rs`（扩展）

- [ ] **Step 1: 写失败测试**

`crates/panel-server/tests/api_users.rs` 追加：

```rust
#[tokio::test]
async fn user_quota_fields_roundtrip() {
    let app = common::make_app().await.unwrap();
    // create 带 expires_at + traffic_limit_bytes_30d
    let req = common::auth_req(
        Method::POST,
        "/api/users",
        &app.admin_token,
        Some(json!({
            "username": "quotauser",
            "password": "password123",
            "role": "user",
            "expires_at": "2030-01-01T00:00",
            "traffic_limit_bytes_30d": 1073741824_i64
        })),
    )
    .unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    let uid = body["id"].as_i64().unwrap();
    // normalize 后统一空格分隔格式
    assert_eq!(body["expires_at"], "2030-01-01 00:00:00");
    assert_eq!(body["traffic_limit_bytes_30d"], 1073741824_i64);
    assert_eq!(body["period_used_bytes_cached"], 0);
    assert_eq!(body["period_remaining_bytes"], 1073741824_i64);

    // PATCH 置空协议:expires_at="" 清除;limit=0 清除
    let req = common::auth_req(
        Method::PATCH,
        &format!("/api/users/{uid}"),
        &app.admin_token,
        Some(json!({ "expires_at": "", "traffic_limit_bytes_30d": 0 })),
    )
    .unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["expires_at"].is_null());
    assert!(body["traffic_limit_bytes_30d"].is_null());
    assert!(body["period_remaining_bytes"].is_null());
}

#[tokio::test]
async fn user_create_rejects_bad_expires_format() {
    let app = common::make_app().await.unwrap();
    let req = common::auth_req(
        Method::POST,
        "/api/users",
        &app.admin_token,
        Some(json!({
            "username": "badexp", "password": "password123", "role": "user",
            "expires_at": "not-a-date"
        })),
    )
    .unwrap();
    let (status, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn expired_user_cannot_login() {
    let app = common::make_app().await.unwrap();
    // 直接建一个已过期用户
    let req = common::auth_req(
        Method::POST,
        "/api/users",
        &app.admin_token,
        Some(json!({
            "username": "expired1", "password": "password123", "role": "user",
            "expires_at": "2020-01-01 00:00:00"
        })),
    )
    .unwrap();
    let (status, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);

    let login = Request::post("/api/auth/login")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&json!({ "username": "expired1", "password": "password123" }))
                .unwrap(),
        ))
        .unwrap();
    let (status, body) = common::send(app.app.clone(), login).await.unwrap();
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["message"], "account_expired");
}
```

文件顶部 import 需有 `axum::http::{Method, Request, StatusCode}`、`axum::body::Body`、`serde_json::json`（与现有 api_users.rs 顶部对齐，缺哪个补哪个）。

- [ ] **Step 2: 跑测试验证失败**

Run: `cargo test -p panel-server --test api_users`
Expected: 新增 3 个测试 FAIL（字段不存在 / 400 未触发 / 登录未拒）。

- [ ] **Step 3: error.rs 加变体 + util.rs 加 normalize_datetime**

`error.rs` 在 `Unauthorized` 之后加变体，并在 `into_response` 的 match 中处理：

```rust
    #[error("{0}")]
    UnauthorizedMsg(String),
```

```rust
            Self::Unauthorized | Self::UnauthorizedMsg(_) => {
                (StatusCode::UNAUTHORIZED, "unauthorized")
            }
```

（替换原 `Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),` 一行。）

`util.rs` 追加：

```rust
/// 规范化用户输入的到期时间为 SQLite `datetime('now')` 同款格式（UTC 语义）。
/// 接受 "YYYY-MM-DDTHH:MM"(datetime-local) / "YYYY-MM-DDTHH:MM:SS" / "YYYY-MM-DD HH:MM:SS"。
/// 统一输出 "YYYY-MM-DD HH:MM:SS"，可与 datetime('now') 直接字符串比较。
pub fn normalize_datetime(s: &str) -> Option<String> {
    let s = s.trim();
    const FORMATS: &[&str] = &["%Y-%m-%dT%H:%M", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S"];
    for f in FORMATS {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, f) {
            return Some(dt.format("%Y-%m-%d %H:%M:%S").to_string());
        }
    }
    None
}
```

- [ ] **Step 4: routes/users.rs 扩展 DTO / handlers**

```rust
#[derive(Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub password: String,
    pub role: String,
    pub expires_at: Option<String>,
    pub traffic_limit_bytes_30d: Option<i64>,
}

#[derive(Deserialize)]
pub struct UpdateUserRequest {
    pub password: Option<String>,
    pub role: Option<String>,
    /// "" = 清除
    pub expires_at: Option<String>,
    /// 0 = 清除
    pub traffic_limit_bytes_30d: Option<i64>,
}
```

`UserView` 加字段（`total_traffic_bytes` 之后）：

```rust
    pub expires_at: Option<String>,
    pub traffic_limit_bytes_30d: Option<i64>,
    pub period_used_bytes_cached: i64,
    pub period_used_calculated_at: Option<String>,
    /// 计算字段:max(0, limit - used);limit 为 NULL 时为 None。
    pub period_remaining_bytes: Option<i64>,
}
```

`From<User>` 与 `From<UserListRow>` 同步填充；`UserListRow` 加同名 4 列；remaining 统一用 helper：

```rust
fn remaining(limit: Option<i64>, used: i64) -> Option<i64> {
    limit.map(|l| (l - used).max(0))
}
```

`From<User>`：

```rust
            expires_at: u.expires_at.clone(),
            traffic_limit_bytes_30d: u.traffic_limit_bytes_30d,
            period_used_bytes_cached: u.period_used_bytes_cached,
            period_used_calculated_at: u.period_used_calculated_at.clone(),
            period_remaining_bytes: remaining(u.traffic_limit_bytes_30d, u.period_used_bytes_cached),
```

（`From<UserListRow>` 同构。注意 From 消费 self，可直接 move 不必 clone——按编译器指引取舍。）

list 的 SQL SELECT 列表加 `u.expires_at, u.traffic_limit_bytes_30d, u.period_used_bytes_cached, u.period_used_calculated_at`。

create handler 在 `validate_role` 之后加校验 + 传参：

```rust
    let normalized_expires = match req.expires_at.as_deref() {
        None | Some("") => None,
        Some(s) => Some(crate::util::normalize_datetime(s).ok_or_else(|| {
            ApiError::BadRequest("expires_at must be YYYY-MM-DDTHH:MM (UTC)".into())
        })?),
    };
    if matches!(req.traffic_limit_bytes_30d, Some(n) if n < 0) {
        return Err(ApiError::BadRequest("traffic_limit_bytes_30d must be >= 0".into()));
    }
    // create 时 0 与 None 等价(都是不限)
    let limit = req.traffic_limit_bytes_30d.filter(|n| *n > 0);
    let new_id = User::create(&state.pool, username, &hash, &req.role, normalized_expires.as_deref(), limit)
        .await
        .map_err(map_sqlx_to_api)?;
```

update handler 在 role 校验块之后加：

```rust
    // 置空协议:"" 原样传给 model 层(CASE WHEN '' THEN NULL);其余 normalize。
    let normalized_expires: Option<String> = match req.expires_at.as_deref() {
        None => None,
        Some("") => Some(String::new()),
        Some(s) => Some(crate::util::normalize_datetime(s).ok_or_else(|| {
            ApiError::BadRequest("expires_at must be YYYY-MM-DDTHH:MM (UTC)".into())
        })?),
    };
    if matches!(req.traffic_limit_bytes_30d, Some(n) if n < 0) {
        return Err(ApiError::BadRequest("traffic_limit_bytes_30d must be >= 0".into()));
    }
```

`User::update` 调用改为：

```rust
    let rows = User::update(
        &state.pool,
        id,
        new_hash.as_deref(),
        req.role.as_deref(),
        normalized_expires.as_deref(),
        req.traffic_limit_bytes_30d,
    )
    .await?;
```

- [ ] **Step 5: auth.rs login 过期检查**

在 `verify_password` 通过（`if !ok` 块之后）、`encode_jwt` 之前插入：

```rust
    // 账号到期拒登录(P2):normalize 后的存储格式可被 parse_sqlite_datetime 解析。
    if let Some(exp) = user.expires_at.as_deref() {
        let ts = crate::grpc::commands::parse_sqlite_datetime(exp);
        if ts > 0 && ts <= chrono::Utc::now().timestamp() {
            audit::record_with_ip(
                &state.pool,
                Some(user.id),
                actor_ip.as_option(),
                "auth.login",
                Some("user"),
                Some(user.id),
                None,
                false,
                Some("account_expired"),
            )
            .await;
            return Err(ApiError::UnauthorizedMsg("account_expired".into()));
        }
    }
```

- [ ] **Step 6: 跑测试验证通过**

Run: `cargo test -p panel-server --test api_users && cargo test --workspace`
Expected: 全 PASS。

- [ ] **Step 7: Commit**

```bash
git add crates/panel-server/src/error.rs crates/panel-server/src/util.rs crates/panel-server/src/routes/users.rs crates/panel-server/src/routes/auth.rs crates/panel-server/src/models/user.rs crates/panel-server/tests/api_users.rs
git commit -m "feat(server): user expires_at + 30d traffic quota fields; reject expired login"
```

---

## Task 3: bandwidth_profiles model + CRUD API

**Files:**
- Create: `crates/panel-server/src/models/bandwidth_profile.rs`
- Create: `crates/panel-server/src/routes/bandwidth_profiles.rs`
- Modify: `crates/panel-server/src/models/mod.rs`、`crates/panel-server/src/routes/mod.rs`
- Test: `crates/panel-server/tests/api_bandwidth_profiles.rs`

- [ ] **Step 1: 写失败测试**

```rust
// crates/panel-server/tests/api_bandwidth_profiles.rs
mod common;

use axum::http::{Method, StatusCode};
use serde_json::json;

#[tokio::test]
async fn bandwidth_profile_crud_roundtrip() {
    let app = common::make_app().await.unwrap();
    // create
    let req = common::auth_req(
        Method::POST,
        "/api/bandwidth-profiles",
        &app.admin_token,
        Some(json!({ "name": "100m-shared", "bandwidth_mbps": 100, "description": "公用 100M" })),
    )
    .unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = body["id"].as_i64().unwrap();
    assert_eq!(body["bandwidth_mbps"], 100);

    // list
    let req = common::auth_req(Method::GET, "/api/bandwidth-profiles", &app.admin_token, None).unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 1);
    assert_eq!(body["items"][0]["name"], "100m-shared");

    // patch
    let req = common::auth_req(
        Method::PATCH,
        &format!("/api/bandwidth-profiles/{id}"),
        &app.admin_token,
        Some(json!({ "bandwidth_mbps": 50 })),
    )
    .unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["bandwidth_mbps"], 50);

    // delete(无引用)
    let req = common::auth_req(
        Method::DELETE,
        &format!("/api/bandwidth-profiles/{id}"),
        &app.admin_token,
        None,
    )
    .unwrap();
    let (status, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);

    // get 已删 → 404
    let req = common::auth_req(
        Method::GET,
        &format!("/api/bandwidth-profiles/{id}"),
        &app.admin_token,
        None,
    )
    .unwrap();
    let (status, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn bandwidth_profile_rejects_dup_name_and_bad_mbps() {
    let app = common::make_app().await.unwrap();
    for _ in 0..2 {
        let req = common::auth_req(
            Method::POST,
            "/api/bandwidth-profiles",
            &app.admin_token,
            Some(json!({ "name": "dup", "bandwidth_mbps": 10 })),
        )
        .unwrap();
        let (status, _) = common::send(app.app.clone(), req).await.unwrap();
        if status != StatusCode::OK {
            assert_eq!(status, StatusCode::BAD_REQUEST);
            break;
        }
    }
    let req = common::auth_req(
        Method::POST,
        "/api/bandwidth-profiles",
        &app.admin_token,
        Some(json!({ "name": "zero", "bandwidth_mbps": 0 })),
    )
    .unwrap();
    let (status, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn bandwidth_profile_delete_blocked_by_rule_reference() {
    let app = common::make_app().await.unwrap();
    // 建 profile
    let req = common::auth_req(
        Method::POST,
        "/api/bandwidth-profiles",
        &app.admin_token,
        Some(json!({ "name": "ref", "bandwidth_mbps": 30 })),
    )
    .unwrap();
    let (_, body) = common::send(app.app.clone(), req).await.unwrap();
    let pid = body["id"].as_i64().unwrap();

    // 直接在 DB 建 node + 引用规则(避开 rules API 依赖)
    sqlx::query("INSERT INTO nodes (name, agent_token_hash) VALUES ('n1', 'x')")
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO forward_rules (user_id, node_id, name, protocol, listen_ip, listen_port, target_host, target_port, bandwidth_profile_id) \
         VALUES (?, 1, 'r1', 'tcp', '0.0.0.0', 20001, '1.2.3.4', 443, ?)",
    )
    .bind(app.admin_user_id)
    .bind(pid)
    .execute(&app.state.pool)
    .await
    .unwrap();

    let req = common::auth_req(
        Method::DELETE,
        &format!("/api/bandwidth-profiles/{pid}"),
        &app.admin_token,
        None,
    )
    .unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["message"].as_str().unwrap().contains("1"), "应包含引用规则数: {body}");
}

#[tokio::test]
async fn bandwidth_profiles_require_admin() {
    let app = common::make_app().await.unwrap();
    let (_uid, token) = common::make_user_token(&app, "normal1", "password123").await.unwrap();
    let req = common::auth_req(Method::GET, "/api/bandwidth-profiles", &token, None).unwrap();
    let (status, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::FORBIDDEN);
}
```

- [ ] **Step 2: 跑测试验证失败**

Run: `cargo test -p panel-server --test api_bandwidth_profiles`
Expected: FAIL（404 路由不存在）。

- [ ] **Step 3: model 实现**

```rust
// crates/panel-server/src/models/bandwidth_profile.rs
use sqlx::{prelude::FromRow, SqlitePool};

#[derive(Debug, Clone, FromRow)]
pub struct BandwidthProfile {
    pub id: i64,
    pub name: String,
    pub bandwidth_mbps: i64,
    pub description: String,
    pub created_at: String,
    pub updated_at: String,
}

const COLUMNS: &str = "id, name, bandwidth_mbps, description, created_at, updated_at";

impl BandwidthProfile {
    pub async fn list_paged(pool: &SqlitePool, limit: i64, offset: i64) -> sqlx::Result<Vec<Self>> {
        let sql = format!(
            "SELECT {COLUMNS} FROM bandwidth_profiles WHERE deleted_at IS NULL \
             ORDER BY id DESC LIMIT ? OFFSET ?"
        );
        sqlx::query_as(&sql).bind(limit).bind(offset).fetch_all(pool).await
    }

    pub async fn count(pool: &SqlitePool) -> sqlx::Result<i64> {
        sqlx::query_scalar("SELECT COUNT(*) FROM bandwidth_profiles WHERE deleted_at IS NULL")
            .fetch_one(pool)
            .await
    }

    pub async fn find_by_id(pool: &SqlitePool, id: i64) -> sqlx::Result<Option<Self>> {
        let sql = format!(
            "SELECT {COLUMNS} FROM bandwidth_profiles WHERE id = ? AND deleted_at IS NULL"
        );
        sqlx::query_as(&sql).bind(id).fetch_optional(pool).await
    }

    pub async fn find_by_name(pool: &SqlitePool, name: &str) -> sqlx::Result<Option<Self>> {
        let sql = format!(
            "SELECT {COLUMNS} FROM bandwidth_profiles WHERE name = ? AND deleted_at IS NULL"
        );
        sqlx::query_as(&sql).bind(name).fetch_optional(pool).await
    }

    pub async fn create(
        pool: &SqlitePool,
        name: &str,
        bandwidth_mbps: i64,
        description: &str,
    ) -> sqlx::Result<i64> {
        let res = sqlx::query(
            "INSERT INTO bandwidth_profiles (name, bandwidth_mbps, description) VALUES (?, ?, ?)",
        )
        .bind(name)
        .bind(bandwidth_mbps)
        .bind(description)
        .execute(pool)
        .await?;
        Ok(res.last_insert_rowid())
    }

    pub async fn update_fields(
        pool: &SqlitePool,
        id: i64,
        name: Option<&str>,
        bandwidth_mbps: Option<i64>,
        description: Option<&str>,
    ) -> sqlx::Result<u64> {
        let res = sqlx::query(
            "UPDATE bandwidth_profiles SET \
                name = COALESCE(?, name), \
                bandwidth_mbps = COALESCE(?, bandwidth_mbps), \
                description = COALESCE(?, description), \
                updated_at = datetime('now') \
             WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(name)
        .bind(bandwidth_mbps)
        .bind(description)
        .bind(id)
        .execute(pool)
        .await?;
        Ok(res.rows_affected())
    }

    pub async fn soft_delete(pool: &SqlitePool, id: i64) -> sqlx::Result<u64> {
        let res = sqlx::query(
            "UPDATE bandwidth_profiles SET deleted_at = datetime('now'), updated_at = datetime('now') \
             WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(id)
        .execute(pool)
        .await?;
        Ok(res.rows_affected())
    }

    /// 活跃规则对该 profile 的引用数(删除保护用)。
    pub async fn active_rule_refs(pool: &SqlitePool, id: i64) -> sqlx::Result<i64> {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM forward_rules \
             WHERE bandwidth_profile_id = ? AND deleted_at IS NULL",
        )
        .bind(id)
        .fetch_one(pool)
        .await
    }
}
```

`models/mod.rs` 加 `pub mod bandwidth_profile;`。

- [ ] **Step 4: routes 实现**

```rust
// crates/panel-server/src/routes/bandwidth_profiles.rs
use crate::{
    audit,
    auth::extractor::{ActorIp, AuthUser},
    error::{ApiError, ApiResult},
    models::bandwidth_profile::BandwidthProfile,
    state::AppState,
};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Serialize)]
pub struct ProfileView {
    pub id: i64,
    pub name: String,
    pub bandwidth_mbps: i64,
    pub description: String,
    pub created_at: String,
    pub updated_at: String,
}

impl From<BandwidthProfile> for ProfileView {
    fn from(p: BandwidthProfile) -> Self {
        Self {
            id: p.id,
            name: p.name,
            bandwidth_mbps: p.bandwidth_mbps,
            description: p.description,
            created_at: p.created_at,
            updated_at: p.updated_at,
        }
    }
}

#[derive(Deserialize)]
pub struct ListQuery {
    pub page: Option<i64>,
    pub page_size: Option<i64>,
}

#[derive(Serialize)]
pub struct ProfileListResponse {
    pub items: Vec<ProfileView>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
}

#[derive(Deserialize)]
pub struct CreateProfileRequest {
    pub name: String,
    pub bandwidth_mbps: i64,
    #[serde(default)]
    pub description: String,
}

#[derive(Deserialize)]
pub struct UpdateProfileRequest {
    pub name: Option<String>,
    pub bandwidth_mbps: Option<i64>,
    pub description: Option<String>,
}

fn validate_mbps(n: i64) -> ApiResult<()> {
    if n > 0 {
        Ok(())
    } else {
        Err(ApiError::BadRequest("bandwidth_mbps must be > 0".into()))
    }
}

fn validate_name(n: &str) -> ApiResult<()> {
    if n.trim().is_empty() {
        Err(ApiError::BadRequest("name is required".into()))
    } else {
        Ok(())
    }
}

pub async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<ProfileListResponse>> {
    auth.require_admin()?;
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).clamp(1, 100);
    let offset = page.saturating_sub(1).saturating_mul(page_size);
    let items = BandwidthProfile::list_paged(&state.pool, page_size, offset).await?;
    let total = BandwidthProfile::count(&state.pool).await?;
    Ok(Json(ProfileListResponse {
        items: items.into_iter().map(Into::into).collect(),
        total,
        page,
        page_size,
    }))
}

pub async fn get(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<ProfileView>> {
    auth.require_admin()?;
    let p = BandwidthProfile::find_by_id(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(p.into()))
}

pub async fn create(
    State(state): State<AppState>,
    auth: AuthUser,
    actor_ip: ActorIp,
    Json(req): Json<CreateProfileRequest>,
) -> ApiResult<Json<ProfileView>> {
    auth.require_admin()?;
    let name = req.name.trim();
    validate_name(name)?;
    validate_mbps(req.bandwidth_mbps)?;
    let new_id = BandwidthProfile::create(&state.pool, name, req.bandwidth_mbps, req.description.trim())
        .await
        .map_err(map_sqlx_to_api)?;
    let p = BandwidthProfile::find_by_id(&state.pool, new_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    audit::record_with_ip(
        &state.pool,
        Some(auth.0.sub),
        actor_ip.as_option(),
        "bandwidth_profile.create",
        Some("bandwidth_profile"),
        Some(new_id),
        Some(name),
        true,
        None,
    )
    .await;
    Ok(Json(p.into()))
}

pub async fn update(
    State(state): State<AppState>,
    auth: AuthUser,
    actor_ip: ActorIp,
    Path(id): Path<i64>,
    Json(req): Json<UpdateProfileRequest>,
) -> ApiResult<Json<ProfileView>> {
    auth.require_admin()?;
    if let Some(n) = req.name.as_deref() {
        validate_name(n)?;
    }
    if let Some(m) = req.bandwidth_mbps {
        validate_mbps(m)?;
    }
    let rows = BandwidthProfile::update_fields(
        &state.pool,
        id,
        req.name.as_deref().map(str::trim),
        req.bandwidth_mbps,
        req.description.as_deref().map(str::trim),
    )
    .await
    .map_err(map_sqlx_to_api)?;
    if rows == 0 {
        return Err(ApiError::NotFound);
    }
    let p = BandwidthProfile::find_by_id(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    audit::record_with_ip(
        &state.pool,
        Some(auth.0.sub),
        actor_ip.as_option(),
        "bandwidth_profile.update",
        Some("bandwidth_profile"),
        Some(id),
        None,
        true,
        None,
    )
    .await;

    // 引用该 profile 的活跃规则即时下发新带宽(重建 Agent token bucket)。
    dispatch_referencing_rules(&state, id).await;

    Ok(Json(p.into()))
}

pub async fn delete(
    State(state): State<AppState>,
    auth: AuthUser,
    actor_ip: ActorIp,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    auth.require_admin()?;
    let refs = BandwidthProfile::active_rule_refs(&state.pool, id).await?;
    if refs > 0 {
        return Err(ApiError::BadRequest(format!(
            "bandwidth profile is referenced by {refs} active rule(s); detach them first"
        )));
    }
    let rows = BandwidthProfile::soft_delete(&state.pool, id).await?;
    if rows == 0 {
        return Err(ApiError::NotFound);
    }
    audit::record_with_ip(
        &state.pool,
        Some(auth.0.sub),
        actor_ip.as_option(),
        "bandwidth_profile.delete",
        Some("bandwidth_profile"),
        Some(id),
        None,
        true,
        None,
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

/// profile 改动后,把引用它的活跃规则逐条 ApplyRule 重下发。
/// Agent 离线时静默跳过(下次 register reconcile 对齐)。
async fn dispatch_referencing_rules(state: &AppState, profile_id: i64) {
    use crate::grpc::commands::apply_command;
    use crate::models::rule::Rule;
    let ids: Vec<(i64,)> = sqlx::query_as(
        "SELECT id FROM forward_rules WHERE bandwidth_profile_id = ? AND deleted_at IS NULL",
    )
    .bind(profile_id)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();
    for (rule_id,) in ids {
        if let Ok(Some(rule)) = Rule::find_by_id(&state.pool, rule_id).await {
            if !state.dispatcher.dispatch(rule.node_id, apply_command(&rule)) {
                tracing::warn!(node_id = rule.node_id, rule_id, "agent offline; bandwidth change syncs at next register");
            }
        }
    }
}

fn map_sqlx_to_api(e: sqlx::Error) -> ApiError {
    if let Some(db_err) = e.as_database_error() {
        if db_err.is_unique_violation() {
            return ApiError::BadRequest("profile name already exists".into());
        }
        if db_err.is_check_violation() {
            return ApiError::BadRequest("bandwidth_mbps must be > 0".into());
        }
    }
    ApiError::Database(e)
}
```

`routes/mod.rs`：`pub mod bandwidth_profiles;` + 在 `/api/users` 路由块后注册：

```rust
        .route(
            "/api/bandwidth-profiles",
            get(bandwidth_profiles::list).post(bandwidth_profiles::create),
        )
        .route(
            "/api/bandwidth-profiles/{id}",
            get(bandwidth_profiles::get)
                .patch(bandwidth_profiles::update)
                .delete(bandwidth_profiles::delete),
        )
```

- [ ] **Step 5: 跑测试验证通过**

Run: `cargo test -p panel-server --test api_bandwidth_profiles && cargo test --workspace`
Expected: 全 PASS。

- [ ] **Step 6: Commit**

```bash
git add crates/panel-server/src/models/bandwidth_profile.rs crates/panel-server/src/models/mod.rs crates/panel-server/src/routes/bandwidth_profiles.rs crates/panel-server/src/routes/mod.rs crates/panel-server/tests/api_bandwidth_profiles.rs
git commit -m "feat(server): bandwidth_profiles CRUD with rule-reference delete protection"
```

## Task 4: proto 重排 + 规则级限制全链路下线（migration 0004 / server / agent / tests）

> 这是「不可再分的原子大改」：DROP 三列后所有引用必须同步消失才能编译。内部步骤逐文件推进，最终一次性验证。

**Files:**
- Create: `migrations/0004_drop_rule_limits.sql`
- Modify: `crates/common/proto/control.proto`
- Modify: `crates/panel-server/src/models/rule.rs`、`routes/rules.rs`、`grpc/commands.rs`、`grpc/service.rs`、`src/main.rs`、`routes/system.rs`
- Modify: `crates/node-agent/src/store.rs`、`relay/tcp.rs`、`relay/udp.rs`
- Modify: `crates/panel-server/tests/api_rules.rs`、`tests/agent_e2e.rs`

- [ ] **Step 1: 写 migration 0004**

```sql
-- migrations/0004_drop_rule_limits.sql
-- Phase 2 减法:规则级限制三列下线(语义已迁移至 users.expires_at /
-- users.traffic_limit_bytes_30d / forward_rules.bandwidth_profile_id)。
-- SQLite 3.35+ 原生 DROP COLUMN(sqlx bundled sqlite 满足);PG 语法一致。
ALTER TABLE forward_rules DROP COLUMN expires_at;
ALTER TABLE forward_rules DROP COLUMN traffic_limit_bytes;
ALTER TABLE forward_rules DROP COLUMN bandwidth_limit_mbps;

-- 孤儿配置 key:其语义依附于已删除的规则级字段。
DELETE FROM system_settings
WHERE key IN ('default_traffic_limit_bytes', 'default_bandwidth_limit_mbps');

-- 用户级 sweeper(Task 5)按 expires_at 扫描;部分索引只覆盖设了到期的行。
-- (Task 1 review 建议项,在本 migration 顺手落地。)
CREATE INDEX idx_users_expires_at ON users (expires_at) WHERE expires_at IS NOT NULL;
```

- [ ] **Step 2: proto Rule 重排**

`crates/common/proto/control.proto` 的 `message Rule`：删除字段 8/9/10 及其注释，改为：

```proto
message Rule {
  int64 id = 1;
  string protocol = 2;          // tcp / udp / tcp_udp
  string listen_ip = 3;
  uint32 listen_port = 4;
  string target_host = 5;
  uint32 target_port = 6;
  bool enabled = 7;
  // P2 起规则级 traffic_limit_bytes(8) / bandwidth_limit_mbps(9) /
  // expires_at_unix(10) 下线:到期与流量配额归 users,带宽归 bandwidth_profiles。
  // 字段号不复用。
  reserved 8, 9, 10;
  reserved "traffic_limit_bytes", "bandwidth_limit_mbps", "expires_at_unix";
  // 关联 bandwidth_profile 的限速值。0 = 无限速。
  // server dispatch 时由 forward_rules.bandwidth_profile_id 联查得出。
  int64 bandwidth_mbps = 11;
}
```

- [ ] **Step 3: models/rule.rs 删三字段 + 签名收敛**

struct 删 `expires_at` / `traffic_limit_bytes` / `bandwidth_limit_mbps` 三个字段（保留 Task 1 加的 `bandwidth_profile_id` / `bandwidth_mbps`）。

`RULE_COLUMNS` 改为：

```rust
const RULE_COLUMNS: &str = "id, user_id, node_id, name, protocol, listen_ip, listen_port, \
    target_host, target_port, enabled, rx_bytes, tx_bytes, connection_count, \
    bandwidth_profile_id, \
    (SELECT bp.bandwidth_mbps FROM bandwidth_profiles bp \
        WHERE bp.id = forward_rules.bandwidth_profile_id AND bp.deleted_at IS NULL) AS bandwidth_mbps, \
    created_at, updated_at";
```

`create` 签名：删 `expires_at: Option<&str>, traffic_limit_bytes: Option<i64>, bandwidth_limit_mbps: Option<i64>` 三参数，加 `bandwidth_profile_id: Option<i64>`；INSERT 改为：

```rust
        let res = sqlx::query(
            "INSERT INTO forward_rules \
                (user_id, node_id, name, protocol, listen_ip, listen_port, \
                 target_host, target_port, bandwidth_profile_id) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(user_id)
        .bind(node_id)
        .bind(name)
        .bind(protocol)
        .bind(listen_ip)
        .bind(listen_port)
        .bind(target_host)
        .bind(target_port)
        .bind(bandwidth_profile_id)
        .execute(pool)
        .await?;
```

`update_fields` 签名：删三参数，加 `bandwidth_profile_id: Option<i64>`（0 = 解除关联）；UPDATE 语句删三行 COALESCE，加（全语句改用 `?1`-`?8` 显式编号，`bandwidth_profile_id`=?6, `id`=?7——按最终参数顺序编号，确保 CASE 复用同一参数）：

```rust
        let res = sqlx::query(
            "UPDATE forward_rules SET \
                name = COALESCE(?1, name), \
                listen_ip = COALESCE(?2, listen_ip), \
                listen_port = COALESCE(?3, listen_port), \
                target_host = COALESCE(?4, target_host), \
                target_port = COALESCE(?5, target_port), \
                bandwidth_profile_id = CASE \
                    WHEN ?6 IS NULL THEN bandwidth_profile_id \
                    WHEN ?6 = 0 THEN NULL \
                    ELSE ?6 END, \
                updated_at = datetime('now') \
             WHERE id = ?7 AND deleted_at IS NULL",
        )
        .bind(name)
        .bind(listen_ip)
        .bind(listen_port)
        .bind(target_host)
        .bind(target_port)
        .bind(bandwidth_profile_id)
        .bind(id)
        .execute(pool)
        .await?;
```

- [ ] **Step 4: routes/rules.rs DTO + 校验同步**

- `RuleView`：删三字段；加 `pub bandwidth_profile_id: Option<i64>, pub bandwidth_mbps: Option<i64>`；`From<Rule>` 同步。
- `CreateRuleRequest`：删三字段；加 `pub bandwidth_profile_id: Option<i64>`。
- `UpdateRuleRequest`：删三字段；加 `pub bandwidth_profile_id: Option<i64>`（注释 `/// 0 = 解除关联`）。
- `create` handler：删除 `expires_at` 空串校验块；在 node 校验后加 profile 存在性校验：

```rust
    if let Some(pid) = req.bandwidth_profile_id {
        if pid <= 0 {
            return Err(ApiError::BadRequest("bandwidth_profile_id must be > 0".into()));
        }
        crate::models::bandwidth_profile::BandwidthProfile::find_by_id(&state.pool, pid)
            .await?
            .ok_or_else(|| ApiError::BadRequest("bandwidth_profile_id does not exist".into()))?;
    }
```

`Rule::create(...)` 调用尾参数改为 `req.bandwidth_profile_id`。
- `update` handler：同样加 profile 校验（`Some(pid) if pid > 0` 时查存在；`Some(0)` 直通=解除）：

```rust
    if let Some(pid) = req.bandwidth_profile_id {
        if pid < 0 {
            return Err(ApiError::BadRequest("bandwidth_profile_id must be >= 0".into()));
        }
        if pid > 0 {
            crate::models::bandwidth_profile::BandwidthProfile::find_by_id(&state.pool, pid)
                .await?
                .ok_or_else(|| ApiError::BadRequest("bandwidth_profile_id does not exist".into()))?;
        }
    }
```

`Rule::update_fields(...)` 调用删三实参、传 `req.bandwidth_profile_id`。

- [ ] **Step 5: grpc/commands.rs + service.rs + main.rs**

`commands.rs::rule_to_proto` 删三行赋值，加：

```rust
        bandwidth_mbps: rule.bandwidth_mbps.unwrap_or(0),
```

`parse_sqlite_datetime` 保留（login / 后续 sweeper 复用），doc 注释里删去对 auto_stop sweeper 的引用、改为「login 到期检查与 user_quota sweeper 复用」。

`grpc/service.rs`：
- 删除整个 `auto_stop_if_exceeded` 函数与 `spawn_expiry_sweeper` 函数（约 445-552 行区间）。
- `report_rule_stats` 内删除调用块（420-423 行的 `if let Err(e) = auto_stop_if_exceeded(...)`）及其注释。
- 顶部 import 若 `DbRule` / `apply_command` / `parse_sqlite_datetime` / `Utc` 因此不再使用则一并移除（编译器 warning 指引）。

`main.rs`：删 `grpc::service::spawn_expiry_sweeper(state.clone());` 与其注释（Task 5 会在同位置换 user_quota sweeper；本 Task 先删，保持编译绿）。

`routes/system.rs`：`ALLOWED` 数组删 `"default_traffic_limit_bytes", "default_bandwidth_limit_mbps"` 两行；`validate_setting` 删除对应 match 分支（302 行 `"default_traffic_limit_bytes" | "default_bandwidth_limit_mbps" => {...}` 整块）。

- [ ] **Step 6: node-agent 同步**

`store.rs::RuleJson`：删 `traffic_limit_bytes` / `bandwidth_limit_mbps` / `expires_at_unix` 三字段；加：

```rust
    /// P2 新增。`#[serde(default)]` 兼容旧版 agent-state.json(缺字段 → 0 = 不限速)。
    #[serde(default)]
    bandwidth_mbps: i64,
```

两个 `From` impl 同步（`bandwidth_mbps: r.bandwidth_mbps`）。serde 默认忽略 unknown fields，旧状态文件中的三个废弃字段无害。

`relay/tcp.rs` 与 `relay/udp.rs` 测试模块的 `rule_for()`：删三行字段，加 `bandwidth_mbps: 0,`。

`control.rs:97` 的 `expires_at = inner.expires_at_unix` 是 **RegisterResponse 的 session 过期时间，与规则无关，不要动**。

- [ ] **Step 7: panel-server 测试同步**

`tests/api_rules.rs`：
- 约 203 行 create 请求 JSON 中的 `"traffic_limit_bytes": 100` 一行删除；该用例若断言这三个字段的返回值则改为断言 `bandwidth_profile_id: null`。
- `auto_stop_when_expires_at_past` 测试（约 232-260 行）整体删除——规则级到期语义已退役，用户级覆盖在 Task 5 测试。
- 其余出现 `"expires_at"` / `"traffic_limit_bytes"` / `"bandwidth_limit_mbps"` 的请求体/断言逐处删除（grep 确认清零）。

`tests/agent_e2e.rs`：约 186 行「防回归: 无 traffic_limit 时 report_rule_stats 不应触发 auto_stop_if_exceeded」相关注释与断言段——auto_stop 已不存在，把该段改为仅断言 stats 落库后 `enabled` 保持 1（语义不变、引用消失）。文件中其它 proto `Rule {}` 构造若含三字段则删除并补 `bandwidth_mbps: 0`。

执行后用 `grep -rn "traffic_limit_bytes\|bandwidth_limit_mbps" crates/ migrations/` 验证残留仅有三类合法历史：`migrations/0001_init.sql`（历史 schema，**不可改**——测试库按 0001→0004 顺序重放，改 0001 会让 0004 的 DROP 失败）、`migrations/0004_drop_rule_limits.sql` 自身、proto 的 `reserved` 注释。

- [ ] **Step 8: 全量验证**

Run: `cargo build --workspace 2>&1 | tail -5 && cargo test --workspace`
Expected: 编译零 warning（除既有），测试全 PASS。

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "feat!: retire rule-level expires/traffic/bandwidth across DB+proto+server+agent; wire bandwidth_mbps from profiles"
```

---

## Task 5: user_quota sweeper（到期 60s / 配额 300s 双 tick）

**Files:**
- Create: `crates/panel-server/src/sweeper/mod.rs`、`crates/panel-server/src/sweeper/user_quota.rs`
- Modify: `crates/panel-server/src/lib.rs`、`crates/panel-server/src/main.rs`
- Test: `crates/panel-server/tests/user_quota_sweeper.rs`

- [ ] **Step 1: 写失败测试**

```rust
// crates/panel-server/tests/user_quota_sweeper.rs
//! 直接调 tick 函数测试(确定性),不依赖真实 interval。
mod common;

use panel_server::sweeper::user_quota::{expiry_tick_once, quota_tick_once};

/// 建 node + 指定 user 名下两条 enabled 规则,返回 rule ids。
async fn seed_rules(app: &common::TestApp, user_id: i64) -> (i64, i64) {
    sqlx::query("INSERT INTO nodes (name, agent_token_hash) VALUES ('swn', 'x')")
        .execute(&app.state.pool)
        .await
        .unwrap();
    let mut ids = Vec::new();
    for (i, port) in [(1, 21001_i64), (2, 21002)] {
        let res = sqlx::query(
            "INSERT INTO forward_rules (user_id, node_id, name, protocol, listen_ip, listen_port, target_host, target_port) \
             VALUES (?, 1, ?, 'tcp', '0.0.0.0', ?, '1.2.3.4', 443)",
        )
        .bind(user_id)
        .bind(format!("swr{i}"))
        .bind(port)
        .execute(&app.state.pool)
        .await
        .unwrap();
        ids.push(res.last_insert_rowid());
    }
    (ids[0], ids[1])
}

async fn enabled_of(app: &common::TestApp, rule_id: i64) -> i64 {
    sqlx::query_scalar("SELECT enabled FROM forward_rules WHERE id = ?")
        .bind(rule_id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn expiry_tick_disables_rules_of_expired_user() {
    let app = common::make_app().await.unwrap();
    let (uid, _) = common::make_user_token(&app, "expuser", "password123").await.unwrap();
    sqlx::query("UPDATE users SET expires_at = '2020-01-01 00:00:00' WHERE id = ?")
        .bind(uid)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let (r1, r2) = seed_rules(&app, uid).await;

    let hit = expiry_tick_once(&app.state).await.unwrap();
    assert_eq!(hit, 1, "应命中 1 个过期用户");
    assert_eq!(enabled_of(&app, r1).await, 0);
    assert_eq!(enabled_of(&app, r2).await, 0);

    // audit 聚合一条
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_logs WHERE action = 'user.expired_auto_disable_rules' AND target_id = ?",
    )
    .bind(uid)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(n, 1);

    // 幂等:再跑一次不重复触发(规则已 disabled,无 enabled=1 命中)
    let hit2 = expiry_tick_once(&app.state).await.unwrap();
    assert_eq!(hit2, 0);
}

#[tokio::test]
async fn quota_tick_refreshes_cache_then_disables_over_limit() {
    let app = common::make_app().await.unwrap();
    let (uid, _) = common::make_user_token(&app, "quotau", "password123").await.unwrap();
    sqlx::query("UPDATE users SET traffic_limit_bytes_30d = 100 WHERE id = ?")
        .bind(uid)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let (r1, _r2) = seed_rules(&app, uid).await;
    // 30 天窗口内的 rule_stats:rx+tx = 150 > 100
    sqlx::query(
        "INSERT INTO rule_stats (rule_id, bucket_at, rx_bytes, tx_bytes) \
         VALUES (?, datetime('now'), 100, 50)",
    )
    .bind(r1)
    .execute(&app.state.pool)
    .await
    .unwrap();

    let hit = quota_tick_once(&app.state).await.unwrap();
    assert_eq!(hit, 1, "应命中 1 个超额用户");

    let (cached, calc_at): (i64, Option<String>) = sqlx::query_as(
        "SELECT period_used_bytes_cached, period_used_calculated_at FROM users WHERE id = ?",
    )
    .bind(uid)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(cached, 150, "cache 必须先刷新再判定");
    assert!(calc_at.is_some());
    assert_eq!(enabled_of(&app, r1).await, 0);

    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_logs WHERE action = 'user.quota_exceeded_auto_disable_rules' AND target_id = ?",
    )
    .bind(uid)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn quota_tick_ignores_unlimited_and_under_limit_users() {
    let app = common::make_app().await.unwrap();
    // admin 无 limit;再建一个 limit 充足的用户
    let (uid, _) = common::make_user_token(&app, "underu", "password123").await.unwrap();
    sqlx::query("UPDATE users SET traffic_limit_bytes_30d = 1000000 WHERE id = ?")
        .bind(uid)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let (r1, _) = seed_rules(&app, uid).await;
    sqlx::query(
        "INSERT INTO rule_stats (rule_id, bucket_at, rx_bytes, tx_bytes) VALUES (?, datetime('now'), 10, 10)",
    )
    .bind(r1)
    .execute(&app.state.pool)
    .await
    .unwrap();

    let hit = quota_tick_once(&app.state).await.unwrap();
    assert_eq!(hit, 0);
    assert_eq!(enabled_of(&app, r1).await, 1);
}
```

- [ ] **Step 2: 跑测试验证失败**

Run: `cargo test -p panel-server --test user_quota_sweeper`
Expected: FAIL（`panel_server::sweeper` 模块不存在）。

- [ ] **Step 3: 实现 sweeper**

`lib.rs` 加 `pub mod sweeper;`。`sweeper/mod.rs`：

```rust
pub mod user_quota;
```

```rust
// crates/panel-server/src/sweeper/user_quota.rs
//! 用户级到期 / 滚动 30 天流量配额 sweeper(P2)。
//! 取代已退役的规则级 expiry sweeper:一个 tokio task 内两个独立 interval,
//! expiry 默认 60s(PANEL_USER_EXPIRY_SWEEP_SECS),quota 默认 300s(PANEL_USER_QUOTA_SWEEP_SECS)。
use crate::{audit, grpc::commands::apply_command, models::rule::Rule, state::AppState};
use std::time::Duration;
use tracing::{info, warn};

fn env_secs(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
        .max(5)
}

pub fn spawn_user_quota_sweeper(state: AppState) {
    let expiry_secs = env_secs("PANEL_USER_EXPIRY_SWEEP_SECS", 60);
    let quota_secs = env_secs("PANEL_USER_QUOTA_SWEEP_SECS", 300);
    tokio::spawn(async move {
        let mut expiry_tick = tokio::time::interval(Duration::from_secs(expiry_secs));
        let mut quota_tick = tokio::time::interval(Duration::from_secs(quota_secs));
        expiry_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        quota_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = expiry_tick.tick() => {
                    if let Err(e) = expiry_tick_once(&state).await {
                        warn!(error = ?e, "user expiry sweep failed");
                    }
                }
                _ = quota_tick.tick() => {
                    if let Err(e) = quota_tick_once(&state).await {
                        warn!(error = ?e, "user quota sweep failed");
                    }
                }
            }
        }
    });
}

/// 扫已过期用户,停其名下 enabled 规则。返回命中的用户数。
/// pub 供 integration tests 直接调用(确定性,不等 interval)。
pub async fn expiry_tick_once(state: &AppState) -> anyhow::Result<u64> {
    let users: Vec<(i64,)> = sqlx::query_as(
        "SELECT u.id FROM users u \
         WHERE u.expires_at IS NOT NULL AND u.expires_at <= datetime('now') \
           AND u.deleted_at IS NULL \
           AND EXISTS (SELECT 1 FROM forward_rules fr \
                       WHERE fr.user_id = u.id AND fr.enabled = 1 AND fr.deleted_at IS NULL)",
    )
    .fetch_all(&state.pool)
    .await?;
    let mut hit = 0u64;
    for (user_id,) in users {
        let disabled = disable_rules_for_user(state, user_id, "expired").await?;
        if disabled > 0 {
            audit::record(
                &state.pool,
                None,
                "user.expired_auto_disable_rules",
                Some("user"),
                Some(user_id),
                Some(&format!("user_id={user_id},disabled_rule_count={disabled},reason=expired")),
                true,
                None,
            )
            .await;
            info!(user_id, disabled, "expired user rules auto-disabled");
            hit += 1;
        }
    }
    Ok(hit)
}

/// 先刷新所有活跃用户的 30 天用量 cache,再停超额用户的规则。返回命中的用户数。
/// 不变量:必须先刷 cache 再判定,禁止用过期 cache 判断超额。
pub async fn quota_tick_once(state: &AppState) -> anyhow::Result<u64> {
    sqlx::query(
        "UPDATE users SET \
             period_used_bytes_cached = ( \
                 SELECT COALESCE(SUM(rs.rx_bytes + rs.tx_bytes), 0) \
                 FROM rule_stats rs \
                 JOIN forward_rules fr ON rs.rule_id = fr.id \
                 WHERE fr.user_id = users.id \
                   AND rs.bucket_at >= datetime('now', '-30 days') \
             ), \
             period_used_calculated_at = datetime('now') \
         WHERE deleted_at IS NULL",
    )
    .execute(&state.pool)
    .await?;

    let users: Vec<(i64,)> = sqlx::query_as(
        "SELECT u.id FROM users u \
         WHERE u.traffic_limit_bytes_30d IS NOT NULL \
           AND u.period_used_bytes_cached > u.traffic_limit_bytes_30d \
           AND u.deleted_at IS NULL \
           AND EXISTS (SELECT 1 FROM forward_rules fr \
                       WHERE fr.user_id = u.id AND fr.enabled = 1 AND fr.deleted_at IS NULL)",
    )
    .fetch_all(&state.pool)
    .await?;
    let mut hit = 0u64;
    for (user_id,) in users {
        let disabled = disable_rules_for_user(state, user_id, "quota_exceeded").await?;
        if disabled > 0 {
            audit::record(
                &state.pool,
                None,
                "user.quota_exceeded_auto_disable_rules",
                Some("user"),
                Some(user_id),
                Some(&format!(
                    "user_id={user_id},disabled_rule_count={disabled},reason=quota_exceeded"
                )),
                true,
                None,
            )
            .await;
            info!(user_id, disabled, "over-quota user rules auto-disabled");
            hit += 1;
        }
    }
    Ok(hit)
}

/// 原子停用某用户全部 enabled 规则并逐条 dispatch ApplyRule(enabled=false)。
/// 返回实际停掉的行数。Agent 离线时静默(下次 register reconcile 对齐)。
async fn disable_rules_for_user(state: &AppState, user_id: i64, reason: &str) -> anyhow::Result<u64> {
    let ids: Vec<(i64,)> = sqlx::query_as(
        "SELECT id FROM forward_rules WHERE user_id = ? AND enabled = 1 AND deleted_at IS NULL",
    )
    .bind(user_id)
    .fetch_all(&state.pool)
    .await?;
    if ids.is_empty() {
        return Ok(0);
    }
    // 单次 UPDATE 原子落库;WHERE enabled = 1 防并发重复触发。
    let rows = sqlx::query(
        "UPDATE forward_rules SET enabled = 0, updated_at = datetime('now') \
         WHERE user_id = ? AND enabled = 1 AND deleted_at IS NULL",
    )
    .bind(user_id)
    .execute(&state.pool)
    .await?
    .rows_affected();
    for (rule_id,) in ids {
        if let Ok(Some(rule)) = Rule::find_by_id(&state.pool, rule_id).await {
            if !state.dispatcher.dispatch(rule.node_id, apply_command(&rule)) {
                warn!(node_id = rule.node_id, rule_id, reason, "agent offline; disable syncs at next register");
            }
        }
    }
    Ok(rows)
}
```

`main.rs` 在原 spawn_expiry_sweeper 位置加：

```rust
    // 用户级到期(60s)与 30 天配额(300s)双 tick sweeper;随 tokio runtime 一起 drop。
    panel_server::sweeper::user_quota::spawn_user_quota_sweeper(state.clone());
```

（main.rs 已 `use panel_server::...`,可直接在 use 列表补 `sweeper` 或全路径调用，取编译最顺者。）

- [ ] **Step 4: 跑测试验证通过**

Run: `cargo test -p panel-server --test user_quota_sweeper && cargo test --workspace`
Expected: 全 PASS。

- [ ] **Step 5: Commit**

```bash
git add crates/panel-server/src/sweeper crates/panel-server/src/lib.rs crates/panel-server/src/main.rs crates/panel-server/tests/user_quota_sweeper.rs
git commit -m "feat(server): user-level expiry/quota sweeper with aggregated audit"
```

## Task 6: 端口自动分配（listen_port 可空）

**Files:**
- Modify: `crates/panel-server/src/routes/rules.rs`
- Test: `crates/panel-server/tests/api_rules_port_alloc.rs`

- [ ] **Step 1: 写失败测试**

```rust
// crates/panel-server/tests/api_rules_port_alloc.rs
mod common;

use axum::http::{Method, StatusCode};
use serde_json::{json, Value};

/// 建一个 port_pool [25000, 25005] 的节点,返回 node_id。
async fn seed_node(app: &common::TestApp) -> i64 {
    let res = sqlx::query(
        "INSERT INTO nodes (name, agent_token_hash, port_pool_min, port_pool_max) \
         VALUES ('palloc', 'x', 25000, 25005)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    res.last_insert_rowid()
}

async fn create_rule(app: &common::TestApp, body: Value) -> (StatusCode, Value) {
    let req = common::auth_req(Method::POST, "/api/rules", &app.admin_token, Some(body)).unwrap();
    common::send(app.app.clone(), req).await.unwrap()
}

#[tokio::test]
async fn auto_alloc_picks_smallest_free_port() {
    let app = common::make_app().await.unwrap();
    let node_id = seed_node(&app).await;
    let (status, body) = create_rule(
        &app,
        json!({ "node_id": node_id, "name": "a1", "protocol": "tcp", "target_host": "1.2.3.4", "target_port": 443 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["listen_port"], 25000);

    // 第二条跳过已占用
    let (status, body) = create_rule(
        &app,
        json!({ "node_id": node_id, "name": "a2", "protocol": "tcp", "target_host": "1.2.3.4", "target_port": 443 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["listen_port"], 25001);
}

#[tokio::test]
async fn auto_alloc_skips_reserved_ports() {
    let app = common::make_app().await.unwrap();
    let node_id = seed_node(&app).await;
    sqlx::query("UPDATE system_settings SET value = '[25000, 25001]' WHERE key = 'reserved_ports'")
        .execute(&app.state.pool)
        .await
        .unwrap();
    let (status, body) = create_rule(
        &app,
        json!({ "node_id": node_id, "name": "r1", "protocol": "tcp", "target_host": "1.2.3.4", "target_port": 443 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["listen_port"], 25002);
}

#[tokio::test]
async fn auto_alloc_respects_protocol_mutex() {
    let app = common::make_app().await.unwrap();
    let node_id = seed_node(&app).await;
    // 占 25000:tcp_udp(与 tcp 和 udp 都互斥)
    let (status, _) = create_rule(
        &app,
        json!({ "node_id": node_id, "name": "m0", "protocol": "tcp_udp", "listen_port": 25000, "target_host": "1.2.3.4", "target_port": 443 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // 占 25001:udp
    let (status, _) = create_rule(
        &app,
        json!({ "node_id": node_id, "name": "m1", "protocol": "udp", "listen_port": 25001, "target_host": "1.2.3.4", "target_port": 443 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // 自动分配 tcp:25000 被 tcp_udp 互斥;25001 的 udp 与 tcp 不互斥 → 拿 25001
    let (status, body) = create_rule(
        &app,
        json!({ "node_id": node_id, "name": "m2", "protocol": "tcp", "target_host": "1.2.3.4", "target_port": 443 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["listen_port"], 25001);

    // 自动分配 tcp_udp:25000(tcp_udp)/25001(udp+tcp) 都冲突 → 25002
    let (status, body) = create_rule(
        &app,
        json!({ "node_id": node_id, "name": "m3", "protocol": "tcp_udp", "target_host": "1.2.3.4", "target_port": 443 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["listen_port"], 25002);
}

#[tokio::test]
async fn auto_alloc_pool_exhausted_returns_400() {
    let app = common::make_app().await.unwrap();
    let node_id = seed_node(&app).await;
    for p in 25000..=25005_i64 {
        let (status, _) = create_rule(
            &app,
            json!({ "node_id": node_id, "name": format!("f{p}"), "protocol": "tcp_udp", "listen_port": p, "target_host": "1.2.3.4", "target_port": 443 }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    let (status, body) = create_rule(
        &app,
        json!({ "node_id": node_id, "name": "overflow", "protocol": "tcp", "target_host": "1.2.3.4", "target_port": 443 }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["message"].as_str().unwrap().contains("port pool exhausted"), "{body}");
}

#[tokio::test]
async fn explicit_listen_port_still_validated() {
    let app = common::make_app().await.unwrap();
    let node_id = seed_node(&app).await;
    // 显式端口走旧校验路径:池外 → 400
    let (status, _) = create_rule(
        &app,
        json!({ "node_id": node_id, "name": "e1", "protocol": "tcp", "listen_port": 30000, "target_host": "1.2.3.4", "target_port": 443 }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
```

- [ ] **Step 2: 跑测试验证失败**

Run: `cargo test -p panel-server --test api_rules_port_alloc`
Expected: 自动分配相关用例 FAIL（缺 listen_port 被 serde 拒收 422 / 400）。

- [ ] **Step 3: 实现**

`routes/rules.rs`：
- `CreateRuleRequest.listen_port` 改为 `pub listen_port: Option<u16>,`。
- `create` handler 中原「`req.listen_port == 0 || req.target_port == 0`」改为：

```rust
    if matches!(req.listen_port, Some(0)) || req.target_port == 0 {
        return Err(ApiError::BadRequest(
            "listen_port and target_port must be 1-65535".into(),
        ));
    }
```

- node 查询保持在前；端口确定逻辑替换原 port_pool/reserved 两段为：

```rust
    let reserved = settings::reserved_ports(&state.pool).await;
    let listen_port_i64 = match req.listen_port {
        Some(p) => {
            let p = i64::from(p);
            if p < node.port_pool_min || p > node.port_pool_max {
                return Err(ApiError::BadRequest(format!(
                    "listen_port {} outside node's port pool [{}-{}]",
                    p, node.port_pool_min, node.port_pool_max
                )));
            }
            if reserved.contains(&p) {
                return Err(ApiError::BadRequest(format!("listen_port {p} is reserved")));
            }
            p
        }
        // 留空 → 池内最小可用端口(排除 reserved 与按协议互斥的占用)。
        None => allocate_port(&state.pool, &node, &req.listen_ip, &req.protocol, &reserved).await?,
    };
```

- 文件底部加分配函数（与 `ensure_no_protocol_conflict` 同区域）：

```rust
/// 自动分配:node 池内最小可用 listen_port。
/// 占用集合 = 同 node + 同 listen_ip 的活跃规则,按协议互斥语义判定:
/// tcp ↔ {tcp, tcp_udp} / udp ↔ {udp, tcp_udp} / tcp_udp ↔ 全部。
/// 并发窗口与 ensure_no_protocol_conflict 相同:精确重复由 DB UNIQUE 兜底,
/// 互斥型并发与既有 create 行为一致(MVP 已接受)。
async fn allocate_port(
    pool: &sqlx::SqlitePool,
    node: &Node,
    listen_ip: &str,
    protocol: &str,
    reserved: &[i64],
) -> ApiResult<i64> {
    let taken: Vec<(i64, String)> = sqlx::query_as(
        "SELECT listen_port, protocol FROM forward_rules \
         WHERE node_id = ? AND listen_ip = ? AND deleted_at IS NULL \
           AND listen_port BETWEEN ? AND ?",
    )
    .bind(node.id)
    .bind(listen_ip)
    .bind(node.port_pool_min)
    .bind(node.port_pool_max)
    .fetch_all(pool)
    .await?;

    let conflicts = |existing: &str| -> bool {
        match protocol {
            "tcp" => matches!(existing, "tcp" | "tcp_udp"),
            "udp" => matches!(existing, "udp" | "tcp_udp"),
            _ => true, // tcp_udp 与所有协议互斥
        }
    };
    let blocked: std::collections::HashSet<i64> = taken
        .iter()
        .filter(|(_, proto)| conflicts(proto))
        .map(|(port, _)| *port)
        .collect();

    for port in node.port_pool_min..=node.port_pool_max {
        if !reserved.contains(&port) && !blocked.contains(&port) {
            return Ok(port);
        }
    }
    Err(ApiError::BadRequest(format!(
        "port pool exhausted on node {} [{}-{}]",
        node.id, node.port_pool_min, node.port_pool_max
    )))
}
```

后续 `ensure_no_protocol_conflict(... listen_port_i64 ...)` 与 `Rule::create(... listen_port_i64 ...)` 沿用变量，无需变。

- [ ] **Step 4: 跑测试验证通过**

Run: `cargo test -p panel-server --test api_rules_port_alloc && cargo test --workspace`
Expected: 全 PASS（api_rules.rs 既有显式端口用例不受影响）。

- [ ] **Step 5: Commit**

```bash
git add crates/panel-server/src/routes/rules.rs crates/panel-server/tests/api_rules_port_alloc.rs
git commit -m "feat(server): auto-allocate listen_port from node pool when omitted"
```

---

## Task 7: Agent token bucket + TCP/UDP relay 限速接入

**Files:**
- Create: `crates/node-agent/src/limit/mod.rs`、`crates/node-agent/src/limit/token_bucket.rs`
- Modify: `crates/node-agent/src/main.rs`（`mod limit;`）、`manager.rs`、`relay/tcp.rs`、`relay/udp.rs`
- Delete: `crates/node-agent/src/relay/traits.rs`（QuotaGuard 占位被取代；`relay/mod.rs` 去掉 `pub mod traits;`）

- [ ] **Step 1: 写失败测试（token bucket 单元）**

```rust
// crates/node-agent/src/limit/token_bucket.rs 文件底部 tests(实现见 Step 3,先建文件骨架让测试可编译失败)
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn from_mbps_zero_means_unlimited() {
        assert!(TokenBucket::from_mbps(0).is_none());
        assert!(TokenBucket::from_mbps(-1).is_none());
        assert!(TokenBucket::from_mbps(10).is_some());
    }

    /// paused clock:8 Mbps = 1_000_000 B/s,burst = max(rate/5, 65536) = 200_000。
    /// 初始满桶,先吃掉 burst,再要 500_000 字节必须推进 ≈0.5s 虚拟时间。
    #[tokio::test(start_paused = true)]
    async fn acquire_waits_for_refill() {
        let b = TokenBucket::from_mbps(8).unwrap();
        b.acquire(200_000).await; // 清空 burst,不等待
        let start = tokio::time::Instant::now();
        b.acquire(500_000).await;
        let waited = start.elapsed();
        assert!(
            waited >= Duration::from_millis(450) && waited <= Duration::from_millis(650),
            "expected ~0.5s virtual wait, got {waited:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn try_acquire_fails_without_blocking_then_recovers() {
        let b = TokenBucket::from_mbps(8).unwrap();
        assert!(b.try_acquire(200_000), "满桶应放行");
        assert!(!b.try_acquire(100_000), "桶空应立即拒绝");
        tokio::time::advance(Duration::from_millis(200)).await; // 回填 200_000
        assert!(b.try_acquire(100_000));
    }

    /// 多任务共享同一桶:总放行速率受桶约束(虚拟时间)。
    #[tokio::test(start_paused = true)]
    async fn shared_bucket_serializes_concurrent_acquire() {
        let b: Arc<TokenBucket> = TokenBucket::from_mbps(8).unwrap();
        b.acquire(200_000).await; // 清空
        let start = tokio::time::Instant::now();
        let (b1, b2) = (b.clone(), b.clone());
        let t1 = tokio::spawn(async move { b1.acquire(250_000).await });
        let t2 = tokio::spawn(async move { b2.acquire(250_000).await });
        let _ = tokio::join!(t1, t2);
        let waited = start.elapsed();
        assert!(
            waited >= Duration::from_millis(400),
            "两个 250KB @1MB/s 应共等 ≥0.4s, got {waited:?}"
        );
    }
}
```

- [ ] **Step 2: 跑测试验证失败**

Run: `cargo test -p node-agent`
Expected: 编译 FAIL（TokenBucket 未定义）。

- [ ] **Step 3: 实现 TokenBucket**

`crates/node-agent/src/limit/mod.rs`：

```rust
pub mod token_bucket;
pub use token_bucket::TokenBucket;
```

```rust
// crates/node-agent/src/limit/token_bucket.rs
//! per-rule token bucket(P2 限速)。rx+tx 共用一桶;tcp_udp 协议的 TCP/UDP 任务共享同一实例。
//! rate = bandwidth_mbps * 125_000 B/s;burst = max(rate/5, 65536)
//! (≈200ms 容量,下限 64KB 保证 UDP 最大单包可放行)。
//! 用 tokio::time::Instant,测试可用 start_paused 虚拟时钟。
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::Instant;

pub struct TokenBucket {
    rate_bytes_per_sec: f64,
    burst_bytes: f64,
    state: Mutex<BucketState>,
}

struct BucketState {
    tokens: f64,
    last_refill: Instant,
}

impl TokenBucket {
    /// mbps <= 0 → None(不限速)。
    pub fn from_mbps(mbps: i64) -> Option<Arc<Self>> {
        if mbps <= 0 {
            return None;
        }
        let rate = mbps as f64 * 125_000.0;
        let burst = (rate / 5.0).max(65536.0);
        Some(Arc::new(Self {
            rate_bytes_per_sec: rate,
            burst_bytes: burst,
            state: Mutex::new(BucketState {
                tokens: burst,
                last_refill: Instant::now(),
            }),
        }))
    }

    fn refill(&self, st: &mut BucketState) {
        let now = Instant::now();
        let dt = now.duration_since(st.last_refill).as_secs_f64();
        st.tokens = (st.tokens + dt * self.rate_bytes_per_sec).min(self.burst_bytes);
        st.last_refill = now;
    }

    /// TCP 路径:阻塞等待直到拿到 want 字节配额。
    /// want > burst 时按 burst 计(防御;TCP chunk 8KB 远小于 burst 下限 64KB)。
    pub async fn acquire(&self, want: usize) {
        let want = (want as f64).min(self.burst_bytes);
        loop {
            let wait = {
                let mut st = self.state.lock().expect("token bucket poisoned");
                self.refill(&mut st);
                if st.tokens >= want {
                    st.tokens -= want;
                    return;
                }
                // 锁外 sleep,等缺口补齐
                Duration::from_secs_f64((want - st.tokens) / self.rate_bytes_per_sec)
            };
            tokio::time::sleep(wait).await;
        }
    }

    /// UDP 路径:不阻塞;配额不足返回 false(调用方丢包计 error)。
    pub fn try_acquire(&self, want: usize) -> bool {
        let want = want as f64;
        let mut st = self.state.lock().expect("token bucket poisoned");
        self.refill(&mut st);
        if st.tokens >= want {
            st.tokens -= want;
            true
        } else {
            false
        }
    }
}
```

`main.rs` 模块声明区加 `mod limit;`（按字母序放 `manager` 前）。

Run: `cargo test -p node-agent limit` → 4 个单元测试 PASS。

- [ ] **Step 4: TCP relay 接入（写失败测试 → 实现）**

`relay/tcp.rs` tests 模块追加（rule_for 已在 Task 4 改为含 `bandwidth_mbps`，此处显式覆盖）：

```rust
    /// 限速生效:2 MB @ 40 Mbps(5 MB/s, burst 1 MB)理论 ≥(2MB-1MB)/5MB/s = 0.2s。
    /// 只断言下限(慢 CI 不误报);同时校验数据完整性。
    #[tokio::test]
    async fn tcp_relay_throttles_when_bucket_set() {
        use crate::limit::TokenBucket;
        let echo_port = spawn_echo_server().await;
        let listen_port = ephemeral_port();
        let stats = Arc::new(StatsCollector::new());
        let mut rule = rule_for(listen_port, echo_port);
        rule.bandwidth_mbps = 40;
        let bucket = TokenBucket::from_mbps(rule.bandwidth_mbps);
        let handle = start(rule, stats.clone(), bucket).await.expect("relay start");
        tokio::time::sleep(Duration::from_millis(50)).await;

        let payload = vec![0xAB_u8; 2 * 1024 * 1024];
        let started = std::time::Instant::now();
        let mut conn = TcpStream::connect(("127.0.0.1", listen_port)).await.unwrap();
        let writer = {
            let payload = payload.clone();
            async move {
                let (mut r, mut w) = conn.split();
                w.write_all(&payload).await.unwrap();
                w.shutdown().await.unwrap();
                let mut buf = Vec::with_capacity(payload.len());
                r.read_to_end(&mut buf).await.unwrap();
                buf
            }
        };
        let echoed = writer.await;
        let elapsed = started.elapsed();
        assert_eq!(echoed.len(), payload.len(), "数据必须完整");
        assert!(
            elapsed >= Duration::from_millis(180),
            "40Mbps 下 2MB 往返应明显被限速, got {elapsed:?}"
        );
        handle.stop().await;
    }
```

既有两个 tcp 测试的 `start(...)` 调用补第三参数 `None`。

Run: `cargo test -p node-agent tcp` → 新测试编译 FAIL（start 没有第三参数）。

实现：`relay/tcp.rs`

签名与传递：

```rust
use crate::limit::TokenBucket;

pub async fn start(
    rule: Rule,
    stats: Arc<StatsCollector>,
    bucket: Option<Arc<TokenBucket>>,
) -> Result<TcpRelayHandle> {
```

accept 循环里 spawn 前 `let bucket = bucket.clone();`（外层先 clone 进 task 同现有 counter 模式），bridge 调用改 `bridge(client, target_host, target_port, counter.clone(), bucket).await`。

`bridge` 改造（删除原 TODO(bandwidth) 注释；无限速路径保持 `tokio::io::copy` 快路径零回归）：

```rust
async fn bridge(
    mut client: TcpStream,
    target_host: String,
    target_port: u16,
    counter: Arc<RuleCounter>,
    bucket: Option<Arc<TokenBucket>>,
) -> Result<()> {
    let mut server = TcpStream::connect((target_host.as_str(), target_port))
        .await
        .with_context(|| format!("connect upstream {target_host}:{target_port}"))?;

    let (mut c_r, mut c_w) = client.split();
    let (mut s_r, mut s_w) = server.split();

    match bucket {
        // 限速:手动 chunk 循环,每块写前向共享桶取配额(rx+tx 同桶)。
        Some(bucket) => {
            let c2s = copy_limited(&mut c_r, &mut s_w, &bucket, &counter.tx_bytes);
            let s2c = copy_limited(&mut s_r, &mut c_w, &bucket, &counter.rx_bytes);
            tokio::try_join!(c2s, s2c)?;
        }
        // 不限速:维持 tokio::io::copy 快路径。
        None => {
            let tx_counter = counter.clone();
            let c2s = async {
                let n = tokio::io::copy(&mut c_r, &mut s_w).await?;
                tx_counter.tx_bytes.fetch_add(n as i64, Ordering::Relaxed);
                let _ = s_w.shutdown().await;
                Ok::<u64, std::io::Error>(n)
            };
            let rx_counter = counter.clone();
            let s2c = async {
                let n = tokio::io::copy(&mut s_r, &mut c_w).await?;
                rx_counter.rx_bytes.fetch_add(n as i64, Ordering::Relaxed);
                let _ = c_w.shutdown().await;
                Ok::<u64, std::io::Error>(n)
            };
            tokio::try_join!(c2s, s2c)?;
        }
    }
    Ok(())
}

/// 8KB chunk 复制:读 → acquire 配额 → 写 → 计数。EOF 时半关写端。
async fn copy_limited<R, W>(
    r: &mut R,
    w: &mut W,
    bucket: &TokenBucket,
    counted: &std::sync::atomic::AtomicI64,
) -> std::io::Result<u64>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buf = [0u8; 8192];
    let mut total = 0u64;
    loop {
        let n = r.read(&mut buf).await?;
        if n == 0 {
            let _ = w.shutdown().await;
            return Ok(total);
        }
        bucket.acquire(n).await;
        w.write_all(&buf[..n]).await?;
        counted.fetch_add(n as i64, Ordering::Relaxed);
        total += n as u64;
    }
}
```

（`RuleCounter` 的 `tx_bytes`/`rx_bytes` 为 `AtomicI64` 公有字段——与现有 `counter.tx_bytes.fetch_add` 用法一致。）

- [ ] **Step 5: UDP relay 接入**

`relay/udp.rs`：
- `pub async fn start(rule, stats, bucket: Option<Arc<TokenBucket>>)` 与 `start_inner(..., bucket)` 透传；顶部 `use crate::limit::TokenBucket;`。
- 入向（`forward` 调用点之前、`counter.tx_bytes.fetch_add` 之后）不动计数位置（保持「收到即计」既有语义）；在 `forward()` 内 send 前检查。`forward` 加参数 `bucket: &Option<Arc<TokenBucket>>`（事件循环里的调用点同步传 `&bucket`；spawn 进主 task 前先把 start_inner 收到的 bucket move 进闭包），函数体开头加：

```rust
    // 限速:配额不足直接丢包(UDP 语义,不阻塞事件循环)。
    if let Some(b) = bucket {
        if !b.try_acquire(data.len()) {
            counter.error_count.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
    }
```

- 反向 task（upstream → client）:spawn 前 `let bucket_back = bucket.clone();`，循环内 `send_to` 前加：

```rust
                    if let Some(b) = &bucket_back {
                        if !b.try_acquire(n) {
                            counter_clone.error_count.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                    }
```

（`rx_bytes` 计数移到 try_acquire 之后、send_to 之前不必动——现有顺序是 recv 即计；保持不动，丢包字节计入 rx 属「接收量」语义，注释说明即可。）
- tests 模块两处 `start(...)` / `start_with(...)` 调用补 `None`；`start_with` 辅助签名透传 bucket。

- [ ] **Step 6: manager + traits 清理**

`manager.rs::apply` 在 protocol match 前创建桶并传入：

```rust
        // P2 限速:per-rule 桶;tcp_udp 两个 listener 共享同一实例(rx+tx 合并计)。
        let bucket = crate::limit::TokenBucket::from_mbps(rule.bandwidth_mbps);
```

三个分支 `tcp::start(rule.clone(), self.stats.clone(), bucket.clone())` / `udp::start(rule.clone(), self.stats.clone(), bucket.clone())` 同步传参。

删除 `relay/traits.rs` 文件，`relay/mod.rs` 删去 `pub mod traits;`。grep 确认无 `QuotaGuard` / `null_quota` 残留引用。

- [ ] **Step 7: 跑测试验证通过**

Run: `cargo test -p node-agent && cargo test --workspace`
Expected: 全 PASS（含 4 个 bucket 单元 + tcp 限速 + 既有 relay 测试）。

- [ ] **Step 8: Commit**

```bash
git add crates/node-agent/src/limit crates/node-agent/src/main.rs crates/node-agent/src/manager.rs crates/node-agent/src/relay
git commit -m "feat(agent): per-rule token bucket; throttle TCP chunks, drop over-quota UDP packets"
```

## Task 8: 规则导入导出 API

**Files:**
- Create: `crates/panel-server/src/routes/rules_io.rs`
- Modify: `crates/panel-server/src/routes/mod.rs`
- Test: `crates/panel-server/tests/api_rules_io.rs`

- [ ] **Step 1: 写失败测试**

```rust
// crates/panel-server/tests/api_rules_io.rs
mod common;

use axum::http::{Method, StatusCode};
use serde_json::{json, Value};

async fn seed_node_named(app: &common::TestApp, name: &str) -> i64 {
    let res = sqlx::query(
        "INSERT INTO nodes (name, agent_token_hash, port_pool_min, port_pool_max) \
         VALUES (?, 'x', 20000, 29999)",
    )
    .bind(name)
    .execute(&app.state.pool)
    .await
    .unwrap();
    res.last_insert_rowid()
}

async fn create_rule(app: &common::TestApp, body: Value) -> Value {
    let req = common::auth_req(Method::POST, "/api/rules", &app.admin_token, Some(body)).unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

#[tokio::test]
async fn export_then_reimport_restores_rules() {
    let app = common::make_app().await.unwrap();
    let node_id = seed_node_named(&app, "io-node").await;
    // 带 profile 的规则
    let req = common::auth_req(
        Method::POST,
        "/api/bandwidth-profiles",
        &app.admin_token,
        Some(json!({ "name": "io-100m", "bandwidth_mbps": 100 })),
    )
    .unwrap();
    let (_, p) = common::send(app.app.clone(), req).await.unwrap();
    let pid = p["id"].as_i64().unwrap();

    let r = create_rule(
        &app,
        json!({ "node_id": node_id, "name": "io-r1", "protocol": "tcp_udp", "listen_port": 20000,
                "target_host": "1.2.3.4", "target_port": 443, "bandwidth_profile_id": pid }),
    )
    .await;
    let rule_id = r["id"].as_i64().unwrap();

    // export
    let req = common::auth_req(Method::GET, "/api/rules/export", &app.admin_token, None).unwrap();
    let (status, exported) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{exported}");
    let items = exported.as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["node_name"], "io-node");
    assert_eq!(items[0]["bandwidth_profile_name"], "io-100m");
    assert!(items[0]["tunnel_name"].is_null());
    assert!(items[0].get("id").is_none(), "导出不含 id");
    assert!(items[0].get("user_id").is_none(), "导出不含 user_id");

    // 删掉规则
    let req = common::auth_req(
        Method::DELETE,
        &format!("/api/rules/{rule_id}"),
        &app.admin_token,
        None,
    )
    .unwrap();
    let (status, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);

    // dry-run 预览:action=create 不写库
    let req = common::auth_req(
        Method::POST,
        "/api/rules/import?strategy=skip&dry_run=1",
        &app.admin_token,
        Some(exported.clone()),
    )
    .unwrap();
    let (status, report) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["items"][0]["action"], "create");
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM forward_rules WHERE deleted_at IS NULL",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(count, 0, "dry_run 不得写库");

    // 实导
    let req = common::auth_req(
        Method::POST,
        "/api/rules/import?strategy=skip&dry_run=0",
        &app.admin_token,
        Some(exported),
    )
    .unwrap();
    let (status, report) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["items"][0]["action"], "create");
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM forward_rules WHERE deleted_at IS NULL",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "规则数恢复");
}

#[tokio::test]
async fn import_marks_missing_node_as_error_without_write() {
    let app = common::make_app().await.unwrap();
    let payload = json!([{
        "name": "ghost", "protocol": "tcp", "listen_ip": "0.0.0.0", "listen_port": 20001,
        "target_host": "1.2.3.4", "target_port": 443, "enabled": true,
        "node_name": "no-such-node", "tunnel_name": null, "bandwidth_profile_name": null
    }]);
    let req = common::auth_req(
        Method::POST,
        "/api/rules/import?strategy=skip&dry_run=1",
        &app.admin_token,
        Some(payload),
    )
    .unwrap();
    let (status, report) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["items"][0]["action"], "error");
    assert!(report["items"][0]["reason"].as_str().unwrap().contains("node not found"));
}

#[tokio::test]
async fn import_conflict_strategies_skip_and_overwrite() {
    let app = common::make_app().await.unwrap();
    let node_id = seed_node_named(&app, "io2").await;
    create_rule(
        &app,
        json!({ "node_id": node_id, "name": "exist", "protocol": "tcp", "listen_port": 20010,
                "target_host": "1.1.1.1", "target_port": 80 }),
    )
    .await;
    let payload = json!([{
        "name": "incoming", "protocol": "tcp", "listen_ip": "0.0.0.0", "listen_port": 20010,
        "target_host": "9.9.9.9", "target_port": 443, "enabled": true,
        "node_name": "io2", "tunnel_name": null, "bandwidth_profile_name": null
    }]);

    // skip
    let req = common::auth_req(
        Method::POST,
        "/api/rules/import?strategy=skip&dry_run=0",
        &app.admin_token,
        Some(payload.clone()),
    )
    .unwrap();
    let (_, report) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(report["items"][0]["action"], "skip");

    // overwrite → PATCH 现有规则
    let req = common::auth_req(
        Method::POST,
        "/api/rules/import?strategy=overwrite&dry_run=0",
        &app.admin_token,
        Some(payload),
    )
    .unwrap();
    let (_, report) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(report["items"][0]["action"], "overwrite", "{report}");
    let host: String = sqlx::query_scalar(
        "SELECT target_host FROM forward_rules WHERE listen_port = 20010 AND deleted_at IS NULL",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(host, "9.9.9.9");
}

#[tokio::test]
async fn import_rejects_tunnel_items_and_requires_admin() {
    let app = common::make_app().await.unwrap();
    seed_node_named(&app, "io3").await;
    let payload = json!([{
        "name": "tun", "protocol": "tcp", "listen_ip": "0.0.0.0", "listen_port": 20020,
        "target_host": "1.2.3.4", "target_port": 443, "enabled": true,
        "node_name": "io3", "tunnel_name": "hk-jp", "bandwidth_profile_name": null
    }]);
    let req = common::auth_req(
        Method::POST,
        "/api/rules/import?dry_run=1",
        &app.admin_token,
        Some(payload),
    )
    .unwrap();
    let (_, report) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(report["items"][0]["action"], "error");
    assert!(report["items"][0]["reason"].as_str().unwrap().contains("tunnel"));

    // 非 admin → 403
    let (_uid, token) = common::make_user_token(&app, "iouser", "password123").await.unwrap();
    let req = common::auth_req(Method::GET, "/api/rules/export", &token, None).unwrap();
    let (status, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::FORBIDDEN);
}
```

- [ ] **Step 2: 跑测试验证失败**

Run: `cargo test -p panel-server --test api_rules_io`
Expected: FAIL（404 路由不存在）。

- [ ] **Step 3: 实现 rules_io.rs**

```rust
// crates/panel-server/src/routes/rules_io.rs
//! 规则导入导出(admin only)。导出不含 id/user_id/created_at(跨实例不可控);
//! 以 node_name / bandwidth_profile_name 做跨实例映射。tunnel_name 字段为
//! P3 预留:导出恒 null,导入非空报 error。
use crate::{
    audit,
    auth::extractor::{ActorIp, AuthUser},
    error::{ApiError, ApiResult},
    grpc::commands::apply_command,
    models::{bandwidth_profile::BandwidthProfile, node::Node, rule::Rule, settings},
    state::AppState,
};
use axum::{
    extract::{Query, State},
    http::header,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::prelude::FromRow;

#[derive(Serialize, Deserialize, FromRow)]
pub struct RuleExportItem {
    pub name: String,
    pub protocol: String,
    pub listen_ip: String,
    pub listen_port: u16,
    pub target_host: String,
    pub target_port: u16,
    pub enabled: bool,
    pub node_name: String,
    pub tunnel_name: Option<String>,
    pub bandwidth_profile_name: Option<String>,
}

#[derive(Deserialize)]
pub struct ExportQuery {
    pub node_id: Option<i64>,
    pub user_id: Option<i64>,
}

#[derive(FromRow)]
struct ExportRow {
    name: String,
    protocol: String,
    listen_ip: String,
    listen_port: i64,
    target_host: String,
    target_port: i64,
    enabled: i64,
    node_name: String,
    bandwidth_profile_name: Option<String>,
}

pub async fn export(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(q): Query<ExportQuery>,
) -> ApiResult<impl IntoResponse> {
    auth.require_admin()?;
    let mut where_parts = vec!["fr.deleted_at IS NULL".to_string()];
    if q.node_id.is_some() {
        where_parts.push("fr.node_id = ?".into());
    }
    if q.user_id.is_some() {
        where_parts.push("fr.user_id = ?".into());
    }
    let sql = format!(
        "SELECT fr.name, fr.protocol, fr.listen_ip, fr.listen_port, fr.target_host, \
                fr.target_port, fr.enabled, n.name AS node_name, \
                bp.name AS bandwidth_profile_name \
         FROM forward_rules fr \
         JOIN nodes n ON n.id = fr.node_id \
         LEFT JOIN bandwidth_profiles bp \
           ON bp.id = fr.bandwidth_profile_id AND bp.deleted_at IS NULL \
         WHERE {} ORDER BY fr.id",
        where_parts.join(" AND ")
    );
    let mut query = sqlx::query_as::<_, ExportRow>(&sql);
    if let Some(nid) = q.node_id {
        query = query.bind(nid);
    }
    if let Some(uid) = q.user_id {
        query = query.bind(uid);
    }
    let rows = query.fetch_all(&state.pool).await?;
    let items: Vec<RuleExportItem> = rows
        .into_iter()
        .map(|r| RuleExportItem {
            name: r.name,
            protocol: r.protocol,
            listen_ip: r.listen_ip,
            listen_port: r.listen_port as u16,
            target_host: r.target_host,
            target_port: r.target_port as u16,
            enabled: r.enabled != 0,
            node_name: r.node_name,
            tunnel_name: None,
            bandwidth_profile_name: r.bandwidth_profile_name,
        })
        .collect();
    Ok((
        [(
            header::CONTENT_DISPOSITION,
            "attachment; filename=\"emorelay-rules-export.json\"",
        )],
        Json(items),
    ))
}

#[derive(Deserialize)]
pub struct ImportQuery {
    pub strategy: Option<String>,
    pub dry_run: Option<u8>,
}

#[derive(Serialize)]
pub struct ImportItemReport {
    pub index: usize,
    pub action: &'static str, // create | skip | overwrite | error
    pub reason: String,
}

#[derive(Serialize)]
pub struct ImportReport {
    pub dry_run: bool,
    pub strategy: String,
    pub items: Vec<ImportItemReport>,
}

enum PlannedAction {
    Create {
        node_id: i64,
        bandwidth_profile_id: Option<i64>,
    },
    Overwrite {
        existing_id: i64,
        bandwidth_profile_id: Option<i64>,
    },
    Skip,
    Error(String),
}

pub async fn import(
    State(state): State<AppState>,
    auth: AuthUser,
    actor_ip: ActorIp,
    Query(q): Query<ImportQuery>,
    Json(items): Json<Vec<RuleExportItem>>,
) -> ApiResult<Json<ImportReport>> {
    auth.require_admin()?;
    let strategy = q.strategy.as_deref().unwrap_or("skip");
    if !matches!(strategy, "skip" | "overwrite") {
        return Err(ApiError::BadRequest("strategy must be skip | overwrite".into()));
    }
    let dry_run = q.dry_run.unwrap_or(1) != 0;
    let reserved = settings::reserved_ports(&state.pool).await;

    let mut report = Vec::with_capacity(items.len());
    let mut created = 0u32;
    let mut overwritten = 0u32;
    let mut skipped = 0u32;
    let mut errors = 0u32;

    for (index, item) in items.iter().enumerate() {
        let planned = plan_item(&state, item, strategy, &reserved).await?;
        let (action, reason): (&'static str, String) = match planned {
            PlannedAction::Error(reason) => ("error", reason),
            PlannedAction::Skip => ("skip", "binding already exists".into()),
            PlannedAction::Create { node_id, bandwidth_profile_id } => {
                if dry_run {
                    ("create", String::new())
                } else {
                    match execute_create(&state, &auth, item, node_id, bandwidth_profile_id).await {
                        Ok(()) => ("create", String::new()),
                        Err(e) => ("error", format!("create failed: {e}")),
                    }
                }
            }
            PlannedAction::Overwrite { existing_id, bandwidth_profile_id } => {
                if dry_run {
                    ("overwrite", format!("will patch rule #{existing_id}"))
                } else {
                    match execute_overwrite(&state, item, existing_id, bandwidth_profile_id).await {
                        Ok(()) => ("overwrite", format!("patched rule #{existing_id}")),
                        Err(e) => ("error", format!("overwrite failed: {e}")),
                    }
                }
            }
        };
        match action {
            "create" => created += 1,
            "overwrite" => overwritten += 1,
            "skip" => skipped += 1,
            _ => errors += 1,
        }
        report.push(ImportItemReport { index, action, reason });
    }

    if !dry_run {
        audit::record_with_ip(
            &state.pool,
            Some(auth.0.sub),
            actor_ip.as_option(),
            "rule.import",
            Some("rule"),
            None,
            Some(&format!(
                "strategy={strategy},created={created},overwritten={overwritten},skipped={skipped},errors={errors}"
            )),
            errors == 0,
            None,
        )
        .await;
    }

    Ok(Json(ImportReport {
        dry_run,
        strategy: strategy.to_string(),
        items: report,
    }))
}

/// 单项校验与映射,不写库。
async fn plan_item(
    state: &AppState,
    item: &RuleExportItem,
    strategy: &str,
    reserved: &[i64],
) -> ApiResult<PlannedAction> {
    if item.tunnel_name.as_deref().is_some_and(|t| !t.is_empty()) {
        return Ok(PlannedAction::Error(
            "tunnel feature unavailable until P3".into(),
        ));
    }
    if item.name.trim().is_empty() {
        return Ok(PlannedAction::Error("name is required".into()));
    }
    if !matches!(item.protocol.as_str(), "tcp" | "udp" | "tcp_udp") {
        return Ok(PlannedAction::Error("protocol must be tcp | udp | tcp_udp".into()));
    }
    if item.listen_port == 0 || item.target_port == 0 {
        return Ok(PlannedAction::Error("ports must be 1-65535".into()));
    }
    if !crate::util::is_valid_ip(&item.listen_ip) {
        return Ok(PlannedAction::Error("listen_ip is not a valid IP".into()));
    }
    if !crate::util::is_valid_target_host(item.target_host.trim()) {
        return Ok(PlannedAction::Error("target_host is not a valid IP or hostname".into()));
    }

    let Some(node) = Node::find_by_name(&state.pool, &item.node_name).await? else {
        return Ok(PlannedAction::Error(format!(
            "node not found: {}",
            item.node_name
        )));
    };
    let port = i64::from(item.listen_port);
    if port < node.port_pool_min || port > node.port_pool_max {
        return Ok(PlannedAction::Error(format!(
            "listen_port {} outside node's port pool [{}-{}]",
            port, node.port_pool_min, node.port_pool_max
        )));
    }
    if reserved.contains(&port) {
        return Ok(PlannedAction::Error(format!("listen_port {port} is reserved")));
    }

    // profile 找不到 → NULL(不自动创建,避免误植)
    let bandwidth_profile_id = match item.bandwidth_profile_name.as_deref() {
        None | Some("") => None,
        Some(name) => BandwidthProfile::find_by_name(&state.pool, name)
            .await?
            .map(|p| p.id),
    };

    // 冲突检测:精确同 binding → 按 strategy;互斥协议冲突 → error(那是另一条规则)。
    let exact: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM forward_rules \
         WHERE node_id = ? AND listen_ip = ? AND listen_port = ? AND protocol = ? \
           AND deleted_at IS NULL LIMIT 1",
    )
    .bind(node.id)
    .bind(&item.listen_ip)
    .bind(port)
    .bind(&item.protocol)
    .fetch_optional(&state.pool)
    .await?;
    if let Some((existing_id,)) = exact {
        return Ok(match strategy {
            "overwrite" => PlannedAction::Overwrite { existing_id, bandwidth_profile_id },
            _ => PlannedAction::Skip,
        });
    }
    let mutex_conflicts: &[&str] = match item.protocol.as_str() {
        "tcp" => &["tcp_udp"],
        "udp" => &["tcp_udp"],
        _ => &["tcp", "udp"],
    };
    let placeholders = vec!["?"; mutex_conflicts.len()].join(",");
    let sql = format!(
        "SELECT id FROM forward_rules \
         WHERE node_id = ? AND listen_ip = ? AND listen_port = ? \
           AND protocol IN ({placeholders}) AND deleted_at IS NULL LIMIT 1"
    );
    let mut mq = sqlx::query_scalar::<_, i64>(&sql)
        .bind(node.id)
        .bind(&item.listen_ip)
        .bind(port);
    for p in mutex_conflicts {
        mq = mq.bind(*p);
    }
    if mq.fetch_optional(&state.pool).await?.is_some() {
        return Ok(PlannedAction::Error(format!(
            "listen_port {port} conflicts with an existing rule of a mutually-exclusive protocol"
        )));
    }

    Ok(PlannedAction::Create {
        node_id: node.id,
        bandwidth_profile_id,
    })
}

async fn execute_create(
    state: &AppState,
    auth: &AuthUser,
    item: &RuleExportItem,
    node_id: i64,
    bandwidth_profile_id: Option<i64>,
) -> anyhow::Result<()> {
    let new_id = Rule::create(
        &state.pool,
        auth.0.sub,
        node_id,
        item.name.trim(),
        &item.protocol,
        &item.listen_ip,
        i64::from(item.listen_port),
        item.target_host.trim(),
        i64::from(item.target_port),
        bandwidth_profile_id,
    )
    .await?;
    if !item.enabled {
        Rule::set_enabled(&state.pool, new_id, false).await?;
    }
    if let Some(rule) = Rule::find_by_id(&state.pool, new_id).await? {
        if !state.dispatcher.dispatch(rule.node_id, apply_command(&rule)) {
            tracing::warn!(node_id = rule.node_id, rule_id = new_id, "agent offline; imported rule syncs at next register");
        }
    }
    Ok(())
}

async fn execute_overwrite(
    state: &AppState,
    item: &RuleExportItem,
    existing_id: i64,
    bandwidth_profile_id: Option<i64>,
) -> anyhow::Result<()> {
    Rule::update_fields(
        &state.pool,
        existing_id,
        Some(item.name.trim()),
        None,
        None,
        Some(item.target_host.trim()),
        Some(i64::from(item.target_port)),
        // None=不改;导入 profile 缺失映射为解除关联(0)
        Some(bandwidth_profile_id.unwrap_or(0)),
    )
    .await?;
    Rule::set_enabled(&state.pool, existing_id, item.enabled).await?;
    if let Some(rule) = Rule::find_by_id(&state.pool, existing_id).await? {
        if !state.dispatcher.dispatch(rule.node_id, apply_command(&rule)) {
            tracing::warn!(node_id = rule.node_id, rule_id = existing_id, "agent offline; overwrite syncs at next register");
        }
    }
    Ok(())
}
```

前置检查：`models/node.rs` 若无 `find_by_name` 则按 `find_by_id` 同款补一个（`WHERE name = ? AND deleted_at IS NULL`）。`util.rs` 的 `is_valid_ip` / `is_valid_target_host` 若非 `pub` 则改 pub（现已是 pub，rules.rs 在用）。

`routes/mod.rs`：`pub mod rules_io;` + 在 rules 路由块后注册：

```rust
        .route("/api/rules/export", get(rules_io::export))
        .route("/api/rules/import", post(rules_io::import))
```

注意：axum 路由 `/api/rules/export` 与 `/api/rules/{id}` 不冲突（静态段优先于参数段）。

- [ ] **Step 4: 跑测试验证通过**

Run: `cargo test -p panel-server --test api_rules_io && cargo test --workspace`
Expected: 全 PASS。

- [ ] **Step 5: Commit**

```bash
git add crates/panel-server/src/routes/rules_io.rs crates/panel-server/src/routes/mod.rs crates/panel-server/src/models/node.rs crates/panel-server/tests/api_rules_io.rs
git commit -m "feat(server): rules export/import with dry-run preview and skip/overwrite strategies"
```

## Task 9: 前端 — api.ts 用户类型 + Users 页（到期 / 30d 用量）+ Login 到期文案

**Files:**
- Create: `web/src/lib/quota.ts`、`web/src/lib/quota.test.ts`
- Modify: `web/src/lib/api.ts`、`web/src/pages/Users.tsx`、`web/src/pages/Login.tsx`

- [ ] **Step 1: 写失败测试（quota 纯函数）**

```typescript
// web/src/lib/quota.test.ts
import { describe, it, expect } from 'vitest'
import { quotaTone, quotaPercent, gbToBytes, bytesToGbString } from './quota'

describe('quota helpers', () => {
  it('quotaPercent clamps to 0-100 and handles null limit', () => {
    expect(quotaPercent(50, 100)).toBe(50)
    expect(quotaPercent(150, 100)).toBe(100)
    expect(quotaPercent(0, 100)).toBe(0)
    expect(quotaPercent(10, null)).toBeNull()
    expect(quotaPercent(10, 0)).toBeNull()
  })

  it('quotaTone: green <70, amber 70-90, red >=90', () => {
    expect(quotaTone(69)).toBe('green')
    expect(quotaTone(70)).toBe('amber')
    expect(quotaTone(89.9)).toBe('amber')
    expect(quotaTone(90)).toBe('red')
    expect(quotaTone(100)).toBe('red')
  })

  it('gbToBytes / bytesToGbString roundtrip', () => {
    expect(gbToBytes('1')).toBe(1073741824)
    expect(gbToBytes('0.5')).toBe(536870912)
    expect(gbToBytes('')).toBeNull()
    expect(gbToBytes('abc')).toBeUndefined()
    expect(bytesToGbString(1073741824)).toBe('1')
    expect(bytesToGbString(null)).toBe('')
  })
})
```

- [ ] **Step 2: 跑测试验证失败**

Run: `cd web && npx vitest run quota`
Expected: FAIL（模块不存在）。

- [ ] **Step 3: 实现 quota.ts**

```typescript
// web/src/lib/quota.ts
// 30 天用量进度条的纯函数(配色阈值 / GB↔bytes 转换)。

export type QuotaTone = 'green' | 'amber' | 'red'

/** used/limit 百分比,clamp 0-100;limit null/0 = 不限 → null。 */
export function quotaPercent(used: number, limit: number | null): number | null {
  if (limit == null || limit <= 0) return null
  return Math.min(100, Math.max(0, (used / limit) * 100))
}

/** 绿 <70 / 橙 70-90 / 红 ≥90。 */
export function quotaTone(percent: number): QuotaTone {
  if (percent >= 90) return 'red'
  if (percent >= 70) return 'amber'
  return 'green'
}

/** 表单 GB 字符串 → bytes。'' → null(不限);非法 → undefined(校验失败)。 */
export function gbToBytes(v: string): number | null | undefined {
  const s = v.trim()
  if (s === '') return null
  const n = Number(s)
  if (!Number.isFinite(n) || n < 0) return undefined
  return Math.round(n * 1024 ** 3)
}

/** bytes → 表单 GB 字符串(去尾零);null → ''。 */
export function bytesToGbString(bytes: number | null): string {
  if (bytes == null) return ''
  return String(parseFloat((bytes / 1024 ** 3).toFixed(2)))
}
```

Run: `cd web && npx vitest run quota` → PASS。

- [ ] **Step 4: api.ts 类型扩展**

`UserDetail` 追加：

```typescript
  expires_at: string | null
  traffic_limit_bytes_30d: number | null
  period_used_bytes_cached: number
  period_used_calculated_at: string | null
  period_remaining_bytes: number | null
```

`CreateUserRequest` 追加：

```typescript
  expires_at?: string | null
  traffic_limit_bytes_30d?: number | null
```

`UpdateUserRequest` 追加（注释置空协议）：

```typescript
  /** '' = 清除到期 */
  expires_at?: string
  /** 0 = 清除限额 */
  traffic_limit_bytes_30d?: number
```

- [ ] **Step 5: Users.tsx 列表两列 + 进度条 + 表单扩展**

顶部 import 补 `import { bytesToGbString, gbToBytes, quotaPercent, quotaTone } from '../lib/quota'`。

表头在「累计流量」后加两列：

```tsx
                        <th className="px-4 py-2.5 text-left font-medium">到期</th>
                        <th className="px-4 py-2.5 text-left font-medium">30d 用量</th>
```

`UserRow` 在「累计流量」单元格后加两个 `<td>`：

```tsx
      <td className="px-4 py-3 align-top text-[12px] text-zinc-300 whitespace-nowrap">
        {user.expires_at ? shortTime(user.expires_at) : '不限'}
      </td>
      <td className="px-4 py-3 align-top min-w-[10rem]">
        <QuotaBar used={user.period_used_bytes_cached} limit={user.traffic_limit_bytes_30d} />
      </td>
```

文件内（UserRow 之后）加组件：

```tsx
const TONE_CLS = {
  green: 'bg-emerald-500',
  amber: 'bg-amber-500',
  red: 'bg-red-500',
} as const

function QuotaBar({ used, limit }: { used: number; limit: number | null }) {
  const percent = quotaPercent(used, limit)
  if (percent == null) {
    return <span className="text-[12px] text-zinc-500">{formatBytes(used)} / 不限</span>
  }
  return (
    <div>
      <div className="h-1.5 w-full rounded-full bg-zinc-800 overflow-hidden">
        <div
          className={`h-full rounded-full ${TONE_CLS[quotaTone(percent)]}`}
          style={{ width: `${percent}%` }}
        />
      </div>
      <div className="text-[11px] text-zinc-500 mt-1">
        {formatBytes(used)} / {formatBytes(limit as number)}（{percent.toFixed(0)}%）
      </div>
    </div>
  )
}
```

`UserFormState` 加 `expires_at: string`（datetime-local 值）与 `traffic_limit_gb: string`；初始化：

```tsx
    expires_at: initial?.expires_at ? initial.expires_at.replace(' ', 'T').slice(0, 16) : '',
    traffic_limit_gb: bytesToGbString(initial?.traffic_limit_bytes_30d ?? null),
```

表单角色块后加两个输入：

```tsx
      <div className="grid grid-cols-2 gap-3">
        <div>
          <label className={fieldLabelCls}>到期时间 (UTC)</label>
          <input
            type="datetime-local"
            value={form.expires_at}
            onChange={(e) => setForm((f) => ({ ...f, expires_at: e.target.value }))}
            className={fieldInputCls}
          />
          <p className="text-[11px] text-zinc-500 mt-1">留空 = 永不到期。到期后规则自动停用、登录被拒。</p>
        </div>
        <div>
          <label className={fieldLabelCls}>30 天用量上限 (GB)</label>
          <input
            type="number"
            min={0}
            step="0.5"
            value={form.traffic_limit_gb}
            onChange={(e) => setForm((f) => ({ ...f, traffic_limit_gb: e.target.value }))}
            className={fieldInputCls}
            placeholder="留空 = 不限"
          />
          <p className="text-[11px] text-zinc-500 mt-1">滚动 30 天窗口;超限后该用户全部规则自动停用。</p>
        </div>
      </div>
```

`onSubmit` 提交逻辑：

```tsx
      const limitBytes = gbToBytes(form.traffic_limit_gb)
      if (limitBytes === undefined) {
        setError('30 天用量上限必须是非负数字')
        setSubmitting(false)
        return
      }
      if (mode === 'create') {
        // ...原有校验...
        const payload: CreateUserRequest = {
          username: form.username.trim(),
          password: form.password,
          role: form.role,
          expires_at: form.expires_at || null,
          traffic_limit_bytes_30d: limitBytes,
        }
        await users.create(payload)
      } else if (initial) {
        const payload: UpdateUserRequest = {}
        // ...原有 password/role 逻辑...
        const initialExpiresLocal = initial.expires_at
          ? initial.expires_at.replace(' ', 'T').slice(0, 16)
          : ''
        if (form.expires_at !== initialExpiresLocal) {
          payload.expires_at = form.expires_at // '' = 清除
        }
        const initialLimit = initial.traffic_limit_bytes_30d
        if ((limitBytes ?? 0) !== (initialLimit ?? 0)) {
          payload.traffic_limit_bytes_30d = limitBytes ?? 0 // 0 = 清除
        }
        if (Object.keys(payload).length === 0) {
          onCancel()
          return
        }
        await users.update(initial.id, payload)
      }
```

- [ ] **Step 6: Login.tsx 到期文案**

`onSubmit` catch 中 401 分支改为区分 message：

```tsx
      if (e instanceof ApiError) {
        if (e.status === 401 && e.message === 'account_expired') {
          setError('账号已到期，请联系管理员')
        } else if (e.status === 401) {
          setError('用户名或密码错误')
        } else {
          setError(e.message)
        }
      } else {
        setError('登录失败，请检查网络')
      }
```

- [ ] **Step 7: 验证**

Run: `cd web && npx vitest run && npm run build`
Expected: vitest 全 PASS（含 quota 3 个）；tsc + vite build 零错误。

- [ ] **Step 8: Commit**

```bash
git add web/src/lib/quota.ts web/src/lib/quota.test.ts web/src/lib/api.ts web/src/pages/Users.tsx web/src/pages/Login.tsx
git commit -m "feat(web): user expiry + 30d quota columns, form fields, progress bar; expired-login message"
```

---

## Task 10: 前端 — BandwidthProfiles 页 + Rules/RuleDetail 表单改造 + Settings 孤儿字段清理

**Files:**
- Create: `web/src/pages/BandwidthProfiles.tsx`
- Modify: `web/src/lib/api.ts`、`web/src/App.tsx`、`web/src/pages/Rules.tsx`、`web/src/pages/RuleDetail.tsx`、`web/src/pages/Settings.tsx`

- [ ] **Step 1: api.ts 类型与端点**

`RuleView` 删 `expires_at` / `traffic_limit_bytes` / `bandwidth_limit_mbps` 三行，加：

```typescript
  bandwidth_profile_id: number | null
  bandwidth_mbps: number | null
```

`CreateRuleRequest`：`listen_port: number` 改 `listen_port?: number`；删三个可选限制字段；加 `bandwidth_profile_id?: number | null`。
`UpdateRuleRequest`：删三字段；加 `/** 0 = 解除关联 */ bandwidth_profile_id?: number`。

新类型与端点组（放 users 端点组之后）：

```typescript
export interface BandwidthProfileView {
  id: number
  name: string
  bandwidth_mbps: number
  description: string
  created_at: string
  updated_at: string
}

export interface BandwidthProfileListResponse {
  items: BandwidthProfileView[]
  total: number
  page: number
  page_size: number
}

export const bandwidthProfiles = {
  list: (q: { page?: number; page_size?: number } = {}) => {
    const sp = new URLSearchParams()
    if (q.page) sp.set('page', String(q.page))
    if (q.page_size) sp.set('page_size', String(q.page_size))
    return api.get<BandwidthProfileListResponse>(`/api/bandwidth-profiles?${sp.toString()}`)
  },
  create: (req: { name: string; bandwidth_mbps: number; description?: string }) =>
    api.post<BandwidthProfileView>('/api/bandwidth-profiles', req),
  update: (id: number, req: { name?: string; bandwidth_mbps?: number; description?: string }) =>
    api.patch<BandwidthProfileView>(`/api/bandwidth-profiles/${id}`, req),
  del: (id: number) => api.del<{ ok: boolean }>(`/api/bandwidth-profiles/${id}`),
}
```

`SettingsResponse` 周边若有 settings key 联合类型注释则删去两个孤儿 key 引用。

- [ ] **Step 2: BandwidthProfiles.tsx（结构仿 Users.tsx：列表 + create/edit Modal + delete confirm）**

```tsx
// web/src/pages/BandwidthProfiles.tsx
import { useEffect, useState, type FormEvent } from 'react'
import {
  ApiError,
  bandwidthProfiles,
  shortTime,
  type BandwidthProfileView,
} from '../lib/api'
import { Modal, fieldInputCls, fieldLabelCls } from '../lib/ui'
import { Pagination } from '../components/Pagination'
import { useToast } from '../lib/use-toast'

type Editing = { mode: 'create' } | { mode: 'edit'; profile: BandwidthProfileView } | null

interface ListState {
  items: BandwidthProfileView[]
  total: number
  loading: boolean
  error: string | null
}

export default function BandwidthProfiles() {
  const toast = useToast()
  const [list, setList] = useState<ListState>({ items: [], total: 0, loading: true, error: null })
  const [editing, setEditing] = useState<Editing>(null)
  const [confirming, setConfirming] = useState<BandwidthProfileView | null>(null)
  const [busy, setBusy] = useState(false)
  const [page, setPage] = useState(1)
  const [pageSize, setPageSize] = useState(20)

  async function reload() {
    setList((s) => ({ ...s, loading: true, error: null }))
    try {
      const r = await bandwidthProfiles.list({ page, page_size: pageSize })
      setList({ items: r.items, total: r.total, loading: false, error: null })
    } catch (e) {
      const msg = e instanceof ApiError ? e.message : '加载失败'
      setList({ items: [], total: 0, loading: false, error: msg })
    }
  }

  useEffect(() => {
    let cancelled = false
    bandwidthProfiles
      .list({ page, page_size: pageSize })
      .then((r) => {
        if (!cancelled) setList({ items: r.items, total: r.total, loading: false, error: null })
      })
      .catch((e: unknown) => {
        if (cancelled) return
        const msg = e instanceof ApiError ? e.message : '加载失败'
        setList({ items: [], total: 0, loading: false, error: msg })
      })
    return () => {
      cancelled = true
    }
  }, [page, pageSize])

  async function doDelete(p: BandwidthProfileView) {
    setBusy(true)
    try {
      await bandwidthProfiles.del(p.id)
      toast.success('限速配置已删除')
      setConfirming(null)
      await reload()
    } catch (e) {
      toast.error(e instanceof ApiError ? e.message : '删除失败')
      setConfirming(null)
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="space-y-6">
      <div className="flex items-end justify-between gap-3">
        <div>
          <h2 className="text-xl font-semibold tracking-tight">限速配置</h2>
          <p className="text-sm text-zinc-400 mt-1">可复用的带宽上限模板，应用于转发规则</p>
        </div>
        <button
          onClick={() => setEditing({ mode: 'create' })}
          className="rounded-lg bg-indigo-600 hover:bg-indigo-500 px-3 py-2 text-sm font-medium shrink-0"
        >
          新增限速配置
        </button>
      </div>

      {list.error && (
        <div className="rounded-lg border border-red-500/30 bg-red-500/10 px-4 py-3 text-sm text-red-200">
          {list.error}
        </div>
      )}

      <section className="rounded-2xl border border-white/10 bg-zinc-900/40 overflow-hidden">
        {list.loading ? (
          <div className="p-6 text-sm text-zinc-400">加载中…</div>
        ) : list.items.length === 0 ? (
          <div className="p-6 text-sm text-zinc-500">尚无限速配置。点击右上角「新增限速配置」。</div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead className="text-[11px] uppercase text-zinc-500 bg-zinc-900/80">
                <tr>
                  <th className="px-4 py-2.5 text-left font-medium">名称</th>
                  <th className="px-4 py-2.5 text-right font-medium">带宽 (Mbps)</th>
                  <th className="px-4 py-2.5 text-left font-medium">描述</th>
                  <th className="px-4 py-2.5 text-left font-medium">更新于</th>
                  <th className="px-4 py-2.5 text-right font-medium">操作</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-white/5">
                {list.items.map((p) => (
                  <tr key={p.id} className="hover:bg-white/[0.02]">
                    <td className="px-4 py-3 align-top">
                      <div className="font-medium text-zinc-100">{p.name}</div>
                      <div className="text-[11px] text-zinc-500 mt-0.5">ID #{p.id}</div>
                    </td>
                    <td className="px-4 py-3 align-top text-right text-zinc-200 tabular-nums">
                      {p.bandwidth_mbps}
                    </td>
                    <td className="px-4 py-3 align-top text-zinc-400 text-[12px] max-w-[18rem] truncate">
                      {p.description || '—'}
                    </td>
                    <td className="px-4 py-3 align-top text-zinc-400 text-[12px]">
                      {shortTime(p.updated_at)}
                    </td>
                    <td className="px-4 py-3 align-top text-right whitespace-nowrap">
                      <button
                        type="button"
                        onClick={() => setEditing({ mode: 'edit', profile: p })}
                        className="rounded-md bg-zinc-800 hover:bg-zinc-700 px-2.5 py-1 text-xs"
                      >
                        编辑
                      </button>
                      <button
                        type="button"
                        onClick={() => setConfirming(p)}
                        className="ml-1.5 rounded-md bg-red-600/80 hover:bg-red-500 px-2.5 py-1 text-xs"
                      >
                        删除
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
        {!list.loading && list.items.length > 0 && (
          <Pagination
            page={page}
            pageSize={pageSize}
            total={list.total}
            onChangePage={setPage}
            onChangePageSize={(n) => {
              setPageSize(n)
              setPage(1)
            }}
          />
        )}
      </section>

      {editing && (
        <Modal
          title={editing.mode === 'create' ? '新增限速配置' : `编辑 · ${editing.profile.name}`}
          onClose={() => setEditing(null)}
        >
          <ProfileForm
            mode={editing.mode}
            initial={editing.mode === 'edit' ? editing.profile : undefined}
            onCancel={() => setEditing(null)}
            onSuccess={async () => {
              toast.success(editing.mode === 'create' ? '限速配置已创建' : '限速配置已保存')
              setEditing(null)
              await reload()
            }}
          />
        </Modal>
      )}

      {confirming && (
        <Modal title="删除限速配置" onClose={() => !busy && setConfirming(null)} size="sm">
          <p className="text-sm text-zinc-300">
            将删除 <span className="text-white font-medium">{confirming.name}</span>（
            {confirming.bandwidth_mbps} Mbps）。仍被规则引用时会被拒绝。
          </p>
          <div className="mt-5 flex justify-end gap-2">
            <button
              type="button"
              onClick={() => setConfirming(null)}
              disabled={busy}
              className="rounded-lg bg-zinc-800 hover:bg-zinc-700 px-3 py-2 text-sm"
            >
              取消
            </button>
            <button
              type="button"
              onClick={() => doDelete(confirming)}
              disabled={busy}
              className="rounded-lg bg-red-600 hover:bg-red-500 disabled:bg-zinc-700 disabled:cursor-not-allowed px-3 py-2 text-sm font-medium"
            >
              {busy ? '删除中…' : '确认删除'}
            </button>
          </div>
        </Modal>
      )}
    </div>
  )
}

function ProfileForm({
  mode,
  initial,
  onCancel,
  onSuccess,
}: {
  mode: 'create' | 'edit'
  initial?: BandwidthProfileView
  onCancel: () => void
  onSuccess: () => void | Promise<void>
}) {
  const [form, setForm] = useState({
    name: initial?.name ?? '',
    bandwidth_mbps: initial ? String(initial.bandwidth_mbps) : '',
    description: initial?.description ?? '',
  })
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function onSubmit(e: FormEvent) {
    e.preventDefault()
    setError(null)
    const mbps = Number(form.bandwidth_mbps)
    if (!Number.isInteger(mbps) || mbps <= 0) {
      setError('带宽必须是正整数 (Mbps)')
      return
    }
    if (!form.name.trim()) {
      setError('名称不能为空')
      return
    }
    setSubmitting(true)
    try {
      if (mode === 'create') {
        await bandwidthProfiles.create({
          name: form.name.trim(),
          bandwidth_mbps: mbps,
          description: form.description.trim(),
        })
      } else if (initial) {
        await bandwidthProfiles.update(initial.id, {
          name: form.name.trim() !== initial.name ? form.name.trim() : undefined,
          bandwidth_mbps: mbps !== initial.bandwidth_mbps ? mbps : undefined,
          description:
            form.description.trim() !== initial.description ? form.description.trim() : undefined,
        })
      }
      await onSuccess()
    } catch (e) {
      setError(e instanceof ApiError ? e.message : '提交失败')
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <form onSubmit={onSubmit} className="space-y-4">
      <div>
        <label className={fieldLabelCls}>名称 *</label>
        <input
          required
          value={form.name}
          onChange={(e) => setForm((f) => ({ ...f, name: e.target.value }))}
          className={fieldInputCls}
          placeholder="例如 100mbps-shared"
        />
      </div>
      <div>
        <label className={fieldLabelCls}>带宽 (Mbps) *</label>
        <input
          type="number"
          min={1}
          required
          value={form.bandwidth_mbps}
          onChange={(e) => setForm((f) => ({ ...f, bandwidth_mbps: e.target.value }))}
          className={fieldInputCls}
          placeholder="100"
        />
        <p className="text-[11px] text-zinc-500 mt-1">
          上下行合并计；修改后引用此配置的规则即时生效。
        </p>
      </div>
      <div>
        <label className={fieldLabelCls}>描述</label>
        <input
          value={form.description}
          onChange={(e) => setForm((f) => ({ ...f, description: e.target.value }))}
          className={fieldInputCls}
          placeholder="可选"
        />
      </div>
      {error && (
        <div className="rounded-lg border border-red-500/30 bg-red-500/10 px-3 py-2 text-xs text-red-200">
          {error}
        </div>
      )}
      <div className="flex justify-end gap-2 pt-1">
        <button
          type="button"
          onClick={onCancel}
          disabled={submitting}
          className="rounded-lg bg-zinc-800 hover:bg-zinc-700 px-3 py-2 text-sm"
        >
          取消
        </button>
        <button
          type="submit"
          disabled={submitting}
          className="rounded-lg bg-indigo-600 hover:bg-indigo-500 disabled:bg-zinc-700 disabled:cursor-not-allowed px-3 py-2 text-sm font-medium"
        >
          {submitting ? '提交中…' : mode === 'create' ? '创建' : '保存'}
        </button>
      </div>
    </form>
  )
}
```

- [ ] **Step 3: App.tsx 路由 + 导航**

- import 区加 `import BandwidthProfiles from './pages/BandwidthProfiles'`
- Routes 内 `users` 之后加 `<Route path="bandwidth-profiles" element={<BandwidthProfiles />} />`
- nav 区「用户」之后加 `<NavItem to="/bandwidth-profiles" label="限速" onClick={() => setDrawerOpen(false)} />`
- `CurrentRoute` 的 labels 加 `'/bandwidth-profiles': '限速',`

- [ ] **Step 4: Rules.tsx 表单改造**

- `RuleFormState`：删 `expires_at` / `traffic_limit_bytes` / `bandwidth_limit_mbps` 三字段，加 `bandwidth_profile_id: string`。
- 初始化删三行，加 `bandwidth_profile_id: initial?.bandwidth_profile_id != null ? String(initial.bandwidth_profile_id) : '',`。
- 组件签名/Modal 传参：`RuleForm` 增加 `profiles: BandwidthProfileView[]` prop；`Rules` 页与 nodeList 同款方式拉一次：

```tsx
  const [profileList, setProfileList] = useState<BandwidthProfileView[]>([])
  useEffect(() => {
    let cancelled = false
    bandwidthProfiles
      .list({ page_size: 100 })
      .then((r) => {
        if (!cancelled) setProfileList(r.items)
      })
      .catch(() => {
        // 拉取失败仅创建表单缺下拉项,不阻塞规则列表。
      })
    return () => {
      cancelled = true
    }
  }, [])
```

（import 区补 `bandwidthProfiles, type BandwidthProfileView`。）
- 监听端口 input：去掉 `required`，label 去 `*`，`placeholder="留空 = 自动分配"`。
- 「到期时间/总流量/带宽」三列 grid 整体替换为限速下拉：

```tsx
      <div>
        <label className={fieldLabelCls}>限速配置</label>
        <select
          value={form.bandwidth_profile_id}
          onChange={(e) => set('bandwidth_profile_id', e.target.value)}
          className={fieldInputCls}
        >
          <option value="">不限速</option>
          {profiles.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name}（{p.bandwidth_mbps} Mbps）
            </option>
          ))}
        </select>
        <p className="text-[11px] text-zinc-500 mt-1">
          在「限速」页维护可复用配置；到期与流量配额已移至用户维度。
        </p>
      </div>
```

- `onSubmit`：`parsePort(form.listen_port, ...)` 改为允许空：

```tsx
    let listenPort: number | undefined
    if (form.listen_port.trim() !== '') {
      const parsed = parsePort(form.listen_port, '监听端口')
      if (typeof parsed === 'string') return setError(parsed)
      listenPort = parsed
    }
```

  删 `trafficLimit` / `bandwidthLimit` 两段解析；create payload 删三字段加：

```tsx
          listen_port: listenPort,
          bandwidth_profile_id: form.bandwidth_profile_id ? Number(form.bandwidth_profile_id) : null,
```

  edit payload 删三字段；listen_port 行改 `listen_port: listenPort !== undefined && listenPort !== initial.listen_port ? listenPort : undefined,`；加：

```tsx
          bandwidth_profile_id:
            (form.bandwidth_profile_id ? Number(form.bandwidth_profile_id) : 0) !==
            (initial.bandwidth_profile_id ?? 0)
              ? form.bandwidth_profile_id
                ? Number(form.bandwidth_profile_id)
                : 0
              : undefined,
```

  编辑模式 listen_port 仍预填旧值（初始化已有），留空则不改。
- `RuleRow`：删 `expires_at` 显示块（状态列里 `{rule.expires_at && ...}`）；在协议小字旁追加限速展示：

```tsx
        <div className="text-[11px] text-zinc-500 mt-0.5">
          {protoLabel}
          {rule.bandwidth_mbps != null && ` · ${rule.bandwidth_mbps} Mbps`}
        </div>
```

（替换原 protoLabel 那行 div。）
- 顶部 import 删去不再使用的项（如 `shortTime` 若仅 expires 用到）。

- [ ] **Step 5: RuleDetail.tsx ConfigCard 同步**

「到期 / 总流量上限 / 带宽上限」三个 `<Row>` 替换为一行：

```tsx
        <Row k="限速" v={rule.bandwidth_mbps != null ? `${rule.bandwidth_mbps} Mbps` : '不限'} />
```

顶部 import 若 `formatBytes` 仅被删除行使用则按编译器提示清理（TrafficCard 仍用 formatBytes，保留）。

- [ ] **Step 6: Settings.tsx 孤儿字段清理**

删除 `default_traffic_limit_bytes` / `default_bandwidth_limit_mbps`：
- `SettingsFormState` 两行、`EMPTY_FORM` 两行
- 加载/保存两处映射（74-75、127-128 行语义位置）
- `keys` 数组两项
- 「默认总流量 / 默认带宽」两个输入框的整个 `grid grid-cols-2` 块

- [ ] **Step 7: 验证**

Run: `cd web && npx vitest run && npm run build`
Expected: 全 PASS + build 零错误（tsc 会暴露所有漏改引用）。

- [ ] **Step 8: Commit**

```bash
git add web/src
git commit -m "feat(web): bandwidth-profiles page; rule form uses profile + optional listen_port; drop rule-level limits UI"
```

---

## Task 11: 前端 — 规则导入导出 UI

**Files:**
- Modify: `web/src/lib/api.ts`、`web/src/pages/Rules.tsx`

- [ ] **Step 1: api.ts 导入导出函数**

```typescript
export interface RuleExportItem {
  name: string
  protocol: 'tcp' | 'udp' | 'tcp_udp'
  listen_ip: string
  listen_port: number
  target_host: string
  target_port: number
  enabled: boolean
  node_name: string
  tunnel_name: string | null
  bandwidth_profile_name: string | null
}

export interface ImportItemReport {
  index: number
  action: 'create' | 'skip' | 'overwrite' | 'error'
  reason: string
}

export interface ImportReport {
  dry_run: boolean
  strategy: string
  items: ImportItemReport[]
}
```

`rules` 端点组追加：

```typescript
  /** 按当前筛选导出并触发浏览器下载(需带 Authorization,不能用 <a href>)。 */
  exportDownload: async (q: { node_id?: number } = {}) => {
    const sp = new URLSearchParams()
    if (q.node_id) sp.set('node_id', String(q.node_id))
    const token = getToken()
    const res = await fetch(`/api/rules/export?${sp.toString()}`, {
      headers: token ? { Authorization: `Bearer ${token}` } : {},
    })
    if (!res.ok) {
      const err = (await res.json().catch(() => null)) as { error?: string; message?: string } | null
      throw new ApiError(res.status, err?.error ?? 'unknown', err?.message ?? res.statusText)
    }
    const blob = await res.blob()
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = 'emorelay-rules-export.json'
    a.click()
    URL.revokeObjectURL(url)
  },
  importRules: (items: RuleExportItem[], strategy: 'skip' | 'overwrite', dryRun: boolean) =>
    api.post<ImportReport>(
      `/api/rules/import?strategy=${strategy}&dry_run=${dryRun ? 1 : 0}`,
      items,
    ),
```

- [ ] **Step 2: Rules.tsx 工具栏按钮 + 导入流程**

import 区补 `type ImportReport, type RuleExportItem`。`Rules` 组件加状态：

```tsx
  const [importing, setImporting] = useState<{
    items: RuleExportItem[]
    report: ImportReport
    strategy: 'skip' | 'overwrite'
    submitting: boolean
  } | null>(null)
```

「新增规则」按钮左侧加两个按钮与隐藏 file input：

```tsx
        <div className="flex gap-2 shrink-0">
          <button
            onClick={async () => {
              try {
                await rules.exportDownload({
                  node_id: filters.node_id ? Number(filters.node_id) : undefined,
                })
                toast.success('已导出当前筛选规则')
              } catch (e) {
                toast.error(e instanceof ApiError ? e.message : '导出失败')
              }
            }}
            className="rounded-lg bg-zinc-800 hover:bg-zinc-700 px-3 py-2 text-sm"
          >
            导出
          </button>
          <label className="rounded-lg bg-zinc-800 hover:bg-zinc-700 px-3 py-2 text-sm cursor-pointer">
            导入
            <input
              type="file"
              accept="application/json,.json"
              className="hidden"
              onChange={(e) => void onImportFile(e)}
            />
          </label>
          {/* 原「新增规则」按钮移入本容器 */}
        </div>
```

组件内处理函数：

```tsx
  async function onImportFile(e: React.ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0]
    e.target.value = '' // 允许重复选同一文件
    if (!file) return
    let items: RuleExportItem[]
    try {
      items = JSON.parse(await file.text()) as RuleExportItem[]
      if (!Array.isArray(items)) throw new Error('not array')
    } catch {
      toast.error('文件不是合法的规则导出 JSON')
      return
    }
    try {
      const report = await rules.importRules(items, 'skip', true)
      setImporting({ items, report, strategy: 'skip', submitting: false })
    } catch (err) {
      toast.error(err instanceof ApiError ? err.message : '预检失败')
    }
  }

  async function changeStrategy(strategy: 'skip' | 'overwrite') {
    if (!importing) return
    try {
      const report = await rules.importRules(importing.items, strategy, true)
      setImporting({ ...importing, strategy, report })
    } catch (err) {
      toast.error(err instanceof ApiError ? err.message : '预检失败')
    }
  }

  async function confirmImport() {
    if (!importing) return
    setImporting({ ...importing, submitting: true })
    try {
      const report = await rules.importRules(importing.items, importing.strategy, false)
      const errs = report.items.filter((i) => i.action === 'error').length
      if (errs > 0) toast.error(`导入完成，${errs} 项失败`)
      else toast.success('导入完成')
      setImporting(null)
      await reload()
    } catch (err) {
      toast.error(err instanceof ApiError ? err.message : '导入失败')
      setImporting(null)
    }
  }
```

页面底部（confirming Modal 之后）加预览 Modal：

```tsx
      {importing && (
        <Modal
          title={`导入预览 · ${importing.items.length} 项`}
          onClose={() => !importing.submitting && setImporting(null)}
          size="lg"
        >
          <div className="flex items-center gap-3 mb-3 text-sm">
            <span className="text-zinc-400">冲突策略:</span>
            {(['skip', 'overwrite'] as const).map((s) => (
              <label key={s} className="inline-flex items-center gap-1.5 cursor-pointer">
                <input
                  type="radio"
                  name="import-strategy"
                  checked={importing.strategy === s}
                  onChange={() => void changeStrategy(s)}
                />
                {s === 'skip' ? '跳过 (skip)' : '覆盖 (overwrite)'}
              </label>
            ))}
          </div>
          <div className="max-h-80 overflow-y-auto rounded-lg border border-white/10">
            <table className="w-full text-sm">
              <thead className="text-[11px] uppercase text-zinc-500 bg-zinc-900/80 sticky top-0">
                <tr>
                  <th className="px-3 py-2 text-left font-medium">#</th>
                  <th className="px-3 py-2 text-left font-medium">规则</th>
                  <th className="px-3 py-2 text-left font-medium">动作</th>
                  <th className="px-3 py-2 text-left font-medium">说明</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-white/5">
                {importing.report.items.map((it) => {
                  const src = importing.items[it.index]
                  const tone =
                    it.action === 'error'
                      ? 'text-red-300'
                      : it.action === 'skip'
                        ? 'text-zinc-400'
                        : 'text-emerald-300'
                  return (
                    <tr key={it.index}>
                      <td className="px-3 py-2 text-zinc-500">{it.index + 1}</td>
                      <td className="px-3 py-2 text-zinc-200">
                        {src?.name ?? '—'}
                        <span className="text-[11px] text-zinc-500 ml-1.5 font-mono">
                          {src ? `${src.node_name}:${src.listen_port}/${src.protocol}` : ''}
                        </span>
                      </td>
                      <td className={`px-3 py-2 ${tone}`}>{it.action}</td>
                      <td className="px-3 py-2 text-[12px] text-zinc-400">{it.reason || '—'}</td>
                    </tr>
                  )
                })}
              </tbody>
            </table>
          </div>
          <div className="mt-4 flex justify-end gap-2">
            <button
              type="button"
              onClick={() => setImporting(null)}
              disabled={importing.submitting}
              className="rounded-lg bg-zinc-800 hover:bg-zinc-700 px-3 py-2 text-sm"
            >
              取消
            </button>
            <button
              type="button"
              onClick={() => void confirmImport()}
              disabled={importing.submitting}
              className="rounded-lg bg-indigo-600 hover:bg-indigo-500 disabled:bg-zinc-700 disabled:cursor-not-allowed px-3 py-2 text-sm font-medium"
            >
              {importing.submitting ? '导入中…' : '确认导入'}
            </button>
          </div>
        </Modal>
      )}
```

- [ ] **Step 3: 验证**

Run: `cd web && npx vitest run && npm run build`
Expected: 全 PASS + build 零错误。

- [ ] **Step 4: Commit**

```bash
git add web/src/lib/api.ts web/src/pages/Rules.tsx
git commit -m "feat(web): rules export download + import with dry-run preview modal"
```

## Task 12: 配置 / 脚本 / 文档收尾

**Files:**
- Modify: `scripts/seed-dev.py`、`.env.example`、`docs/api.md`、`README.md`、`docs/deployment.md`、`plan.md`、本文件

- [ ] **Step 1: seed-dev.py 同步**

规则种子元组（约 79 行起注释 + 数据）删去 `traffic_limit / bw / expires` 三列及对应注释；约 95-97 行的三个 `if ... body[...] = ...` 删除。脚本跑通标准：空库 + panel-server 运行时 `python scripts/seed-dev.py` 全部 POST 200。

- [ ] **Step 2: .env.example 同步**

- 删除 `PANEL_EXPIRY_SWEEP_SECS=300` 及其上方 3 行注释块（规则级 sweeper 已退役）。
- 同位置加：

```bash
# 用户级 sweeper:到期扫描间隔(默认 60s)与滚动 30 天流量配额扫描间隔(默认 300s)。
# 到期/超额用户名下全部规则自动停用,并记录聚合 audit。
PANEL_USER_EXPIRY_SWEEP_SECS=60
PANEL_USER_QUOTA_SWEEP_SECS=300
```

- `AGENT_STATS_INTERVAL_SECS` 已存在，确认无需重复添加。

- [ ] **Step 3: docs/api.md 更新**

- rules 资源：`listen_port` 标注「可空=自动分配（池内最小可用，排除 reserved 与互斥占用）」；删除 `traffic_limit_bytes` / `bandwidth_limit_mbps` / `expires_at` 字段说明小节（约 109-111 行）；加 `bandwidth_profile_id`（PATCH 传 0 解除）与响应字段 `bandwidth_mbps`。
- users 资源：加 `expires_at`（UTC、""清除）、`traffic_limit_bytes_30d`（0 清除）、响应字段 `period_used_bytes_cached` / `period_used_calculated_at` / `period_remaining_bytes`；login 失败新增 `account_expired` message 说明。
- 新增小节 `## Bandwidth Profiles`（5 个端点 + 删除保护 400）与 `## Rules Export / Import`（query 参数、JSON 格式示例、dry_run 报告格式、strategy 语义、tunnel_name P3 预留）。
- audit actions 列表（如有）加 `bandwidth_profile.*` / `rule.import` / `user.expired_auto_disable_rules` / `user.quota_exceeded_auto_disable_rules`。

- [ ] **Step 4: README.md / docs/deployment.md**

- README 功能列表：限速（bandwidth profiles + Agent token bucket）、用户到期/30 天配额、规则导入导出、端口自动分配各加一行。
- deployment.md：env 表同步 Step 2 的增删。

- [ ] **Step 5: plan.md 附录 + 本计划状态**

`plan.md` 附录「Phase 1」小节之后加：

```markdown
### Phase 2（2026-06-10 启动）

(6) 端口自动分配、(10) 到期搬用户、(13) 流量配额滚动 30 天、(9) 限速独立路由 + Agent token bucket、(12) 规则导入导出 —— 全部交付。

- Spec: `docs/superpowers/specs/2026-06-10-mvp-followups-design.md` §3
- Plan: `docs/superpowers/plans/2026-06-10-mvp-followups-phase-2.md`
- 12 个 Task 全部 spec ✅ + code quality ✅（subagent-driven flow）
- 规则级 expires/traffic/bandwidth 全链路退役(migration 0004 + proto reserved 8-10)
- 测试: `cargo test --workspace` 全绿(新增 bandwidth_profiles / port_alloc / rules_io / user_quota_sweeper / token_bucket);web vitest 全绿
```

本文件（phase-2 plan）顶部如有「占位」字样段落则确认已被本计划取代（Task 启动时已整体重写，无需再动）。

- [ ] **Step 6: 全量回归**

Run: `cargo test --workspace && cd web && npx vitest run && npm run build`
Expected: 全绿。

- [ ] **Step 7: Commit**

```bash
git add scripts/seed-dev.py .env.example docs/api.md README.md docs/deployment.md plan.md docs/superpowers/plans/2026-06-10-mvp-followups-phase-2.md
git commit -m "docs(p2): document Phase 2 features; sync env/seed/api docs; mark plan delivered"
```

---

## P2 验收清单（spec §3.7 对照）

- [ ] 旧 `forward_rules.expires_at` / `traffic_limit_bytes` / `bandwidth_limit_mbps` 全链路下线（DB 0004 + proto reserved + Agent store/relay + 前端表单/详情）→ Task 4 / 10
- [ ] 创建规则 listen_port 留空 → 拿到「最小可用」端口 → Task 6
- [ ] 用户改 expires_at 到过去 → 60s 内规则全停 + 登录被拒 → Task 2 / 5
- [ ] 用户 30d 用量超阈值 → 5 分钟内规则全停 → Task 5
- [ ] 限速 profile 改 50mbps → Agent 重建 token bucket（dispatch_referencing_rules + RuleManager.apply 重建；速率正确性由 token_bucket 单测 + tcp 限速测试守护，真实 iperf 留人工验收）→ Task 3 / 7
- [ ] 导出 → 删 → 导入 dry-run → confirm → 规则数恢复 → Task 8 测试 `export_then_reimport_restores_rules`
- [ ] 跨实例导入：node_name 缺失 → dry-run 标 error 不写库 → Task 8 测试 `import_marks_missing_node_as_error_without_write`
- [ ] DELETE bandwidth_profile 有引用 → 400 列出规则数 → Task 3 测试 `bandwidth_profile_delete_blocked_by_rule_reference`

## 执行注意（给实施会话）

1. 严格按 Task 1→12 顺序；Task 4 之前 Task 1-3 必须已 commit（DROP 列依赖加法列与新 API 已就位）。
2. 每个 Task 收尾跑其 Run 命令 + `cargo test --workspace`（前端 Task 用 vitest+build），全绿才 commit。
3. 每个 Task commit 后按 CLAUDE.md 流程 spawn `general-purpose` 子代理调 `superpowers:code-reviewer` 审查该 Task 改动文件（只读，三段式回报），阻塞性问题修完才进入下一 Task。
4. 不顺手改无关代码；发现计划与代码现状冲突时停下来报告，不擅自绕过。






