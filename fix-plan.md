# EMORELAY 修复与剩余工作计划

**起始日期**: 2026-06-09
**依据**: `audit-findings.md` 的红线发现 + plan 第十二节剩余步骤
**执行规则**: 每完成一项调用 general-purpose 子代理走 `superpowers:code-reviewer` 流程,review 通过才进入下一项

> 修复进度逐项更新在文末「执行追踪」段。

---

## 一、优先级分层

- **P0** — 触红线 / 卡 MVP 验收门槛(plan 第十三节)。无 P0 项目不可视为 MVP。
- **P1** — 功能完整度(plan 列出但非验收硬门槛)。
- **P2** — 体验与可用性收尾。

---

## 二、P0 修复项

### F1 · 限速自动停规则(对应 R2)

**目标**: 让 Agent 真正消费 `traffic_limit_bytes` / `bandwidth_limit_mbps` / `expires_at`,触发后停止规则并上报。

**范围**:
- `crates/node-agent/src/manager.rs` — RuleManager 增加配额检查
- `crates/node-agent/src/relay/tcp.rs` + `relay/udp.rs` — 字节累加点暴露阈值回调,或在 StatsCollector tick 时统一检查
- `crates/node-agent/src/stats.rs` — 累计 rx+tx 与 traffic_limit 比对
- `crates/common/proto/control.proto` — 新增 RuleStopped 或复用 RuleStats batch 携带 `stopped_reason`
- `crates/panel-server/src/grpc/service.rs` — 接收 stopped 上报,UPDATE `forward_rules.enabled = 0`

**实施步骤**:
1. RuleManager 启动 listener 时把 `traffic_limit_bytes` / `expires_at` 复制进 task。
2. StatsCollector tick(已有 60s)时遍历所有规则:
   - 累计 `rx+tx > traffic_limit_bytes` → stop
   - `now > expires_at` → stop
3. stop 后通过 ControlClient 上报「停止原因」,server 端 UPDATE enabled=0 + 写 audit_logs。
4. 带宽 token bucket 先在 relay 层留一个 TODO 接口位(plan 允许 MVP 不做,只要预留)。

**验收**:
- 单测: 模拟 `traffic_limit_bytes=1MB`,推 1.5MB 后规则被 stop。
- 单测: 模拟 `expires_at` 已过,规则被 stop。
- 前端 Rules 页能看到 `enabled=false` 的自动停规则。

**预计**: 1-2 个原子单元。

---

### F2 · 节点资源采集与上报(对应 R3)

**目标**: heartbeat 不再传 0.0,Dashboard 能看到真实 CPU/MEM/LOAD,`node_stats` 表有数据。

**范围**:
- `crates/node-agent/Cargo.toml` — 加 `sysinfo` 依赖(注意精简 features,只要 CPU/MEM/LoadAvg)
- `crates/node-agent/src/main.rs:99-129` — heartbeat loop 调 sysinfo
- 新增 `crates/node-agent/src/system.rs`(可选)封装 sysinfo
- `crates/node-agent/src/control.rs` — 加 `report_node_stats` 方法
- 主循环用 tokio interval 周期上报 `NodeStatsBatch`
- `crates/panel-server/src/grpc/service.rs:233` — 替换 "MVP: log only",UPSERT 到 `node_stats`
- 同时更新 `nodes.cpu_usage` / `memory_usage` / `load_average` / `last_seen_at`
- 前端 Dashboard 增加「今日流量」「最近错误」区块(plan 第六节硬要求)

**实施步骤**:
1. node-agent 引 sysinfo,heartbeat 同步传 CPU/MEM/LOAD。
2. node-agent 新增 60s ReportNodeStats 上报,内容: bucket_at + cpu/mem/load + 节点级 rx/tx_total。
3. server 端 service.rs 写入 `node_stats` + `nodes`。
4. 前端 Dashboard 新增 endpoint 调用 `/api/nodes/{id}/stats` 拉时序数据,渲染「今日流量」(简单求和最近 24h bucket)。
5. 「最近错误」消费 `audit_logs` 的 `result='error'` 行,需要 server 先实现 `/api/system/audit-logs`(归入 F3)。

**验收**:
- Agent 跑起来后 `SELECT * FROM node_stats LIMIT 5` 有行。
- Dashboard 显示真实 CPU/MEM/LOAD。

**预计**: 1-2 个原子单元。

---

