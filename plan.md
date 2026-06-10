你是一个资深全栈工程师，现在从零开发一个开源的流量转发管理面板，项目名暂定为 `EMORELAY`。这是一个用于自有服务器、NAT 转发节点、端口转发业务的管理系统。请直接开始实现，不要只写方案。目标是先做出可以运行的 MVP，然后再逐步完善。

技术栈必须固定如下：

* 前端：React + Vite + TypeScript
* 前端 UI：Tailwind CSS + shadcn/ui 或同等级组件封装
* 后端主控：Rust
* 后端 Web 框架：Axum
* 异步运行时：Tokio
* 数据库：SQLite 优先，结构要兼容未来迁移到 PostgreSQL
* 数据库访问：SQLx
* 节点 Agent：Rust
* 主控与 Agent 通信：优先使用 gRPC / tonic，支持 mTLS 或 token 鉴权
* 部署：systemd 和对应的命令脚本
* 反向代理：默认给出 Caddy 配置
* 项目必须包含 README、部署文档、API 文档、数据库迁移、示例配置

项目整体分为三个核心部分：

1. `panel-web`
   React 前端管理面板。

2. `panel-server`
   Rust 主控后端，负责用户、节点、转发规则、流量统计、审计日志、Agent 调度。

3. `node-agent`
   Rust 节点代理，部署在每台转发服务器上，负责实际创建、启动、停止、热重载 TCP/UDP 转发任务，并定时上报节点状态和流量数据。

核心业务目标：

做一个类似 Flux-panel、Nyanpass、ForwardX、Aurora 风格的转发面板 如果你不知道这些面板是什么 请使用websearch。它要能管理多台转发节点，让管理员在 Web 面板上创建端口转发规则，例如：

* 本机监听 `0.0.0.0:20000`
* 转发到远端 `1.2.3.4:443`
* 协议为 TCP、UDP 或 TCP+UDP
* 绑定某个用户
* 限制到期时间
* 限制总流量
* 限制带宽
* 支持启用、禁用、删除、重启
* 支持查看实时连接数、上下行流量、错误日志

第一阶段 MVP 必须完成：

一、认证系统

实现登录系统，先只做单管理员或多管理员均可，但必须有完整的数据模型。

要求：

* 用户表 `users`
* 密码必须使用 Argon2 或 bcrypt 哈希
* 登录后使用 JWT 或安全 Session
* 支持角色字段：`admin`、`user`
* MVP 阶段可以只开放 admin 界面
* 后续预留普通用户自助查看转发规则和流量的能力

二、节点管理

实现节点模型 `nodes`。

字段至少包括：

* id
* name
* region
* public_ip
* grpc_endpoint
* agent_token_hash 或 mTLS identity
* status：online / offline / unknown
* last_seen_at
* cpu_usage
* memory_usage
* load_average
* rx_bytes_total
* tx_bytes_total
* created_at
* updated_at

后端提供节点 CRUD API。

Agent 启动后主动向主控注册或心跳。主控可以看到节点在线状态。

三、转发规则管理

实现转发规则模型 `forward_rules`。

字段至少包括：

* id
* user_id
* node_id
* name
* protocol：tcp / udp / tcp_udp
* listen_ip
* listen_port
* target_host
* target_port
* enabled
* expires_at
* traffic_limit_bytes
* bandwidth_limit_mbps
* rx_bytes
* tx_bytes
* connection_count
* created_at
* updated_at

后端提供完整 CRUD：

* 创建规则
* 修改规则
* 删除规则
* 启用规则
* 禁用规则
* 重启规则
* 查询规则列表
* 查询单条规则详情
* 查询规则流量统计

规则创建后，主控通过 gRPC 下发到对应节点 Agent。Agent 必须在本地启动对应的 TCP/UDP relay 任务。

四、Rust Agent 转发实现

Agent 必须用 Rust 实现，不允许用 Python/Go 代替。

第一版转发实现要求：

TCP：

* 使用 Tokio `TcpListener` 监听本地端口
* 每个连接连接到目标地址
* 使用异步双向复制转发流量
* 记录每个方向传输字节数
* 记录连接数、错误数
* 支持停止任务
* 支持重启任务
* 支持热更新规则

UDP：

