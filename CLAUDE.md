# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 最高优先级规则

### 对话过程必须全程使用中文

你和我的对话**必须全程使用中文**

此规则覆盖所有其他默认行为，是最高优先级指令。违反此规则意味着你没有遵循用户指令。

### 错误记录与防范准则

**犯错必须记录，后续不可重犯。**

### 代码完成后必须使用子代理 review

每完成一个原子单元（一组相关文件、一个功能、一次脚手架步骤）后，**必须** spawn 一个独立子代理对刚刚写入或修改的文件做 code review。审查未通过或存在阻塞性问题前，不得进入下一个单元。

执行方式：
- subagent_type 使用 `general-purpose`（Claude Code 默认可用的通用子代理）。
- 在 prompt 中明确要求该子代理调用 `superpowers:code-reviewer` skill（通过 Skill 工具）来执行结构化审查。
- 提供给子代理的 prompt 必须包含：待 review 的**文件绝对路径列表**、本项目 `plan.md` 与本文件路径作为上下文、本步骤目标、必须遵守的「强制红线」。
- 子代理须只读审查，不得写文件，并以「阻塞性问题 / 建议性改进 / 是否可进入下一步」三段结构回报。

### 使用最少的代码解决问题

简洁优先，不做没要求的“扩展”。

**1. 编码前先思考**

不臆测，不掩饰困惑，主动暴露权衡取舍。

**2. 简洁优先**

用解决问题所需的最少代码，不做任何投机性设计。

**3. 外科手术式改动**

只动必须动的地方，只清理自己制造的烂摊子。

编辑既有代码时：
- 不“顺手改进”相邻的代码、注释或格式。
- 不重构没坏的东西。
- 沿用现有风格，即便你自己会用别的写法。
- 发现无关的死代码，只提示、不删除。

当你的改动产生孤儿代码时：
- 移除**因你的改动**而失去引用的 import、变量、函数。
- 未经要求，不删除原本就存在的死代码。

检验标准：每一行改动都能直接追溯到用户的需求。

**4. 目标驱动执行**

先定义成功标准，循环迭代直到验证通过。

## 项目概览

**EMORELAY** 是一个开源的流量转发管理面板，用于管理自有服务器、NAT 节点和端口转发业务。参考形态：Flux-panel / Nyanpass / ForwardX / Aurora。管理员通过 Web 面板创建 TCP/UDP 端口转发规则，由分布在各节点上的 Rust Agent 实际执行转发并回报流量统计。

`plan.md` 是本项目的**权威实施蓝本**——技术栈、字段定义、API 路径、表结构、protobuf 消息、安全要求、开发顺序均以它为准。本文件只补充高层架构和跨多文件的约束，不重复 plan.md 已有的细节。

## 仓库现状

**当前阶段：P3c（隧道前端 + e2e）。** MVP（plan.md 第十二节 20 步）+ P1/P2/P3a/P3b 控制面 + P3b 数据面均已交付,每个原子单元经子代理双重 review（spec 合规 + `superpowers:code-reviewer`）。各 Phase 交付记录见 `plan.md` 末尾「附录·实施状态」。

- Rust workspace: `crates/panel-server`、`crates/node-agent`、`crates/common`(protobuf + 共享类型)
- 前端: `web/`（React 19 + Vite 8 + Tailwind 4 + TS）
- 数据库: `migrations/0001_init.sql` → `0007`（基础 8 表 + 用户配额字段 + `bandwidth_profiles` + `nodes.cert_*` + `tunnels`/`tunnel_hops` + WAL + 软删 + 部分唯一索引 + PG 兼容）
- 安全: 内置 CA + 默认强制 mTLS（panel-server 启动自签 CA,gRPC 控制面强制 client cert + CRL 吊销;`PANEL_DEV_DISABLE_MTLS=1` 退 plaintext）
- 部署: `docker-compose.yml`、`docker/{panel-server,web}.Dockerfile`、`docker/Caddyfile.example`
- 文档: `README.md`、`docs/deployment.md`、`docs/api.md`、`.env.example`
- 测试: `cargo test --workspace` 全绿（panel-server 集成 + node-agent 单元 + common proto）+ web `vitest`

**已交付**：P1（Toast/防删节点/默认 TCP+UDP/Settings Agent 端点/一键安装 URL）、P2（端口自动分配/用户到期 + 滚动 30 天流量配额/`bandwidth_profiles` 限速 + Agent token bucket/规则导入导出;规则级 expires/traffic/bandwidth 已退役）、P3a（内置 CA + 默认 mTLS + 节点四件套 + 吊销/CRL + 存量迁移）、P3b 控制面（`tunnels`/`tunnel_hops` + proto `Rule.tunnel` + 隧道 REST CRUD + 删除保护扩展 + `split_tunnel_rule` 拆 hop 纯函数）、P3b 数据面（Agent `tunnel/` 模块 TCP/TLS/WSS + entry/mid/exit `TunnelTask` + 凭据签发下发 + hop 心跳 status 聚合）。

**待推进**：**P3c**（隧道前端 + 双跳/三跳 e2e）。权威计划见 `docs/superpowers/plans/2026-06-10-mvp-followups-phase-3.md`;任何新原子单元仍按本文件「代码完成后必须使用子代理 review」流程审查。

## 目标架构（来自 plan.md 第十一节）