### F3 · 用户管理 + 系统设置 API 与页面(对应 R4 + R5 一半)

**目标**: 补齐 `/api/users/*` + `/api/system/*` 共 9 个 API,并落地用户管理页与系统设置页。

**范围**:
- 后端:
  - 新增 `crates/panel-server/src/routes/users.rs` + `routes/system.rs`
  - 用户 API: GET 列表 / POST 创建 / GET id / PATCH id / DELETE id(软删)
  - 系统 API: GET overview(总节点/规则/在线/流量) / GET audit-logs(分页) / GET settings / PATCH settings(保留端口/全局默认/数据保留)
  - settings 校验:保留端口必须 1-65535 整数列表
- 前端:
  - 新增 `web/src/pages/Users.tsx`
  - 新增 `web/src/pages/Settings.tsx`
  - 扩展 `web/src/lib/api.ts` 的 `users` / `system` 端点
  - App.tsx 加路由 `/users` `/settings`
  - Sidebar 加导航项

**验收**:
- 管理员能在 Web 上创建/禁用普通用户。
- 管理员能在 Web 上看到 audit_logs 列表(最近 N 条)。
- 管理员能在 Web 上调整保留端口,改完立刻生效(`reserved_ports` 校验路径会读到新值)。

**预计**: 3-4 个原子单元(后端 + 前端用户页 + 前端设置页)。

---

### F4 · Docker Compose + Caddyfile + README 部署段(对应 R6 + R7 一半)

**目标**: `docker compose up -d` 一键起 panel-server + web + sqlite volume + Caddy 反代。