* 使用 Tokio `UdpSocket`
* 实现基本 UDP NAT 映射表
* 按客户端地址维护 session
* 设置 session 超时时间
* 统计上下行字节
* 支持停止、重启、热更新

Agent 内部设计：

* 一个 `RuleManager`
* 一个 `TcpRelayTask`
* 一个 `UdpRelayTask`
* 一个 `StatsCollector`
* 一个 `ControlClient`
* 一个 `ConfigStore`

Agent 必须能在没有主控连接时保持已有规则继续运行。规则应落盘保存，例如 `agent-state.json` 或 SQLite。本地规则恢复要在 Agent 重启后自动执行。

五、主控与 Agent 通信

定义 protobuf 文件，至少包含：

* AgentRegister
* AgentHeartbeat
* ApplyRule
* RemoveRule
* EnableRule
* DisableRule
* RestartRule
* GetRuleStats
* StreamNodeStats
* CommandResponse

通信要求：

* Agent 定时向 Server 上报心跳
* Agent 定时上报节点资源状态
* Agent 定时上报每条规则的 rx/tx/connection/error
* Server 可以主动向 Agent 下发规则变更
* 所有 RPC 必须有鉴权
* 不允许 Agent 执行任意 shell 命令
* Agent 只允许执行白名单内的规则操作

六、前端页面

实现以下页面：

1. 登录页

   * 简洁暗色毛玻璃风格
   * 输入用户名和密码
   * 登录失败提示

2. Dashboard

   * 总节点数
   * 在线节点数
   * 总规则数
   * 总上下行流量
   * 今日流量
   * 最近错误
   * 节点在线状态概览

3. 节点管理页

   * 节点列表
   * 新增节点
   * 编辑节点
   * 删除节点
   * 查看节点详情
   * 查看 Agent 心跳
   * 查看 CPU / 内存 / 负载 / 流量

4. 转发规则页

   * 规则列表
   * 创建规则
   * 编辑规则
   * 删除规则
   * 启用/禁用
   * 重启规则
   * 筛选协议
   * 筛选节点
   * 搜索端口、目标地址、规则名称

5. 规则详情页

   * 基本配置
   * 实时连接数
   * 上下行流量
   * 最近错误日志
   * 操作历史

6. 用户管理页

   * MVP 可以只做管理员可见
   * 预留普通用户模型
   * 用户流量统计
   * 用户规则列表

7. 系统设置页

   * JWT 密钥状态提示
   * Agent 鉴权方式
   * 全局默认流量限制
   * 全局默认带宽限制
   * 数据保留周期

前端风格要求：

* 深色现代云服务商控制台风格
* 圆角液态玻璃效果
* 左侧 Sidebar
* 顶部状态栏
* 卡片式数据展示
* 表格支持搜索、筛选、分页
* 所有危险操作必须二次确认
* API 错误要清楚展示
* 加载状态、空状态、错误状态都要处理
* 移动端能基本使用，但优先桌面端体验

七、后端 API

后端需要实现 REST API，路径建议如下：

认证：

* `POST /api/auth/login`
* `POST /api/auth/logout`
* `GET /api/auth/me`

节点：

* `GET /api/nodes`
* `POST /api/nodes`
* `GET /api/nodes/:id`
* `PATCH /api/nodes/:id`
* `DELETE /api/nodes/:id`
* `GET /api/nodes/:id/stats`

转发规则：

* `GET /api/rules`
* `POST /api/rules`
* `GET /api/rules/:id`
* `PATCH /api/rules/:id`
* `DELETE /api/rules/:id`
* `POST /api/rules/:id/enable`
* `POST /api/rules/:id/disable`
* `POST /api/rules/:id/restart`
* `GET /api/rules/:id/stats`
* `GET /api/rules/:id/logs`

用户：

* `GET /api/users`
* `POST /api/users`
* `GET /api/users/:id`
* `PATCH /api/users/:id`
* `DELETE /api/users/:id`

系统：

* `GET /api/system/overview`
* `GET /api/system/audit-logs`
* `GET /api/system/settings`
* `PATCH /api/system/settings`

八、数据库设计

使用 SQLx migrations。必须写出初始 migration。

表至少包括：

* users
* nodes
* forward_rules
* rule_stats
* node_stats
* audit_logs
* system_settings
* agent_sessions

要求：

