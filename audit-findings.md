# EMORELAY 全量审计发现

**审计日期**: 2026-06-09
**审计方式**: 4 个并行 general-purpose 子代理逐节比对 `plan.md` 与代码
**结论**: 主干完成约 70%,MVP 验收(plan 第十三节 11 条)只有 3 条确凿满足

> 本文件是只读快照,记录审计时刻的状态。修复跟踪见 `fix-plan.md`。

---

## 一、plan 第十二节 20 步评级

| 步 | 主题 | 状态 | 关键发现 |
|---|---|---|---|
| 1 | monorepo | ⚠ | 缺 `docker/` 和 `docs/` 目录 |
| 2 | Rust workspace | ✅ | 3 crate + workspace 版本管理 |
| 3 | React 前端脚手架 | ✅ | React 19 / Vite 8 / Tailwind 4(无 shadcn,自写 `lib/ui.tsx` 替代) |
| 4 | DB schema/migrations | ✅ | 8 表全覆盖,WAL、`deleted_at`、关键索引、PG 兼容标注 |
| 5 | panel-server 基础 HTTP | ✅ | Axum + `tracing` + `dotenvy` + `/api/health` |
| 6 | 登录认证 | ✅ | Argon2 + JWT(`PANEL_JWT_SECRET` 强制环境变量) + role |
| 7 | 节点 CRUD | ✅ | 6 个路径全在,字段齐(含 `port_pool_min/max` 扩展) |
| 8 | 规则 CRUD | ✅ | 10 个路径全在,校验完整(协议/IP/端口/保留端口/防重) |
| 9 | protobuf | ✅ | `control.proto` 服务齐,plan 命名→proto 命名映射文档化 |
| 10 | Agent gRPC 通信 | ⚠ | 心跳/规则统计/下发到位,**节点资源 CPU/MEM/LOAD 永远传 0.0**,`ReportNodeStats` 已声明但 Agent 零调用,`node_stats` 表是死表 |
| 11 | TCP relay | ✅ | `tokio::TcpListener` + 手写双向 copy + 字节/连接/错误计数 + 停止/热更新 |
| 12 | UDP relay | ✅ | NAT 映射表 + 120s session 超时 + 30s sweep + 双向计数 |
| 13 | Agent 规则热加载 | ✅ | `store.rs` 原子写 JSON,启动先 load → apply → 再连主控 |
| 14 | 统计上报 | ✅ | 60s bucket,server 端 UPSERT 同 bucket,`drain_snapshot` 用 swap 处理 reset 竞态 |
| 15 | 前端 Dashboard | ⚠ | **「今日流量」「最近错误」缺**(plan 第六节硬性要求) |
| 16 | 节点页 | ⚠ | 详情页缺(无 `/nodes/:id`,不消费 `nodes.stats`) |
| 17 | 规则页 | ⚠ | **分页缺**,写死 `page_size:100` |
| 18 | Docker Compose | ❌ | 文件不存在,`docker/` 目录不存在 |
| 19 | README/部署/Caddy | ❌ | README 22 行明文写"尚无对外可用的命令出口";无 Caddyfile;无 `docs/` |
| 20 | 测试 | ❌ | crates 下 `#[cfg(test)]` 零命中,无 `tests/` 目录,无 migration 测试 |

---

## 二、红线违反与硬偏差(必修)

编号 = 严重度 + 修复 ID(对应 `fix-plan.md`)。

### R1 · gRPC 通道无 TLS — 严重 ★★★
- 证据: `crates/common/proto/control.proto:11` 自标"生产环境必须配 TLS";`Cargo.lock` 无 `rustls`/`openssl`;server 与 agent 均未调 `*_tls_config`。
- 后果: token 与 session_token 明文跑通道,MITM 即可窃取。
- 关联红线: CLAUDE.md「Agent token 数据库内只存哈希,不存明文」精神延伸;plan 第五节「必须 mTLS 或 token 鉴权」严格读法。
- 对应修复: **F7**

### R2 · 流量超限自动停规则未实现 — 严重 ★★★
- 证据: `traffic_limit_bytes` / `bandwidth_limit_mbps` 字段全链路存在,但 `crates/node-agent/src/manager.rs` 与 `relay/*` 不消费;`expires_at` 同样只持久化不读取。
- 后果: plan 第十节「超过总流量限制后,Agent 自动停止该规则,并上报状态」未达成。
- 对应修复: **F1**

### R3 · 节点资源上报缺失 — 严重 ★★★
- 证据: `crates/node-agent/src/main.rs:127` heartbeat 永远传 `0.0, 0.0, 0.0`;无 `sysinfo` 依赖;server 端 `grpc/service.rs:233` 自标 "MVP: log only";`node_stats` 表是空死表。
- 后果: plan 第二节 nodes 表 CPU/MEM/LOAD 字段、第五节 `StreamNodeStats`、Dashboard「今日流量」「最近错误」均无数据来源。
- 对应修复: **F2**

### R4 · 第七节 `/api/users/*` 与 `/api/system/*` 全部缺失 — 严重 ★★★
- 证据: `routes/mod.rs:1-35` 无 users/system 模块;grep `/api/users` / `/api/system` 在 routes 下零命中。共缺 9 个 API。
- 后果: 管理员无法在线管理用户、查 audit_logs、调整保留端口黑名单(只能改 SQL)。
- 对应修复: **F3**

