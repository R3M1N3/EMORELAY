# EMORELAY MVP 后续 · 设计文档

**日期**: 2026-06-10
**作者**: brainstorming session（用户 + Claude）
**状态**: 设计完成，待 review
**实施方式**: 三阶段，每阶段独立交付、独立 review、独立回归

## 0. 背景与目标

MVP（plan.md 第十二节 20 步）已于 2026-06-09 全部交付，验收 11/11 通过。本设计覆盖用户提出的 13 条增量建议，作为 MVP 之后第一波结构性增强。原则：

- 沿用 CLAUDE.md「最少代码、外科手术式改动、目标驱动」红线
- 每阶段结束前 `cargo test --workspace` + 前端 `npm run build` + 子代理 `superpowers:code-reviewer` 全绿
- protobuf / DB schema 一旦改动必须保留 PostgreSQL 兼容路径

## 1. 13 条建议归档

| # | 建议 | Phase |
|---|---|---|
| 1 | 节点一键安装 URL | P1 |
| 2 | Settings 加 Agent 上报端点 | P1 |
| 3 | 主控-Agent 双端加密 → 内置 CA + 默认 mTLS | P3 |
| 4 | 节点组隧道（HK→JP→US，TCP/TLS/WSS） | P3 |
| 5 | 新增「隧道」路由页 | P3 |
| 6 | 转发规则端口自动分配 | P2 |
| 7 | 右上角 Toast 提示 | P1 |
| 8 | 有规则的节点不允许删除 | P1 |
| 9 | 限速独立路由 + 可应用 | P2 |
| 10 | 规则去掉到期、用户加到期 | P2 |
| 11 | 创建规则默认 TCP+UDP | P1 |
| 12 | 规则导入导出 | P2 |
| 13 | 流量限额到用户每月 → 滚动 30 天 | P2 |

## 2. Phase 1 · 体验与防呆

### 2.1 范围

(7) Toast / (8) 防删节点 / (11) 默认 TCP+UDP / (2) Settings Agent 端点 / (1) 一键安装 URL。

### 2.2 (7) 全局 Toast

- 文件: `web/src/lib/toast.tsx`（新建）+ `web/src/App.tsx`
- API: `useToast().success(msg)` / `error(msg)` / `info(msg)`
- 实现: React Context + 右上角 `fixed` 容器 + slide-in 动画 + 4s 自动消失 + 手动关闭
- 替换所有页面里写操作成功/失败后的 inline 红卡片为 Toast；inline 红卡片保留作为加载失败兜底
- `App.tsx` 在 `AuthProvider` 外再包一层 `ToastProvider`

### 2.3 (8) 防删节点

- 文件: `crates/panel-server/src/routes/nodes.rs::delete`
- 前置: `SELECT id, name FROM forward_rules WHERE node_id = ? AND deleted_at IS NULL LIMIT 4`
- `count > 0` → 400 BadRequest，body 含前 3 条规则信息
- P3 隧道落地后这里扩展查 `tunnel_hops.node_id IS NOT NULL`，任一命中即拒
- 前端 Toast 红字 + Modal 提示，列出冲突规则名（链接到 `/rules?node_id=...`）

### 2.4 (11) 创建规则默认 TCP+UDP

- 文件: `web/src/pages/Rules.tsx::RuleForm`
- 初值 `protocol: 'tcp_udp'`
- 编辑模式照旧（沿用 `initial.protocol`）

### 2.5 (2) Settings Agent 端点

- DB: `system_settings` 新增 key `agent_control_endpoint`，值如 `https://relay.example.com:50051`
- 后端 `routes/system.rs::ALLOWED` 数组加这个 key
- 后端 `validate_setting` 加分支：必须 `http(s)://host[:port]`，空字符串允许（语义=未配置）
- 前端 Settings 页表单加输入框 + 帮助提示
- 节点详情页 / 创建节点成功 Modal 的「复制安装命令」按钮：未配置时禁用 + 提示「请先在设置页配置 Agent 上报端点」

### 2.6 (1) 一键安装 URL

#### 端点

新文件 `crates/panel-server/src/routes/install.rs`:
- `GET /install.sh?node=<id>` —— 返回参数化 bash 脚本（无需鉴权，幂等 200）
- `GET /dist/node-agent-linux-amd64` 与 `/dist/node-agent-linux-arm64` —— 静态文件 serve（从 `${PANEL_DATA_DIR}/agent-dist/` 读取）
- 这三个端点在 `routes/mod.rs` 注册时**不挂 admin 中间件**，但加 rate limit（IP 维度 60/min）防扫描

#### 安装命令

前端在节点创建成功 Modal + 节点详情页生成命令字符串：