* 所有表有 created_at / updated_at
* 重要操作写入 audit_logs
* 删除规则时优先软删除，可加 deleted_at
* 流量统计按时间聚合，避免无限写爆数据库
* SQLite 下要开启 WAL
* 关键字段加索引，例如 node_id、user_id、listen_port、created_at

九、安全要求

必须把安全作为第一优先级之一。

要求：

* 不允许后端把用户输入直接拼接成 shell 命令
* Agent 不允许开放无鉴权 gRPC
* Agent token 不能明文存数据库
* 密码必须哈希
* JWT secret 必须来自环境变量
* CORS 默认只允许配置中的前端域名
* 所有端口号必须校验范围 1-65535
* listen_ip、target_host、target_port 必须校验
* 防止同一节点同协议同端口重复绑定
* 防止普通用户操作别人的规则
* 所有危险 API 记录审计日志
* 默认禁止创建监听 22、80、443、3306、5432 等保留端口，管理员可配置白名单/黑名单
* 不要实现任何攻击、扫描、绕过、爆破、DDoS 相关功能

十、限速和限流

MVP 可以先做统计，不强制限速。代码结构必须预留限速层。

要求：

* 每条规则可以配置 `traffic_limit_bytes`
* 每条规则可以配置 `bandwidth_limit_mbps`
* 超过总流量限制后，Agent 自动停止该规则，并上报状态
* 带宽限制后续可用 token bucket 实现
* 统计必须区分 rx 和 tx
* 统计必须能在前端显示为 B、KB、MB、GB、TB

十一、项目结构建议

使用 Rust workspace：

```text
rust-forward-panel/
  Cargo.toml
  crates/
    panel-server/
    node-agent/
    common/
      proto/
      models/
      auth/
  web/
    package.json
    src/
  migrations/
  docker/
  docs/
  scripts/
  README.md
```

`common` 用于共享 protobuf 生成代码、数据类型、错误类型、协议常量。

十二、开发顺序

请按以下顺序实现：

1. 初始化 monorepo
2. 初始化 Rust workspace
3. 初始化 React + Vite + TypeScript 前端
4. 创建数据库 schema 和 migrations
5. 实现 panel-server 基础 HTTP 服务
6. 实现登录认证
7. 实现节点 CRUD
8. 实现规则 CRUD
9. 定义 protobuf
10. 实现 node-agent 基础 gRPC 客户端/服务端通信
11. 实现 TCP relay
12. 实现 UDP relay
13. 实现 Agent 规则热加载
14. 实现统计上报
15. 实现前端 Dashboard
16. 实现节点页面
17. 实现转发规则页面
18. 实现 Docker Compose
19. 实现 README 和部署文档
20. 写基本测试和运行说明

十三、测试要求

至少提供：

* Rust 单元测试
* API 集成测试
* TCP 转发本地测试
* UDP 转发本地测试
* 数据库 migration 测试
* 前端基础构建测试
* Docker Compose 启动测试说明

验收标准：

项目最终必须能做到：

1. `docker compose up -d` 启动 panel-server、web、sqlite volume。
2. 管理员能登录 Web 面板。
3. 能新增一个节点。
4. Agent 能连接主控并显示在线。
5. 能创建一条 TCP 转发规则。
6. Agent 实际监听对应端口。
7. TCP 流量能成功转发到目标地址。
8. 前端能看到规则流量统计。
9. 能禁用/启用/删除规则。
10. Agent 重启后能恢复已有规则。
11. README 里有清晰的一键部署和开发启动步骤。

十四、代码质量要求

* Rust 代码必须使用 `thiserror` 或 `anyhow` 处理错误
* 使用 `tracing` 做结构化日志
* API 返回统一 JSON 格式
* 前端使用类型安全 API client
* 不要写死密钥
* 不要把配置散落在代码里
* 使用 `.env.example`
* 所有 TODO 必须说明原因
* 遇到不确定的地方，先选择简单可靠方案，并在文档中标记后续可扩展点

十五、先交付 MVP，不要一开始做太复杂

暂时不要实现：

* 支付系统
* 多租户复杂计费
* OAuth
* Telegram Bot
* 优惠码
* 工单系统
* 复杂订阅系统
* 第三方转发核心管理
* Kubernetes
* 分布式数据库

但代码结构要预留未来扩展这些能力的位置。
TCP/UDP 转发必须优先自研 Rust Agent 实现，外部 realm/gost/nftables 只能作为后续 executor 插件，不要 MVP 阶段依赖它们

