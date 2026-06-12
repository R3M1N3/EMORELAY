# flux-parity 实施计划（对标 flux-panel 2.0.7-beta）

> **For agentic workers:** 用 superpowers:subagent-driven-development 逐单元实现。每单元：先写失败测试 → 实现 → build/test 绿 → 子代理 `superpowers:code-reviewer` 审查 → 通过后 commit → 下一单元。所有改动遵守 CLAUDE.md「强制红线」与「最少代码/外科手术」。

**Goal:** 把 `docs/flux-panel-comparison-2026-06-13.md` 列出的两个计费正确性修复 + P0/P1/P2 借鉴项全部落地，做到「flux 有的我们都有且更好」。

**Architecture:** 后端 Rust（panel-server Axum + node-agent + common proto + SQLx migrations），前端 React 19。借鉴 flux 实现但按本仓工程标准（mTLS、白名单指令、软删、迁移、测试）重写，不引入 gost/shell。

**Tech Stack:** Rust(tokio/tonic/sqlx/axum) + React19/Vite8/TS + protobuf。

来源报告：`docs/flux-panel-comparison-2026-06-13.md`

---

## 单元总览与优先级

| 阶段 | 单元 | 标题 | 主要文件域 | 状态 |
|---|---|---|---|---|
| 0 修复 | A | relay/tunnel stop 主动断存量连接 | node-agent | ☐ |
| 0 修复 | B | stats 上报失败回填，消除丢数窗口 | node-agent | ☐ |
| 1 P0 | C | 点击复制地址（列表/详情 CopyButton） | web | ☐ |
| 1 P0 | D | 节点失联强制删除逃生门 | server+agent+web | ☐ |
| 1 P0 | E | 首登强制改密 | server+web | ☐ |
| 1 P0 | F | 到期预警 toast + 去重 | web | ☐ |
| 2 P1 | G | 流量倍率 + 单/双向计费 | migration+server | ☐ |
| 2 P1 | H | 月度固定日重置模式 | migration+server(sweeper) | ☐ |
| 2 P1 | I | 配置对账自愈（心跳带规则摘要） | proto+agent+server | ☐ |
| 2 P1 | J | 协议嗅探阻断（节点级开关） | migration+proto+agent+server+web | ☐ |
| 2 P1 | K | 逐段链路诊断（白名单 Probe 指令） | proto+agent+server+web | ☐ |
| 2 P1 | L | SSE 节点实时推送 | server+web | ☐ |
| 2 P1 | M | 订阅用量 API（只读，用户已裁决做） | server | ☐ |
| 3 P2 | N | 多目标 + 负载策略（P11，最大块） | migration+proto+agent+server+web | ☐ |
| 3 P2 | O | 转发列表拖拽排序 | migration+server+web | ☐ |
| 3 P2 | P | 移动端 H5 细节包 | web | ☐ |
| 3 P2 | Q | 声明式 Settings 渲染 | web | ☐ |
| 3 P2 | R | deploy.sh CN 加速回退 | scripts | ☐ |

---

## 阶段 0：计费正确性修复（最先做）

### 单元 A：relay/tunnel stop 主动断存量连接

**问题**：`relay/tcp.rs` `stop()` 只停 listener，per-conn bridge task 是 detached spawn，注释「沿用 stop 不断存量连接的 MVP 语义」(tcp.rs:75)。禁用/删除/超配额停用的规则，存量长连接继续转发并继续计量。tunnel entry (task.rs)、tunnel hop 同样。UDP relay 已正确（stop 时 drain abort 全部 session，udp.rs:124-128），仅 TCP 侧需修。

**方案**：给 TCP listener task 内的 per-conn spawn 改为挂在 `tokio_util::sync::CancellationToken` 上：listener task 持 child token，stop 时 cancel；每个 bridge task `tokio::select!{ _ = token.cancelled() => abort, _ = bridge(...) => {} }`。tunnel entry/hop 同构。需加 `tokio-util` 依赖（features=["rt"]）。

**Files:**
- Modify: `crates/node-agent/Cargo.toml`（加 tokio-util）
- Modify: `crates/node-agent/src/relay/tcp.rs`
- Modify: `crates/node-agent/src/tunnel/task.rs`（entry_tcp_loop、start_relay_hop）

**测试**：tcp.rs 新增 `tcp_relay_stop_drops_inflight_connection`：建连后保持，写一半，调 stop，断言客户端 read 到 EOF/错误（连接被断）。

**验收**：`cargo test -p node-agent` 绿；既有 `tcp_relay_stop_is_idempotent_and_releases_port` 仍过。

### 单元 B：stats 上报失败回填

**问题**：`agent.rs:226` `drain_snapshot()` 先 swap 清零再发 gRPC，`report_rule_stats` 失败时快照丢弃（agent.rs:154 仅 warn）。node_stats 同理（sampler.drain 后失败也丢）。