```sh
curl -fsSL https://relay.example.com/install.sh?node=42 \
  | sudo bash -s -- --token=<明文 token>
```

- token 仍是「仅一次显示」语义；用户复制就用，丢失只能重新签
- P3 之后命令字符串扩展三个 `--ca-pem-b64=` / `--client-cert-pem-b64=` / `--client-key-pem-b64=` 参数（详见 §4.4）

#### install.sh 模板

脚本职责：
1. 解析 `--token=` 参数
2. `uname -m` 判断架构（x86_64 → amd64, aarch64 → arm64），下载对应二进制到 `/usr/local/bin/emorelay-agent`
3. 写 `/etc/emorelay/agent.env`：
   ```
   AGENT_NODE_ID=<注入>
   AGENT_TOKEN=<--token 参数值>
   AGENT_CONTROL_ENDPOINT=<注入：来自 system_settings.agent_control_endpoint>
   ```
4. 写 `/etc/systemd/system/emorelay-agent.service`（沿用 plan.md 附录 P2 清单已预留的 systemd 单元任务）
5. `systemctl daemon-reload && systemctl enable --now emorelay-agent`
6. `systemctl status emorelay-agent --no-pager` 输出供使用者肉眼验证

#### 二进制分发

- panel 镜像 build 时 cross-compile 两个 linux target（`docker/panel-server.Dockerfile` 加 `rustup target add` + `cargo build --release --target` 二步）
- 二进制 COPY 到 `/var/lib/emorelay/agent-dist/`
- `${PANEL_DATA_DIR}` 默认 `/var/lib/emorelay`（已有），通过 env 可覆盖

### 2.7 P1 验收清单

- [ ] 删节点（含规则）→ Toast 红色 + 提示
- [ ] 创建规则默认 TCP+UDP
- [ ] 任意写操作 success/error → 右上角 Toast
- [ ] 安装命令复制 + 干净 VM 执行 → Agent online
- [ ] Settings 页未配置 endpoint → 安装命令按钮禁用

## 3. Phase 2 · 用户与限速重构

### 3.1 范围

(6) 端口自动 / (10) 到期搬到用户 / (13) 流量配额到用户（滚动 30 天）/ (9) 限速独立路由 / (12) 规则导入导出。

### 3.2 Schema migration（`migrations/0002_phase2.sql`）

```sql
-- users 扩展
ALTER TABLE users ADD COLUMN expires_at TEXT;
ALTER TABLE users ADD COLUMN traffic_limit_bytes_30d INTEGER;
ALTER TABLE users ADD COLUMN period_used_bytes_cached INTEGER NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN period_used_calculated_at TEXT;

-- forward_rules 卸三字段（SQLite 3.35+ 原生 DROP COLUMN）
ALTER TABLE forward_rules DROP COLUMN expires_at;
ALTER TABLE forward_rules DROP COLUMN traffic_limit_bytes;
ALTER TABLE forward_rules DROP COLUMN bandwidth_limit_mbps;

-- bandwidth_profiles
CREATE TABLE bandwidth_profiles (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    bandwidth_mbps INTEGER NOT NULL CHECK (bandwidth_mbps > 0),
    description TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    deleted_at TEXT
);
CREATE UNIQUE INDEX idx_bandwidth_profiles_name_active
    ON bandwidth_profiles (name) WHERE deleted_at IS NULL;

ALTER TABLE forward_rules
    ADD COLUMN bandwidth_profile_id INTEGER REFERENCES bandwidth_profiles(id);
CREATE INDEX idx_forward_rules_bandwidth_profile_id
    ON forward_rules (bandwidth_profile_id);
```

migration 顶部用注释标注 PG 迁移路径（DROP COLUMN 一致；其余照旧）。

### 3.3 (6) 端口自动分配

- API: `POST /api/rules` 的 `listen_port` 改为 `Option<u16>`
- 后端 `routes/rules.rs::create` 在 `listen_port = None` 时调 `allocate_port(pool, node_id, listen_ip, protocol)`:
  - 候选 = `node.port_pool_min..=node.port_pool_max`
  - 排除 `reserved_ports`
  - 排除「占用集合」：一次性 `SELECT listen_port, protocol FROM forward_rules WHERE node_id=? AND listen_ip=? AND deleted_at IS NULL AND listen_port BETWEEN ? AND ?` 取出全部活跃绑定；按 protocol 互斥语义判定：
    - 新规则 `tcp` → 与现有 `tcp` 或 `tcp_udp` 冲突
    - 新规则 `udp` → 与现有 `udp` 或 `tcp_udp` 冲突
    - 新规则 `tcp_udp` → 与现有 `tcp` / `udp` / `tcp_udp` 冲突
  - 取最小可用；全部占用 → 400 `port pool exhausted`