现在请直接开始创建项目文件。先输出项目结构，然后开始实现核心代码。每完成一个阶段，都运行相应测试或构建命令，发现错误要自己修复。不要只给解释，要尽可能产出可运行代码。

---

## 附录·实施状态（2026-06-09）

> 本节为实施快照，反映代码当前状态；蓝本（第一至十五节）内容不变。
> 审计原始发现见 `audit-findings.md`（R1-R12 红线快照）。

### 第十二节 20 步开发顺序

20/20 全部完成。仓库结构、Rust workspace、React+Vite+TS、SQLx migrations、panel-server HTTP、Argon2+JWT 登录、节点/规则 CRUD、protobuf 9 消息、node-agent gRPC client、TCP relay、UDP relay、规则热加载与落盘、统计上报、Dashboard、节点页、规则页、Docker Compose、README/部署文档、测试套件均已交付。

### 第十三节验收 11 条

11/11 通过：

1. `docker compose up -d` ✅ 起 panel-server + web + sqlite volume + Caddy 可选反代。
2. 管理员登录 ✅
3. 新增节点 ✅
4. **Agent 连接主控并显示在线** ✅ `crates/panel-server/tests/agent_e2e.rs` 协议级 e2e 验证 register → SubscribeCommands → ReportStats 整链。
5. 创建 TCP 规则 ✅
6. Agent 实际监听端口 ✅ 由 `crates/node-agent/src/relay/tcp.rs` 实现与 unit test 覆盖。
7. TCP 流量转发 ✅ 同上。
8. 前端看到规则流量统计 ✅ Dashboard / Rules / RuleDetail 三页时序图齐。
9. 禁用/启用/删除 ✅
10. Agent 重启恢复 ✅ `agent-state.json` 启动时 load → apply → 再连主控。
11. README 一键部署 + 开发启动 ✅

### 子代理 review 流程（CLAUDE.md 强制要求）

整个 MVP 期间共触发 14 次 `superpowers:code-reviewer` 审查：9 次对应 fix-plan F1-F9，5 次对应 P1-1/P1-2/P1-3/P1-4/P1-5，全部 YES 通过。reviewer 共发现并修复 4 个事实性问题（首次 drain baseline、healthcheck 工具缺失、Caddyfile 端口冲突、TestApp grpc_tls 字段缺失），无任何放行的阻塞性问题。

### 红线与亮点（超出蓝本要求）

- 登录 timing oracle 防御：`dummy_hash` 预热 + 未知用户也跑一次 verify
- gRPC register 安全：`unknown_node` 与 `bad_token` 返回同一 PermissionDenied 消息（防枚举 node_id，由 `agent_e2e_bad_token_rejected_with_same_error` 测试守护）
- `auto_stop_if_exceeded` 原子 `UPDATE WHERE enabled=1` 防并发重复触发 + `spawn_expiry_sweeper` 周期兜底
- `SubscribeCommands` 重连立即 `list_active_for_node` 重放，覆盖断网期间 CRUD
- 最后 admin 保护：自删 / 自降级 / 删最后一个 admin 全部拒绝
- `tcp_udp` 与 `tcp/udp` 应用层互斥预检（DB UNIQUE 索引按字符串精确比较，应用层补 conflict 校验，由 `api_rules_protocol_conflict.rs` 10 个测试守护）
- Agent token 创建时一次性返回明文，DB 只存 SHA-256；session_token 同款

### 剩余非阻塞工作（P2 清单）