**方案**：`report_stats` 失败时把 snapshot 各计数 `fetch_add` 回 counter（下个窗口补报）。`StatsCollector` 加 `restore(snapshot)` 方法。node_stats 的 sampler delta 同理需回填（SystemSampler 加 restore 或把 drain 改成「报成功才清零」）。优先做 rule_stats（计费直接相关），node_stats 评估后处理。

**Files:**
- Modify: `crates/node-agent/src/stats.rs`（加 `restore`）
- Modify: `crates/node-agent/src/agent.rs`（report_stats 失败回填）

**测试**：stats.rs 新增 `restore_adds_back_counts`：drain 后 restore，再 drain 应拿回原值。

**验收**：`cargo test -p node-agent` 绿。

---

## 阶段 1：P0 前端低成本项

### 单元 C：点击复制地址
**Files:** Create `web/src/components/CopyButton.tsx`（封装 clipboard+toast+✓ 反馈，含 IPv6 `[::1]:port` 拼接 helper）；Modify `Rules.tsx`（列表行监听地址、RuleDetail 入口地址加复制）。参考 flux `forward.tsx:749-826` 单/多地址，但我们规则单监听地址，先做单地址 + helper。
**测试**：CopyButton.test.tsx mock clipboard 断言写入值与 ✓ 态。
**验收**：vitest + build + lint 绿。

### 单元 D：强制删除逃生门
**Files:** Modify `routes/rules.rs`（DELETE 加 `?force=true`，force 时跳过 gRPC 下发结果检查、仍软删 + audit 记 `rule.force_deleted`）；`grpc/dispatcher.rs`（force 时 best-effort 下发不阻断）；`Rules.tsx`（常规删除失败弹「强制删除（跳过节点端）」二次确认，仅 admin）。
**测试**：server 集成测试 force 删除离线节点规则成功 + audit 落库；web RuleForm/Rules 交互测试。
**验收**：cargo test -p panel-server + vitest 绿。

### 单元 E：首登强制改密
**Files:** migration `0013`（users 加 `must_change_password` BOOL 默认 0，admin 种子置 1）；`routes/auth.rs`（login 响应带 `must_change_password`；改密成功清标志）；`models/user.rs`；前端 `Login.tsx`/新 `ForcePasswordChange` 流 + `auth.tsx`。
**测试**：server login 返回标志、改密清标志；前端跳转测试。
**验收**：cargo test + vitest 绿；migration 兼容 PG。

### 单元 F：到期预警 toast
**Files:** Modify `UserDashboard.tsx`（到期 ≤7 天分级 toast，localStorage key 去重，参考 flux `dashboard.tsx:73-160`）；可抽 `lib/expiry-warning.ts`。
**测试**：纯函数 `expiryWarning(expiresAt, now)` 返回 {level,message} 的单测。
**验收**：vitest 绿。

---

## 阶段 2：P1 后端

### 单元 G：流量倍率 + 单/双向计费
**方案**：migration 给 `tunnels` 加 `traffic_ratio REAL DEFAULT 1.0`、`billing_mode INT DEFAULT 2`(1=单向只计上行,2=双向)。计费聚合处（rule_stats 写入用户/隧道配额累加点）按隧道倍率与模式换算计费字节；原始 rx/tx 统计保持真实不变，仅配额扣减用换算值。admin 在隧道表单配置。
**Files:** migration `0014`；`models/tunnel.rs`；rule_stats 入库的配额累加逻辑（grpc/service.rs 或相关）；`routes/tunnels.rs`；`Tunnels.tsx` 表单。
**测试**：计费换算纯函数单测（ratio=2 双向、ratio=1 单向只计 tx）；server 集成。
**验收**：cargo test + vitest 绿。

### 单元 H：月度固定日重置
**方案**：users 加 `quota_reset_day INT NULL`(1-31, NULL=沿用滚动30天)。sweeper/user_quota 支持：设了 reset_day 则按「上次重置在本月重置日之前且今天≥重置日」触发清零（月末容错 min(day, 当月天数)）。两模式并存。
**Files:** migration `0015`；`models/user.rs`；`sweeper/user_quota.rs`；`Users.tsx` 表单。
**测试**：重置判定纯函数单测（跨月、月末 31 号容错、滚动模式不受影响）。
**验收**：cargo test + vitest 绿。