- 前端 `Rules.tsx::RuleForm`：listen_port input 改为可空 + 占位符「留空 = 自动分配」+ `port_pool` hint 保留

### 3.4 (10)(13) 用户到期 + 流量配额

#### 字段语义

- `users.expires_at`：ISO8601，NULL = 不过期
- `users.traffic_limit_bytes_30d`：NULL = 不限
- `users.period_used_bytes_cached`：滚动 30 天用量缓存
- `users.period_used_calculated_at`：上次计算时间（用于判定 cache 新鲜度）

#### Sweeper

新文件 `crates/panel-server/src/sweeper/user_quota.rs`:
- 一个 tokio task 内开两个独立 `tokio::time::interval`（用 `tokio::select!` 多路复用）：
  - **expiry tick**: 周期 60s（env `PANEL_USER_EXPIRY_SWEEP_SECS` 覆盖，默认 60）
  - **quota tick**: 周期 300s（env `PANEL_USER_QUOTA_SWEEP_SECS` 覆盖，默认 300）
- expiry tick 流程：
  1. 扫 `users WHERE expires_at IS NOT NULL AND expires_at <= datetime('now') AND deleted_at IS NULL`
  2. 命中 user_id 名下所有 `enabled=1 AND deleted_at IS NULL` 的规则：
     - 原子 `UPDATE forward_rules SET enabled=0 WHERE user_id=? AND enabled=1 AND deleted_at IS NULL` —— 单次 UPDATE 落库
     - 逐条 dispatch `ApplyRule(enabled=false)`（拿 SELECT 列表）
     - 每用户一条 audit `user.expired_auto_disable_rules`，payload 形如 `user_id=N,disabled_rule_count=K,reason=expired`
- quota tick 流程：
  1. 刷 `period_used_bytes_cached`:
     ```sql
     UPDATE users SET
         period_used_bytes_cached = (
             SELECT COALESCE(SUM(rs.rx_bytes + rs.tx_bytes), 0)
             FROM rule_stats rs
             JOIN forward_rules fr ON rs.rule_id = fr.id
             WHERE fr.user_id = users.id
               AND rs.bucket_at >= datetime('now', '-30 days')
         ),
         period_used_calculated_at = datetime('now')
     WHERE deleted_at IS NULL
     ```
  2. 刷新后扫 `users WHERE period_used_bytes_cached > traffic_limit_bytes_30d AND traffic_limit_bytes_30d IS NOT NULL AND deleted_at IS NULL` → 停规则（语义同 expiry tick 步骤 2）
- audit action 命名：`user.expired_auto_disable_rules` / `user.quota_exceeded_auto_disable_rules`（每用户聚合一条，避免审计日志爆量）
- 不变量：quota tick 必须先刷 cache 再判定，禁止用过期 cache 判断超额

#### 登录路径

`POST /api/auth/login` 在 password 验证通过后追加一次过期检查：
```rust
if let Some(exp) = user.expires_at.as_ref() {
    if parse_sqlite_datetime(exp) <= Utc::now().timestamp() {
        return Err(ApiError::Unauthorized("account_expired".into()));
    }
}
```
前端 Login 页根据 error message 显示「账号已到期，请联系管理员」。

#### API/UI

- `POST /api/users` + `PATCH /api/users/:id` 加 `expires_at`、`traffic_limit_bytes_30d`
- `GET /api/users` + `GET /api/users/:id` 返回 `expires_at`、`traffic_limit_bytes_30d`、`period_used_bytes_cached`、`period_used_calculated_at`、`period_remaining_bytes`（计算字段：`max(0, limit - used)`）
- 前端 Users.tsx 表单：加「到期时间」`datetime-local` + 「月用量上限 (GB)」
- 前端 Users.tsx 列表：加「到期」「30d 用量」两列；30d 用量做进度条（绿 <70%、橙 70-90%、红 ≥90%）；空值显示「不限」

### 3.5 (9) 限速独立路由

#### REST API

新文件 `crates/panel-server/src/routes/bandwidth_profiles.rs`:
- `GET /api/bandwidth-profiles` list (分页)
- `POST /api/bandwidth-profiles` create
- `GET /api/bandwidth-profiles/:id`
- `PATCH /api/bandwidth-profiles/:id`
- `DELETE /api/bandwidth-profiles/:id` —— 软删；前置查 `SELECT COUNT(*) FROM forward_rules WHERE bandwidth_profile_id=? AND deleted_at IS NULL` > 0 → 400 列出引用规则数

#### 字段

`bandwidth_profiles` 表三字段：`name`（唯一活跃）/ `bandwidth_mbps`（>0）/ `description`。

#### Protobuf 改动