**范围**:
- 根目录新增 `docker-compose.yml`
- `docker/panel-server.Dockerfile`(多阶段:builder = rust:1.78-slim + protoc;runtime = debian-slim)
- `docker/web.Dockerfile`(多阶段:node:20 build + nginx:alpine serve)
- `docker/Caddyfile.example`(80 → web:80,/api/* 转 panel-server:8080)
- 创建 `docs/` 目录
- `docs/deployment.md` 部署说明
- `docs/api.md` API 概览
- 更新根 `README.md`:一键部署段 + 开发启动段(`cargo run -p panel-server` / `cd web && npm run dev` / `sqlx migrate run`)

**验收**:
- `docker compose up -d` 起来后,浏览器访问 `localhost`,能看到登录页。
- 默认 admin 凭据来源在 README 明确(从 `.env` 引导 + 启动时打印一次)。

**预计**: 1-2 个原子单元。

---

### F5 · 基础测试套件(对应 R7 一半)

**目标**: 给 plan 第十三节测试要求一个最低限度的可跑套件。

**范围**:
- `crates/panel-server/tests/api_auth.rs` — 集成测试: login → me → 401 边界
- `crates/panel-server/tests/api_nodes.rs` — 集成测试: create → get → list → patch → delete
- `crates/panel-server/tests/api_rules.rs` — 集成测试: create → enable/disable/restart → delete
- `crates/node-agent/src/relay/tcp.rs` — 加 `#[cfg(test)]` 单测: loopback echo
- `crates/node-agent/src/relay/udp.rs` — 加单测: loopback echo
- `crates/panel-server/tests/migration.rs` — smoke: 跑 migration + 三个表 SELECT
- `web/` — 在 README/docs 标注 `npm run build` 即 smoke
- `scripts/dev-test.ps1`(或 `.sh`):一键串起 `sqlx migrate run` + `cargo test` + `npm run build`

**验收**:
- `cargo test --workspace` 全绿。
- `npm run build` 在 web/ 全绿。
- 文档说明如何跑。

**预计**: 2-3 个原子单元。

---

## 三、P1 修复项

### F6 · 规则详情页 + stats/logs 消费(对应 R5 一半)

**目标**: 落地 plan 第六节第 5 项,消费已存在的 `rules.stats` / `rules.logs` 端点。

**范围**:
- 新增 `web/src/pages/RuleDetail.tsx`,路由 `/rules/:id`
- 显示:基本配置 / 实时连接数 / 上下行流量曲线 / 最近错误日志 / 操作历史
- 简单时序图: 列表+趋势线(不引图表库,用 `<svg>` 自绘)
- 节点详情页:类似 `/nodes/:id`,消费 `nodes.stats`

**验收**:
- 点击 Rules 列表的规则名 → 跳详情 → 看到 144 个 bucket 时序。
- 点击 Nodes 列表的节点名 → 跳详情 → 看到 CPU/MEM/LOAD 时序。

**预计**: 1-2 个原子单元。

---

### F7 · gRPC TLS(对应 R1)

**目标**: gRPC 通道加 TLS,token 不再明文跑。

**范围**:
- panel-server 引 `tonic` TLS 功能(默认 feature) + `rustls-pemfile`
- agent 同上
- 配置:`PANEL_GRPC_TLS_CERT` / `PANEL_GRPC_TLS_KEY`(server 端)
- agent: `AGENT_GRPC_CA_CERT` 或 `AGENT_GRPC_INSECURE=true`(测试期)
- README 与 docs/deployment.md 加 TLS 段
- 自签证书生成脚本:`scripts/gen-dev-tls.sh`

**验收**:
- 配置 TLS 后 Agent 仍能连接、注册、心跳。
- 未配 TLS 时 server 启动失败(或显式 insecure 模式 + 警告)。

**预计**: 1 个原子单元。

---

### F8 · 普通用户路径 + audit_logs.actor_ip(对应 R10 + R11)

**目标**: rule 路由对普通用户开放只读自己的规则;audit_logs 落 actor_ip。

**范围**:
- `crates/panel-server/src/auth/extractor.rs` — 区分 admin / user 而不是只 require_admin
- `routes/rules.rs` — 普通用户 list/get 自动过滤 `user_id = claims.sub`
- 普通用户禁止 PATCH/DELETE/enable/disable/restart 别人的规则(403)
- `audit.rs` 接受 IP 参数;`auth::login` 等 handler 用 Axum `ConnectInfo<SocketAddr>` 拿对端 IP 传入

**验收**:
- 单测:role=user 调 `/api/rules` 只看到自己的。
- 单测:role=user 调别人的 rule 拿 403。
- audit_logs 的 actor_ip 列开始有值。

**预计**: 1 个原子单元。

---

## 四、P2 修复项

### F9 · 分页器 / 顶部状态栏 / 移动端 Drawer(对应 R8 + R9)

**目标**: 收尾 plan 第六节风格要求。

**范围**:
- 抽 `web/src/components/Pagination.tsx`,Nodes/Rules/Users 都用
- App.tsx 加顶部状态栏(全局健康/在线节点数/当前用户)
- Sidebar 在小屏触发 Drawer,顶部加汉堡按钮

**验收**:
- 100+ 行数据时分页可用,page_size 可切换 20/50/100。
- 手机宽度下 Sidebar 隐藏,汉堡能打开。

**预计**: 1 个原子单元。

---

## 五、执行追踪

逐项更新状态:**待开始** / **进行中** / **完成** / **阻塞**。

| ID | 标题 | 优先级 | 状态 | 完成日期 | 备注 |
|---|---|---|---|---|---|
| F1 | 限速自动停规则 | P0 | **完成** | 2026-06-09 | server 端 `auto_stop_if_exceeded` (inline + 5min sweeper);原子化 + 离线 reconcile + audit payload 标 dispatched |
| F2 | 节点资源采集与上报 | P0 | **完成** | 2026-06-09 | sysinfo 0.32 + first-drain primed 修首次 baseline bug;Dashboard 加「过去 24h 流量」 |
| F3 | 用户管理 + 系统设置 | P0 | **完成** | 2026-06-09 | 9 个 API + Users/Settings 两页 + Sidebar 入口 + self-demotion 保护 |
| F4 | Docker Compose + Caddyfile + README | P0 | **完成** | 2026-06-09 | docker-compose + 双 Dockerfile + nginx + Caddyfile + docs/ + README;curl 健康探活 + Caddyfile/web 端口对齐 |
| F5 | 基础测试套件 | P0 | **完成** | 2026-06-09 | panel-server 重构为 lib+bin;17 个集成测试(auth/nodes/rules)+ 3 个 relay 单测(TCP echo / stop 释放 / UDP echo);`cargo test --workspace` 全绿 |
| F6 | 规则详情页 + stats/logs 消费 | P1 | **完成** | 2026-06-09 | RuleDetail + NodeDetail + Sparkline 共享组件;列表 row 名字 Link 跳详情 |
| F7 | gRPC TLS | P1 | **完成** | 2026-06-09 | scripts/gen-dev-tls.sh 自签;tonic tls feature(server + agent);PANEL_GRPC_TLS_* 和 AGENT_GRPC_CA_CERT env;auto plaintext fallback + warn |
| F8 | 普通用户路径 + actor_ip | P1 | **完成** | 2026-06-09 | AuthUser.is_admin + ensure_can_touch;list_paged restrict_user_id;audit.record_with_ip + ActorIp extractor (XFF/X-Real-IP/ConnectInfo);5 个新集成测试 |
| F9 | 分页器 / 状态栏 / Drawer | P2 | **完成** | 2026-06-09 | Pagination 组件 + Nodes/Rules/Users 接入;顶部 sticky topbar + 当前用户;小屏汉堡 Drawer + 遮罩 |

---

## 六、推荐执行顺序

按依赖关系与最大收益排:

1. **F2** (节点资源上报) — 解锁 Dashboard 真实数据
2. **F1** (限速) — 触红线,Agent 内部改动孤立
3. **F3** (用户/系统 API + 页面) — 解锁 audit-logs
4. **F4** (Docker/Caddy/README) — 拿下验收第 1/11 条
5. **F5** (测试) — 主功能稳定后补测试
6. **F6** (规则/节点详情页) — F2 时序数据 + F3 audit-logs 已就绪后,水到渠成
7. **F8** (普通用户路径 + actor_ip) — 权限分层 + 审计 IP
8. **F9** (分页器 / 顶部状态栏 / Drawer) — 体验收尾
9. **F7** (gRPC TLS) — 加密层独立,放最后

## 七、剩余 F 项分解(F6 → F8 → F9 → F7)

### F6 拆分
- F6.1 — `web/src/pages/RuleDetail.tsx` 路由 `/rules/:id`,消费 `rules.stats` + `rules.logs`,显示基础信息 / 24h 时序 / 最近 audit
- F6.2 — `web/src/pages/NodeDetail.tsx` 路由 `/nodes/:id`,消费 `nodes.stats`
- F6.3 — Rules/Nodes 列表 row 上点名字进详情;build + review

### F8 拆分
- F8.1 — `auth::extractor` 增加 `AuthAny`(允许 user)与现有 `AuthUser`(admin only)并存;`routes::rules` 改 list/get 用 AuthAny + `user_id = claims.sub` 过滤;改写操作 (POST/PATCH/DELETE/enable/disable/restart) 用 AuthAny + 校验 `rule.user_id == claims.sub`
- F8.2 — `audit::record` 接受 `actor_ip` 参数,改成 wrapper(默认 None);为每个 handler 加 `ConnectInfo<SocketAddr>` extractor;login + nodes + rules + users + system 全部传 IP
- F8.3 — 集成测试覆盖:user 角色看不到他人规则、不能写他人规则;build + review

### F9 拆分
- F9.1 — `web/src/components/Pagination.tsx`(page / page_size 受控)
- F9.2 — Nodes / Rules / Users 列表接入分页器 + page_size 选择
- F9.3 — `App.tsx` 顶部状态栏(在线节点数 + 当前用户 + 登出)+ 小屏 Drawer + 汉堡;build + review

### F7 拆分
- F7.1 — `scripts/gen-dev-tls.{sh,ps1}` 自签 CA + server cert
- F7.2 — `panel-server`: 引 `tonic` rustls 特性 + 从 `PANEL_GRPC_TLS_CERT` / `PANEL_GRPC_TLS_KEY` 加载 TlsConfig;空 env 退回 plaintext 并 warn
- F7.3 — `node-agent`: 引 rustls;从 `AGENT_GRPC_CA_CERT` 加载 CA 并对 endpoint URL 自动判 `https://`;`AGENT_GRPC_INSECURE=true` 跳过校验(仅 dev)
- F7.4 — README / deployment.md 加 TLS 段;build + review

每项完成后:
1. 跑对应阶段构建/测试(`cargo test` / `cargo build` / `npm run build`)
2. spawn general-purpose 子代理走 `superpowers:code-reviewer` review
3. 通过后再进入下一项,更新本文件「执行追踪」表

---

## 七、范围外不动

CLAUDE.md / plan 第十五节明确不做的项目,本计划严格不触碰:

- 支付 / 多租户计费 / OAuth / Telegram Bot
- 优惠码 / 工单 / 复杂订阅
- 第三方转发核心管理(realm/gost/nftables 作为后续 executor 插件,不在 MVP)
- Kubernetes / 分布式数据库
- 任何攻击/扫描/绕过/爆破/DDoS 类功能