### 单元 I：配置对账自愈
**方案**：心跳（或 stats）附带 agent 当前运行规则摘要（rule_id + 配置 hash）。server 比对 DB 期望集：节点有 DB 无 → 下发 remove；DB 有节点无 → 重新下发 apply。proto Heartbeat 加 `repeated RuleDigest running_rules`。避免与既有重试队列冲突：对账只补「稳定态」差异（节流，如每 N 次心跳一次）。
**Files:** `crates/common/proto/control.proto`；`node-agent`（manager 算 digest、心跳带上）；`panel-server`（grpc 心跳处理比对 + 下发）。
**测试**：digest 计算稳定性单测；server 比对逻辑单测（孤儿/缺失各一）。
**验收**：cargo test --workspace 绿。

### 单元 J：协议嗅探阻断
**方案**：节点级开关 `nodes.block_protocols`（JSON: {http,tls,socks}）。agent relay bridge 首包 peek（不消费）匹配 HTTP 动词/TLS handshake(0x16 0x03)/SOCKS(0x04/0x05) 指纹，命中且开关开则断连计 error。仅 TCP relay。默认全关。属防滥用（防开放代理），非攻击功能。
**Files:** migration `0016`；proto Rule 加 block flags 或 node 级下发；`node-agent/src/relay/`（首包嗅探，TcpStream peek）；`models/node.rs`；`routes/nodes.rs`；`Nodes.tsx` 开关。
**测试**：嗅探纯函数单测（各协议指纹 + 正常流量放行）；relay 集成断连测试。
**验收**：cargo test + vitest 绿。

### 单元 K：逐段链路诊断
**方案**：proto 新增白名单命令 `Probe{ target_host, target_port, count }` 与响应 `ProbeResult{ reachable, avg_latency_ms, loss_pct }`（符合「只接受白名单规则操作」红线，类同 getStats）。server REST `POST /api/rules/{id}/diagnose`、`/api/tunnels/{id}/diagnose` 枚举 入口→每跳→出口→目标 各段，下发 Probe 收集，前端按段渲染。
**Files:** proto；`node-agent`（handle Probe：TCP connect 计时×count）；`panel-server`（诊断编排 REST + grpc 请求-响应）；`RuleDetail.tsx`/`TunnelDetail.tsx` 诊断 UI。
**测试**：agent probe 单测（可达/不可达）；server 编排单测；前端渲染测试。
**验收**：cargo test --workspace + vitest 绿。

### 单元 L：SSE 节点实时推送
**方案**：复用已有心跳/node_stats，不新增 WS。server `GET /api/nodes/stream`（SSE，admin only，JWT）推送节点 online/offline + 最新指标；前端 Nodes 页用 EventSource 替代 15s 轮询（回退保留轮询）。
**Files:** `routes/nodes.rs`（SSE handler + broadcast channel in state）；`state.rs`；`Nodes.tsx`（EventSource）。
**测试**：server SSE 冒烟（连接收到一帧）；前端 mock EventSource。
**验收**：cargo test + vitest 绿。

### 单元 M：订阅用量 API
**方案**：`GET /api/sub/usage`（token 或 user+pwd 鉴权），返回 `Subscription-Userinfo: upload=;download=;total=;expire=` 头 + 简单 body。只读披露用量，不分发节点配置（守住「不做订阅分发」红线）。
**Files:** `routes/`（新 `subscription.rs` 或并入 system）；`routes/mod.rs`。
**测试**：server 集成（返回正确头）。
**验收**：cargo test -p panel-server 绿。

---

## 阶段 3：P2

### 单元 N：多目标 + 负载策略（P11，最大块，谨慎评估）
**风险**：涉及 relay 数据面重写（单 target → 多 target + selector fifo/round/rand/hash），proto Rule.target 改 repeated，配额/统计/诊断全链路适配。改动面最大，单独成一组子单元，执行前在本文件细化拆分。

### 单元 O：拖拽排序
**Files:** migration 加 `forward_rules.sort_index`；server 排序持久化 + 排序 API；前端引入轻量拖拽（评估是否值得加依赖，倾向原生 HTML5 DnD 避免重依赖）。

### 单元 P：移动端 H5 包
**Files:** `web/src/index.css`（safe-area 工具类、100dvh）、宽表移动端卡片化、路由切换 scrollTo(0,0)。

### 单元 Q：声明式 Settings
**Files:** `Settings.tsx` 重构为 CONFIG_ITEMS 数组 + 统一渲染器（参考 flux config.tsx:46-102），不改后端契约。

### 单元 R：deploy.sh CN 加速
**Files:** `deploy.sh`（探测 CN IP 自动换 GitHub 加速前缀，回退原始 URL）。

---

## 红线自检
- Probe/嗅探均为白名单指令/被动指纹，不执行 shell、非攻击功能 ✓
- 订阅 API 只读用量、不分发配置，规避范围外「订阅」实质 ✓
- 所有 schema 变更走 SQLx migration、保留 PG 兼容、软删不变 ✓
- 危险写操作（强制删除）落 audit_logs ✓
- Agent 仍纯 Rust 自研，不引入 gost ✓