`crates/common/proto/control.proto` 的 `Rule` 消息：
- 删 `traffic_limit_bytes` 字段（已由 user quota 接管）
- 删 `bandwidth_limit_mbps` 字段
- 删 `expires_at_unix` 字段（已由 user expires_at 接管）
- 新增 `int64 bandwidth_mbps`（0 = 无限速；server dispatch 时 LEFT JOIN bandwidth_profiles 取值）

兼容性：proto 字段号不复用，删除字段标记为 `reserved`。

#### Agent token bucket

新文件 `crates/node-agent/src/limit/token_bucket.rs`:
- 简单实现：`struct TokenBucket { rate_bytes_per_sec, burst_bytes, tokens, last_refill }`
- API: `async fn acquire(&self, want: usize) -> ()`：不够则 `tokio::time::sleep`
- 双向共用一个桶（rx + tx 加一起算）
- `rate = bandwidth_mbps * 125_000` B/s
- `burst = rate / 5` ≈ 200ms 容量
- TCP `relay/tcp.rs::bridge` 改：每个 chunk 写之前 `bucket.acquire(chunk.len()).await`
- UDP `relay/udp.rs::session` 改：发包前 `try_acquire(len)`；不够 → 丢包 + `error_count += 1`（UDP 语义，不阻塞）
- profile 变更 → server dispatch 新 `ApplyRule(bandwidth_mbps=N)` → Agent 重建 TokenBucket（per rule_id）
- P3 隧道：限速桶仅在 `TUNNEL_ROLE_ENTRY` 的 TunnelTask 内生效；dispatcher 拆 Rule 时为 mid/exit 角色显式置 `bandwidth_mbps=0` 避免逐跳重复限速

### 3.6 (12) 规则导入导出

新端点（admin only）:

#### Export

`GET /api/rules/export?node_id=&tunnel_id=&user_id=`（三个 query 都可选；不传 = 全部）
返回 `Content-Type: application/json` + `Content-Disposition: attachment; filename=...`，body 为 JSON 数组：
```json
[
  {
    "name": "game-hk-jp",
    "protocol": "tcp_udp",
    "listen_ip": "0.0.0.0",
    "listen_port": 20000,
    "target_host": "1.2.3.4",
    "target_port": 443,
    "enabled": true,
    "node_name": "hk-relay-01",
    "tunnel_name": null,
    "bandwidth_profile_name": "100mbps-shared"
  }
]
```
不包含 `id` / `user_id` / `created_at`（跨实例不可控）。

#### Import

`POST /api/rules/import?strategy=skip|overwrite&dry_run=1`
- body 同 export 格式
- `dry_run=1`：返回每项 `{index, action: "create|skip|overwrite|error", reason: "..."}`，不写库
- `dry_run=0`：实际执行；返回同样的报告
- 映射规则:
  - `node_name` 找节点（活跃）；找不到 → `error: node not found`
  - `bandwidth_profile_name` 找 profile（活跃）；找不到 → `bandwidth_profile_id = NULL`（**不自动创建**，避免误植）
  - `tunnel_name` 在 P2 阶段非空 → `error: tunnel feature unavailable until P3`
  - 冲突检测：`(node_id, listen_ip, listen_port, protocol)` 命中现有活跃规则 → 按 strategy 决定 skip / overwrite（overwrite 即 PATCH 现有规则）
  - `user_id` = 当前登录 admin 的 sub（导入的规则归属导入者）

#### 前端

- `Rules.tsx` 工具栏右侧加「导出」「导入」按钮
- 导出按当前筛选条件 → 浏览器下载 JSON 文件
- 导入：file input → 上传 → dry-run 预览 modal 显示每项 action + reason → 用户选 strategy + 确认 → 真正提交

### 3.7 P2 验收清单

- [ ] 旧 `forward_rules.expires_at`/`traffic_limit_bytes`/`bandwidth_limit_mbps` 全链路下线（DB + proto + Agent + 前端）
- [ ] 创建规则 listen_port 留空 → 拿到「最小可用」端口
- [ ] 用户改 expires_at 到过去 → 60s 内规则全停 + 登录被拒
- [ ] 用户 30d 用量超阈值 → 5 分钟内规则全停
- [ ] 限速 profile 改 50mbps → 上 iperf 测约 50mbps（±10%）
- [ ] 导出 → 删 → 导入 dry-run → confirm → 规则数恢复
- [ ] 跨实例导入：node_name 缺失 → dry-run 标 error 不写库
- [ ] DELETE bandwidth_profile 有引用 → 400 列出规则数

## 4. Phase 3 · 多跳隧道 + 内置 CA + 默认 mTLS

### 4.1 范围