```
EMORELAY/
  Cargo.toml              # Rust workspace 根
  crates/
    panel-server/         # Axum + Tokio + SQLx 主控
    node-agent/           # Tokio TCP/UDP relay + tonic gRPC 客户端
    common/               # 共享：protobuf 生成代码 / 模型 / auth / 错误类型
  web/                    # React + Vite + TS + Tailwind + shadcn/ui
  migrations/             # SQLx migrations（SQLite 优先，兼容 PostgreSQL）
  docker/  docs/  scripts/  README.md
```

### 三个核心进程的边界

- **`panel-server`**：唯一的鉴权入口和数据库写入方。对外提供 REST API（`/api/...`，前端消费），对内通过 gRPC 向 Agent **下发**规则变更并**接收**心跳/流量上报。所有 audit_logs、用户、节点元数据、规则配置都由它持有。
- **`node-agent`**：每台转发服务器跑一个实例。必须**用 Rust 实现**（不允许 Python/Go 替代）。负责实际的 TCP/UDP 转发与统计。即使断开主控也要继续运行已有规则——规则需落盘（`agent-state.json` 或本地 SQLite），重启后自动恢复。
- **`common`**：放 protobuf 生成代码与共享类型。任何同时被 server 和 agent 用到的东西（消息体、错误码、协议常量）都应该走这里，避免在两个 crate 里漂移。

### 通信流向

```
[web] ──REST──► [panel-server] ──gRPC/tonic──► [node-agent]
                       ▲                              │
                       └────心跳/统计/规则状态◄───────┘
```

REST 与 gRPC 是**两条独立链路**：前端绝不直连 Agent，Agent 也绝不暴露无鉴权的 gRPC。两端 RPC 必须用 mTLS 或 token 鉴权。

### Agent 内部模块（plan.md 第四节）

`RuleManager` / `TcpRelayTask` / `UdpRelayTask` / `StatsCollector` / `ControlClient` / `ConfigStore` 六个组件。改动 Agent 任何一处，先确认这六个组件的职责边界没有被破坏。

## 不可妥协的技术红线

实现时若觉得某条阻碍进度，**先停下来问用户**，不要绕过：

- **Agent 必须是 Rust 自研** TCP/UDP relay（含 P3 多跳隧道 transport）。realm/gost/nftables 只能作为后续 executor 插件存在，当前不依赖。
- **Agent 不执行任意 shell**。Agent 只接受白名单内的规则操作（apply/remove/enable/disable/restart/getStats）。后端绝不能把用户输入拼接成 shell 命令。
- **保留端口默认禁止**：22、80、443、3306、5432 等端口不能被规则监听，除非管理员显式配置白名单/黑名单。端口范围必须校验 1-65535。
- **凭据存储**：用户密码 Argon2 或 bcrypt 哈希；JWT secret 从环境变量读取；Agent token 数据库内只存哈希，不存明文。
- **数据库**：SQLite 开启 WAL；SQLx migrations 是唯一的 schema 演进通道；schema 必须保留向 PostgreSQL 迁移的兼容性（避免 SQLite 专有类型）；删除规则用软删除（`deleted_at`）；node_id / user_id / listen_port / created_at 等查询字段必须有索引。
- **流量统计按时间聚合**，不能逐请求无限写入，否则会把库写爆。
- **审计日志**：所有危险 API（用户/节点/规则的写操作）必须落 `audit_logs`。
- **错误与日志**：Rust 端用 `thiserror` 或 `anyhow` 处理错误，用 `tracing` 做结构化日志；API 返回统一 JSON 格式。
- **范围外功能**：暂不做支付、多租户计费、OAuth、Telegram Bot、优惠码、工单、订阅、Kubernetes、分布式数据库——但代码结构要给这些预留扩展位。
- **禁止安全攻击类功能**：扫描、绕过、爆破、DDoS 相关一律不实现。

## 命令出口

- Rust workspace（根目录运行）：
  - `cargo build` / `cargo test --workspace` / `cargo test -p panel-server --test <name>`
  - `cargo run -p panel-server` / `cargo run -p node-agent`
- 前端（`web/` 目录）：`npm run dev` / `npm run build`
- 数据库迁移：`sqlx migrate run`（panel-server 启动时会自动跑一次，开发期可手动）
- 前端测试：`cd web && npm test`（vitest run）
- 一键启动：根 `docker-compose.yml` → `docker compose up -d`
- Dev mock 数据：`python scripts/seed-dev.py`（前提 panel-server 跑着 + 空库）
- gRPC mTLS：P3a 起 panel-server 启动自动生成内置 CA（`${PANEL_DATA_DIR}/tls/`）并默认强制 mTLS;dev 走 plaintext 用 `PANEL_DEV_DISABLE_MTLS=1`（旧 `scripts/gen-dev-tls.sh` 已弃用）

每完成一个改动单元都要跑对应阶段的构建/测试验证并修复错误，再 spawn 子代理 review。

## 实施导航

MVP + P1/P2/P3a/P3b 控制面已交付,当前推进 **P3b 数据面**。权威计划是 `docs/superpowers/plans/2026-06-10-mvp-followups-phase-3.md`（含 P3b 数据面 + P3c 概要,待按需展开为 TDD plan）。推进任何 Task 时：
1. 改动前先看 plan.md 附录·实施状态（含各 Phase 交付记录）与 phase-3.md,避免重复劳动。
2. 跑对应阶段的 build / test 验证（前端 `npm run build`、Rust `cargo test --workspace`、迁移 `sqlx migrate run`）。
3. spawn 子代理走 `superpowers:code-reviewer` 流程。
4. review 通过后再进入下一步。