### R5 · 第六节 7 页只交付 4 页 — 中等 ★★
- 证据: App.tsx 路由仅 `/` `/nodes` `/rules`;无 `/rules/:id` 详情、无 Users.tsx、无 Settings.tsx。
- 对应修复: **F3**(用户/设置页) + **F6**(规则详情页)

### R6 · 第三节「默认给出 Caddy 配置」未满足 — 中等 ★★
- 证据: 仓库内无 `Caddyfile` / `Caddyfile.example`。
- 对应修复: **F4**

### R7 · plan 第十二节 18-20 整体跳过 — 严重 ★★★
- 证据: 无 `docker-compose.yml` / 无 `docker/` / 无 `docs/` / 无任何 `#[cfg(test)]` / README 22 行无命令。
- 后果: 验收第 1 条 (`docker compose up -d`) 与第 11 条 (README 一键部署) 直接 ❌。
- 对应修复: **F4** + **F5**

### R8 · 第六节「表格支持分页」未实现 — 轻 ★
- 证据: `Nodes.tsx` / `Rules.tsx` 写死 `page_size:100`,无分页器。
- 对应修复: **F9**

### R9 · 第六节「顶部状态栏」未实现 — 轻 ★
- 证据: App.tsx 仅有 Sidebar,无 topbar。
- 对应修复: **F9**

### R10 · 普通用户角色路径被一刀切 — 中等 ★★
- 证据: 所有 rule 路由强制 `auth.require_admin()`(`rules.rs:162/215/...`),`Rule.user_id` 从未基于 `claims.sub` 过滤。
- 后果: plan 第六节「预留普通用户自助」与第九节「防止普通用户操作别人的规则」均未落地。
- 对应修复: **F8**

### R11 · `audit_logs.actor_ip` 始终 NULL — 轻 ★
- 证据: `audit.rs:20` 自带 TODO,未接 Axum `ConnectInfo`。
- 对应修复: **F8**

### R12 · CLAUDE.md「每完成一个原子单元必须 spawn 子代理 review」 18-20 步无机会触发
- 证据: 这三步根本没动,review 流程自然缺。
- 修复方式: F4/F5 落地时严格走 review。

---

## 三、轻微偏差(可后续顺手)

- README 22 行,缺命令出口说明 — 由 **F4** 处理
- Dashboard 多了「总连接数」卡片(plan 未要求,合理添加)
- `users` 表 + `User` 模型已就绪,只差 API 暴露 — **F3** 自然带上
- 移动端 Sidebar `hidden md:flex` 后无 Drawer 替代 — **F9**
- `panel-server/Cargo.toml` 里 `tonic`/`prost` 没走 `workspace.dependencies`,有版本漂移风险
- `grpc/dispatcher.rs:39-41` TODO:SubscribeCommands 终止时无 unsubscribe,残留 sender(低危)
- `expires_at` 仅校验非空字符串(`rules.rs:246-250`),未做 ISO8601 解析

---

## 四、亮点(超出 plan 的良性扩展)

- DB schema 顶部明确 SQLite→PG 迁移路径注释(`migrations/0001_init.sql:1-21`)
- 节点字段扩展 `port_pool_min/max`,规则创建会校验端口落入节点池
- agent token 创建时一次性返回,DB 只存 SHA-256 哈希
- `auth/password.rs` 有 `dummy_hash` timing oracle 防御
- TCP relay 不用 `copy_bidirectional` 而手写,目的是每方向末尾原子累加字节
- UDP relay sweep 任务清理过期 session 并 abort 反向 task
- 统计上报 `drain_snapshot` 用 swap 处理 reset 竞态

---

## 五、第十三节验收 11 条命中

| # | 验收项 | 命中 | 备注 |
|---|---|---|---|
| 1 | `docker compose up -d` | ❌ | 无 compose,F4 修 |
| 2 | 管理员登录 Web 面板 | ⚠ | 代码就绪,但需手工跑 cargo + npm |
| 3 | 新增节点 | ✅ | Nodes 页 + API 闭环 |
| 4 | Agent 连接主控并显示在线 | ⚠ | 代码就绪,缺端到端验证脚本 |
| 5 | 创建 TCP 转发规则 | ✅ | Rules 页 + API 闭环 |
| 6 | Agent 监听对应端口 | ⚠ | 代码就绪,本审计不跑 |
| 7 | TCP 流量转发 | ⚠ | 代码就绪,本审计不跑 |
| 8 | 前端看到规则流量统计 | ⚠ | 行内累计 OK,**时序图缺**(F6) |
| 9 | 禁用/启用/删除 | ✅ | Rules 页闭环 |
| 10 | Agent 重启恢复 | ⚠ | `agent-state.json` 与 `store.rs` OK,缺端到端验证 |
| 11 | README 一键部署 + 开发启动 | ❌ | F4 修 |

确凿命中: **3 条**(3/5/9);代码就绪待端到端: 5 条;直接缺失: 3 条。

---

## 六、四份子代理报告来源

本审计由以下 4 个 general-purpose 子代理独立完成:

- Agent A — plan 步骤 1-5(脚手架/workspace/前端/DB schema/HTTP 基础)
- Agent B — plan 步骤 6-8 + 第 7/9 节(认证/CRUD/API 完整性/安全)
- Agent C — plan 步骤 9-14 + 第 4/9/10 节(protobuf/gRPC/relay/热加载/统计/Agent 内部 6 组件/限速/Agent 安全)
- Agent D — plan 步骤 15-20 + 第 6/13 节(前端三页/Docker/README/测试/验收 11 条)

每个子代理只读审计,不修改任何代码。