(3) 内置 CA + 默认 mTLS / (4) 节点多跳隧道（TCP/TLS/WSS） / (5) 隧道路由页。

### 4.2 Schema migration（`migrations/0003_phase3.sql`）

```sql
CREATE TABLE tunnels (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    transport TEXT NOT NULL CHECK (transport IN ('tcp', 'tls', 'wss')),
    status TEXT NOT NULL DEFAULT 'unknown'
           CHECK (status IN ('up', 'degraded', 'down', 'unknown')),
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    deleted_at TEXT
);
CREATE UNIQUE INDEX idx_tunnels_name_active
    ON tunnels (name) WHERE deleted_at IS NULL;

CREATE TABLE tunnel_hops (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    tunnel_id INTEGER NOT NULL REFERENCES tunnels(id),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    node_id INTEGER NOT NULL REFERENCES nodes(id),
    inter_port INTEGER
               CHECK (inter_port IS NULL OR (inter_port BETWEEN 1 AND 65535)),
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE UNIQUE INDEX idx_tunnel_hops_tunnel_ordinal
    ON tunnel_hops (tunnel_id, ordinal);
CREATE INDEX idx_tunnel_hops_node_id ON tunnel_hops (node_id);

ALTER TABLE forward_rules
    ADD COLUMN tunnel_id INTEGER REFERENCES tunnels(id);
CREATE INDEX idx_forward_rules_tunnel_id ON forward_rules (tunnel_id);
```

字段语义关键点：
- `forward_rules.node_id` 走隧道时**必须等于 ordinal=0 的 node_id**（入口节点）。后端 INSERT 校验。
- `inter_port` 由后端分配：从入口节点 port_pool 内取未占用（与现有 listen_port 共享端口池；同样排除 reserved）。
- 删隧道前置：`forward_rules.tunnel_id = X AND deleted_at IS NULL` 命中 → 400。
- 删节点前置（Phase 1 已加）扩展查 `tunnel_hops.node_id` 命中 → 400。

### 4.3 Protobuf 改动

`crates/common/proto/control.proto` 的 `Rule` 消息增加 tunnel context:

```proto
message Rule {
  // ...沿用 P2 之后的字段（id, protocol, listen_ip, listen_port, target_host,
  //    target_port, enabled, bandwidth_mbps）
  TunnelContext tunnel = 11;
}

message TunnelContext {
  int64 tunnel_id = 1;
  TunnelRole role = 2;
  string next_hop_addr = 3;
  uint32 next_hop_inter_port = 4;
  uint32 self_inter_port = 5;
  string transport = 6;
}

enum TunnelRole {
  TUNNEL_ROLE_UNSPECIFIED = 0;
  TUNNEL_ROLE_ENTRY = 1;
  TUNNEL_ROLE_MID = 2;
  TUNNEL_ROLE_EXIT = 3;
}

// Command oneof 新增 6/7 两个分支（与现有 5 个并列）：
// message Command {
//   oneof body {
//     ApplyRule apply_rule = 1;
//     RemoveRule remove_rule = 2;
//     EnableRule enable_rule = 3;
//     DisableRule disable_rule = 4;
//     RestartRule restart_rule = 5;
//     TunnelCredentials tunnel_credentials = 6;  // Phase 3 新增
//     RevokeTunnelCredentials revoke_tunnel_credentials = 7;  // Phase 3 新增
//   }
// }
message TunnelCredentials {
  int64 tunnel_id = 1;
  int32 ordinal = 2;
  // 该 hop 的 server / client 证书（PEM），由面板 CA 在隧道创建时签发。
  // Agent 落盘到 ${AGENT_DATA_DIR}/tunnels/<id>/hop-<ordinal>/{server,client}.{pem,key}。
  string server_cert_pem = 3;
  string server_key_pem = 4;
  string client_cert_pem = 5;
  string client_key_pem = 6;
}

message RevokeTunnelCredentials {
  int64 tunnel_id = 1;
}
```

Server 端 dispatch 时按 hop 拆 `Rule`：同一 rule_id 在 N 个节点上各跑一份不同角色的实例。dispatcher 拆分时为 mid/exit 角色显式置 `bandwidth_mbps=0`（限速只在 entry 起作用，避免重复扣量）。

### 4.4 内置 CA + 默认 mTLS

#### Bootstrap

新文件 `crates/panel-server/src/tls/ca.rs`:
- 用 `rcgen` crate 生成
- 启动检查 `${PANEL_DATA_DIR}/tls/ca.pem`；不存在则:
  - ECDSA P-256 自签 CA，10 年有效期
  - 签 server 证书（SAN: `127.0.0.1`、`localhost`、`PANEL_PUBLIC_HOST` env 值）
  - 落盘 `tls/{ca.pem, ca.key, server.pem, server.key}`，权限 0600