- ~~`relay/traits.rs::QuotaGuard` trait 占位 + `bridge()` hot path `TODO(bandwidth)` 锚点~~（Phase 2 token bucket 取代）
- ~~`grpc/dispatcher.rs` SubscribeCommands stream 终止时 Drop guard 清理 dead sender~~ → 已于 d519172 交付（`grpc/service.rs` GuardedStream）
- ~~gRPC server 端 mTLS 客户端证书校验（`ClientCertVerifier`），与 Agent 已支持的 `ClientTlsConfig` 形成双向认证~~ → 已于 d519172 交付（`PANEL_GRPC_TLS_CLIENT_CA`）；P3a 已用内置 CA 默认强制 mTLS 取代该手动配置路径（`PANEL_GRPC_TLS_*` 弃用，见下 Phase 3a）
- ~~`.env.example` 补 `AGENT_STATS_INTERVAL_SECS` / `PANEL_EXPIRY_SWEEP_SECS`~~（Phase 2 已完成;后者退役,换为 PANEL_USER_EXPIRY/QUOTA_SWEEP_SECS）
- ~~前端引入 vitest + 关键页面渲染 smoke~~ → 已于 d519172 交付
- ~~UDP session 超时测试~~ → 已于 d519172 交付（`udp_session_expires_after_timeout`）
- ~~独立 `emorelay-agent.service` systemd 单元~~ → 已于 d519172 交付（`scripts/emorelay-agent.service`）
- ~~`users` / `nodes` 表 `created_at` 索引补全~~ → 已于 d519172 交付（migration 0002）
- ~~`Nodes.tsx` / `Users.tsx` 表格搜索框~~ → 已于 d519172 交付

### Phase 1（2026-06-10 启动）

(7) 全局 Toast、(8) 防删节点、(11) 创建规则默认 TCP+UDP、(2) Settings 加 Agent 上报端点、(1) 一键安装 URL —— 全部交付。

- Spec: `docs/superpowers/specs/2026-06-10-mvp-followups-design.md` §2
- Plan: `docs/superpowers/plans/2026-06-10-mvp-followups-phase-1.md`
- 12 个 Task 全部 spec ✅ + code quality ✅（subagent-driven flow）
- 测试: `cargo test --workspace` 47 PASS（含 3 个 nodes-delete-protection + 5 个 install）；`web` vitest 15 PASS
- 关键 commit 区间: 基线 `18bb54f` → P1 收尾（待 phase-end commit）
- 后续 P2/P3 见同名 plan-2 / plan-3 文件

### Phase 2（2026-06-10 启动,同日交付）

(6) 端口自动分配、(10) 到期搬用户、(13) 流量配额滚动 30 天、(9) 限速独立路由 + Agent token bucket、(12) 规则导入导出 —— 全部交付。

- Spec: `docs/superpowers/specs/2026-06-10-mvp-followups-design.md` §3
- Plan: `docs/superpowers/plans/2026-06-10-mvp-followups-phase-2.md`
- 12 个 Task 全部 spec ✅ + code quality ✅（subagent-driven flow,每 Task 双重审查）
- 规则级 expires/traffic/bandwidth 全链路退役(migration 0004 + proto reserved 8-10)
- 测试: `cargo test --workspace` 全绿(新增 bandwidth_profiles / port_alloc / rules_io / user_quota_sweeper / token_bucket);web vitest 全绿
- 注意:Phase 2 的 commit 区间须整体部署(中间 commit 存在「规则级执法已删、用户级 sweeper 未上」的过渡态);限速变更对存量 TCP 连接延迟生效(新连接即时)

### Phase 3a（2026-06-10 启动,同日交付）

内置 CA + 默认强制 mTLS + 节点四件套 + install.sh 凭据落盘 + 吊销/CRL + 存量迁移 —— 全部交付。

- Spec: `docs/superpowers/specs/2026-06-10-mvp-followups-design.md` §4.4
- Plan: `docs/superpowers/plans/2026-06-10-mvp-followups-phase-3.md`（P3a 段,7 Task）
- 7 个 Task 全部 spec ✅ + code quality ✅（subagent-driven,每 Task 双重审查 + openssl 链验证）
- migration 0005(nodes.cert_serial/fingerprint);新依赖 rcgen 0.13。
- gRPC 默认强制 mTLS(内置 CA);PANEL_DEV_DISABLE_MTLS=1 退 plaintext。
- 创建节点返回四件套(token+CA+cert+key)一次性;DB 只存 serial/fingerprint。
- 吊销:POST /api/nodes/:id/revoke-credentials → CRL(原子写)+ 重签;register 拒已吊销证书。
- **⚠️ 升级 P3a = fleet-wide Agent 重装**:存量节点须逐个「轮换凭据」重装。
- 已知留置:register 拒吊销的真链路 + 「裸连接(无 client cert)被拒」负向断言留 P3c e2e;CRL 损坏当前 fail-open(loud error),强场景可升级 boot-blocking。
- P3b(多跳隧道)/P3c(隧道前端 + e2e)待展开。