- gRPC server 启动 `tonic` TLS 用内置 server 证书；强制 mTLS（启用 `ClientCertVerifier`）
- 验证 client cert 由本 CA 签名 + 未在 CRL 中

#### dev override

`PANEL_DEV_DISABLE_MTLS=1` → 退回 plaintext（warn 一行）。开发期跑 `cargo run` 不强制每次签证书。

#### 节点凭据

`POST /api/nodes` 响应除 `agent_token` 外新增:
- `ca_pem` —— CA 公钥
- `client_cert_pem` —— 该 node 的 client cert（SAN: `node-<id>.emorelay.internal`）
- `client_key_pem`

四件套**仅一次性返回**。前端 Modal 改为四块独立内容 + 各自复制按钮 + 折叠展开（避免页面过长）。

DB 不存 client cert / key 明文。存 `cert_serial` + `cert_fingerprint`（新字段 `nodes.cert_serial`、`nodes.cert_fingerprint`，在 0003 migration 一并加）供审计 + 吊销。

#### install.sh 升级

Phase 3 起接收三个额外参数:
- `--ca-pem-b64=<base64>`
- `--client-cert-pem-b64=<base64>`
- `--client-key-pem-b64=<base64>`

写到 `/etc/emorelay/tls/{ca,client,client-key}.pem`，权限 0600。

env file 加:
```
AGENT_GRPC_CA_CERT=/etc/emorelay/tls/ca.pem
AGENT_GRPC_CLIENT_CERT=/etc/emorelay/tls/client.pem
AGENT_GRPC_CLIENT_KEY=/etc/emorelay/tls/client-key.pem
AGENT_CONTROL_ENDPOINT=https://relay.example.com:50051
```

前端生成安装命令时把三个 PEM base64 后嵌入命令字符串。复制走就丢。

#### 节点吊销

新 API `POST /api/nodes/:id/revoke-credentials`:
- 把现有 cert serial 加入 CRL（落到 `${PANEL_DATA_DIR}/tls/crl.pem`）
- `ClientCertVerifier` 启动时加载 CRL + 文件 mtime 监听，变更后 in-memory 刷新
- 重新签一份 client cert 返回（与创建节点 Modal 同款一次性显示四件套）
- audit `node.credentials_revoked`

### 4.5 隧道 REST API

新文件 `crates/panel-server/src/routes/tunnels.rs`:

```
GET    /api/tunnels                       list（含 hops count）
POST   /api/tunnels                       create { name, transport, node_ids: [N, ...] }
GET    /api/tunnels/:id                   含 hops 详情（含 inter_port）
PATCH  /api/tunnels/:id                   仅可改 name；hops/transport 不可改
DELETE /api/tunnels/:id                   软删；有规则引用 → 400
POST   /api/tunnels/:id/restart           下发到每个 hop 节点重启 TunnelTask
GET    /api/tunnels/:id/status            聚合每个 hop 的 up/down
```

POST 创建逻辑：
1. 校验 `node_ids` 长度 ≥ 2 且各 id 存在 + 不重复
2. 校验每个 node 状态 = `online`（否则提示「请确保链上所有节点都在线」）
3. 为每个 ordinal < N-1 的节点从其 port_pool 内分配 inter_port（最小可用、排除 reserved）
4. 事务写入 tunnels + N 行 tunnel_hops
5. dispatch 到每个 hop 节点：构造 `Rule.tunnel` context 但不立即下发完整 Rule（隧道本身不带业务规则，等待规则关联）
6. audit `tunnel.create`

### 4.6 Agent 隧道模块

新目录 `crates/node-agent/src/tunnel/`:

#### transport.rs

```rust
#[async_trait]
trait TunnelTransport: Send + Sync {
    type Conn: AsyncRead + AsyncWrite + Send + Unpin + 'static;
    async fn dial(&self, addr: &str) -> Result<Self::Conn>;
    async fn bind(&self, addr: &str) -> Result<Self::Listener>;
    // Listener::accept() -> Self::Conn
}
```

三实现：
- `tcp_transport.rs` —— 裸 TCP
- `tls_transport.rs` —— TLS（rustls）。隧道 TLS 与控制面 mTLS 复用同一 CA：
  - 隧道创建时，server 端按 `(tunnel_id, ordinal)` 为每个非入口 hop 节点签一份 server 证书（SAN = `tunnel-<id>-hop-<ordinal>.emorelay.internal`）+ 一份 client cert（同 SAN）
  - 通过 `Command.tunnel_credentials`（新增 oneof 分支，仅 Phase 3）下发给对应 Agent，Agent 落盘到 `${AGENT_DATA_DIR}/tunnels/<id>/hop-<ordinal>/{server,client}.{pem,key}`
  - 拨号方强制 SNI = `tunnel-<id>-hop-<ordinal>.emorelay.internal`，忽略 `next_hop_addr` 的真实 hostname（hostname 用于路由，SNI 用于身份验证）
  - 隧道删除时 Agent 清理对应目录
- `wss_transport.rs` —— WebSocket over TLS（`tokio-tungstenite` + rustls）

#### task.rs

`TunnelTask` 实例 per `(rule_id, role)`:
- **Entry**: 在 `(rule.listen_ip, rule.listen_port)` `TcpListener::bind`（业务流是 TCP）或 `UdpSocket::bind`（UDP）。新连接 → `transport.dial(next_hop_addr:next_hop_inter_port)` → `bridge()` 双向复制（带 token bucket 限速）
- **Mid**: 在 `(0.0.0.0, self_inter_port)` `transport.bind`。新连接 → `transport.dial(next_hop_addr:next_hop_inter_port)` → `bridge()`
- **Exit**: 在 `(0.0.0.0, self_inter_port)` `transport.bind`。新连接 → `TcpStream::connect(target_host:target_port)`（业务流落回裸 TCP/UDP）→ `bridge()`

UDP 走隧道帧定义：2 字节大端长度前缀 + payload。仅 entry/exit 之间打包/拆包；mid 跳就是字节流。

#### RuleManager 扩展

`crates/node-agent/src/manager.rs`:
- 收到 `Rule` 时检测 `tunnel` 字段
- 走隧道 → 启动 `TunnelTask` 而非 `TcpRelayTask/UdpRelayTask`
- 限速（Phase 2 的 `bandwidth_mbps`）在 entry 角色起作用即可

#### ConfigStore 扩展

`crates/node-agent/src/store.rs`:
- 序列化时存 tunnel context
- 重启后能恢复隧道角色继续跑

### 4.7 前端隧道路由

新文件:
- `web/src/pages/Tunnels.tsx`
- `web/src/pages/TunnelDetail.tsx`

App.tsx 路由加 `/tunnels` 与 `/tunnels/:id`。sidebar 新增「隧道」入口。

- 列表：name / transport / hops 数 / 状态 / 关联规则数 / 操作（删除/重启）
- 创建表单：name + transport 单选（TCP / TLS / WSS）+ 节点链构造器（多个下拉，可拖动排序；最少 2 个节点；同一节点不可出现两次）
- 详情：hops 表（ordinal / 节点名 / inter_port / 角色显示） + 该隧道下的规则列表

`Rules.tsx::RuleForm` 加「关联隧道」可选下拉（含「不关联」选项）：选了隧道后 `node_id` 自动填为入口节点（隧道详情中 ordinal=0 的节点）且禁用 select。

### 4.8 P3 验收清单

- [ ] `cargo run -p panel-server`（无任何 TLS env）→ 启动自动签 CA + server cert，gRPC 强制 TLS
- [ ] `PANEL_DEV_DISABLE_MTLS=1` → 退回 plaintext + 启动 warn
- [ ] 创建节点 → Modal 显示 token + CA + client cert + key 四块
- [ ] 复制安装命令 → 在干净 VM 跑 → Agent 自动启用 mTLS 连上 + 在线
- [ ] 创建隧道 `HK → JP → US`，transport=TLS → 隧道状态 up
- [ ] 在隧道上挂规则 `listen 0.0.0.0:20000 → target 1.2.3.4:443`：curl 20000 能到 443
- [ ] 删 JP 节点 → 拒绝（节点参与隧道）
- [ ] 删隧道 → 拒绝（有规则）→ 先删规则 → 再删隧道 → OK
- [ ] 吊销节点凭据 → 旧 Agent 立即被 server 端断开 TLS
- [ ] `cargo test --workspace` 跑通双跳/三跳 TCP/TLS 隧道 e2e

### 4.9 P3 内部拆分（review 单元）

严格按 CLAUDE.md「每完成一个原子单元 spawn 子代理 review」：

1. 引入 `rcgen` + CA / server cert 生成 + 启动落盘
2. 强制 mTLS（含 ClientCertVerifier + CRL + dev override）
3. 创建节点响应增加四件套 + install.sh 接收三个 base64 PEM + nodes 表加 cert_serial/fingerprint 列
4. tunnels / tunnel_hops migration + REST API + 前端 CRUD
5. proto Rule.tunnel 字段 + dispatcher 按 hop 拆 Rule
6. Agent `tunnel/` 模块 + 三 transport（TCP 先，TLS 次，WSS 最后）
7. 节点删除保护扩展 + 吊销 API + CRL 热加载
8. 端到端 e2e 测试（双跳 + 三跳 + TCP/TLS 矩阵）

## 5. 风险与待办

### 5.1 已识别风险

- **migration 0002 DROP COLUMN**: SQLite 3.35+ 支持，Docker 镜像内 sqlite 版本要在 Dockerfile 锁定 ≥ 3.35（当前 sqlx 用的是 bundled sqlite，版本充足，无风险）
- **install.sh 二进制下载**: 公网暴露 `/dist/*` 加 rate limit（IP 维度 60/min），防爬虫扫描；后续可改为短期签名 URL
- **token bucket 测试稳定性**: bandwidth 限速测试在 CI 上偏好用 mock clock（`tokio::time::pause`）替代真实 iperf
- **mTLS dev 体验**: 强制 mTLS 后 `cargo run` 默认要先生成 CA，会拖慢首跑；`PANEL_DEV_DISABLE_MTLS=1` 是默认推荐用法。Agent 端 `AGENT_CONTROL_ENDPOINT` 用 `http://` 即可走 plaintext（沿用 `control.rs:38` 已有判逻辑），无需新 env
- **P3 落地时存量节点的凭据兼容**: P1/P2 阶段创建的节点没有 client cert，P3 启用 mTLS 后会全部连不上。落地流程必须包含一步迁移：P3 启动时检测「nodes 表存在但 cert_serial IS NULL」的活跃节点 → 自动为每个签发一份 client cert，在 audit 写一条 `node.mtls_credentials_issued`；管理员需要到面板「轮换凭据」一次拿到明文 cert + key 重装 Agent。文档中明确警告「升级 P3 等同 fleet-wide Agent 重装」
- **隧道 UDP 帧重组**: MVP 不做乱序重组、不做重试。单包 > 64KB 时直接丢；前端 Tunnel 详情页 status 显示「up but UDP fragmentation warning」
- **隧道状态汇报**: P3 阶段先用「最近 30s 内有 hop 心跳上来 = up」，不做基于业务连接的 health check（后续 P4 可加 internal probe 包）

### 5.2 P2 清单状态

`plan.md` 附录 P2 清单原 9 项里有 4 项落入本设计：

- ~~relay/traits.rs::QuotaGuard trait 占位 + bridge() hot path TODO(bandwidth) 锚点~~ → P2 (9) 限速
- ~~grpc/dispatcher.rs SubscribeCommands stream 终止时 Drop guard 清理 dead sender~~ → 已在 P1 之前完成（service.rs:557-583）
- ~~gRPC server 端 mTLS 客户端证书校验~~ → P3 (3)
- .env.example 补 AGENT_STATS_INTERVAL_SECS / PANEL_EXPIRY_SWEEP_SECS → 在各 Phase 改动 .env 时顺手补
- ~~前端引入 vitest + 关键页面渲染 smoke~~ → 已完成（`web/src/lib/api.test.ts`、`web/src/components/Pagination.test.tsx`）
- UDP session 超时测试 → P3 隧道 UDP 段顺手加
- 独立 emorelay-agent.service systemd 单元 → P1 一键安装脚本一并交付
- users / nodes 表 created_at 索引补全 → 0002 migration 顺手加
- ~~Nodes.tsx / Users.tsx 表格搜索框~~ → 已完成（Nodes.tsx:99-122、Users.tsx:96-117）

### 5.3 不进本设计的项

- 多用户 / 多租户计费、OAuth、Telegram Bot、订阅、Kubernetes（plan 第十五节明确划出）
- 隧道多跳之间 transport 异构（用户选 A，已确认全链路统一）
- 限速 burst / per-connection（用户选 A，已确认仅 bandwidth_mbps）
- 流量配额自然月（用户选 B，已确认滚动 30 天）

## 6. 交付物与流程

每个 Phase 交付物:
- migration `.sql` 文件（如有）
- protobuf 改动（如有）
- Rust 代码改动
- 前端代码改动
- `.env.example` 同步
- 文档更新（README / docs/api.md / docs/deployment.md / plan.md 附录·实施状态加一节）
- `cargo test --workspace` 全绿
- 前端 `npm run build` 全绿
- 子代理 `superpowers:code-reviewer` 审查通过（按 P3 §4.9 内部拆分的 8 个单元，每个单元独立 review）

## 7. 时间盒（非承诺）

供节奏判断参考，不构成承诺：

- Phase 1：1 单位（小改动，主要在 install.sh / Toast）
- Phase 2：2-3 单位（DB migration + sweeper + 限速 hot path + 导入导出 UI）
- Phase 3：4-5 单位（CA + mTLS 链路 + 多跳隧道 + 三 transport + e2e 测试）
