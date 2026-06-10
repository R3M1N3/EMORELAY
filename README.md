# EMORELAY

开源流量转发管理面板。管理员在 Web 面板创建 TCP/UDP 端口转发规则,由分布在各节点上的 Rust Agent 实际执行转发并回报流量统计。设计与开发计划见 [`plan.md`](./plan.md)。

参考形态:Flux-panel / Nyanpass / ForwardX / Aurora。

## 特性

- **panel-server**(Rust + Axum + SQLx)REST API + gRPC 控制面,SQLite 优先(兼容 PostgreSQL 迁移)。
- **node-agent**(Rust + Tokio)TCP/UDP relay,本地规则落盘,断开主控后继续运行。
- **web**(React 19 + Vite + TS + Tailwind 4)深色现代控制台,登录 / 概览 / 节点 / 规则 / 用户 / 设置。
- 鉴权:Argon2 密码哈希,JWT;Agent 注册 token DB 内只存 SHA-256 哈希。
- 内置 CA + 默认 mTLS（节点证书自动签发 + 吊销）:gRPC 控制面强制双向认证,创建节点一次性下发四件套凭据,支持轮换/吊销。
- 审计:所有写操作落 `audit_logs`;面板「设置」页可查最近 50 条。
- 流量统计:60s 桶聚合,server 端事务 UPSERT,Dashboard 显示过去 24h 流量。
- 通知：右上角全局 Toast 反馈所有写操作。
- 节点安装：「设置」配 Agent 上报端点后，新建节点 Modal 一键复制安装命令，目标机 `curl ... | sudo bash` 完成接入。
- 限速：带宽模板（bandwidth profiles）关联到规则，Agent 端 token bucket 实际执行限速。
- 用户到期 / 滚动 30 天流量配额：到期或超额自动停用名下全部规则，到期账号登录直接拒绝。
- 规则导入导出：JSON 导出（按名称跨实例映射）+ 导入 dry-run 预览（skip/overwrite 策略）。
- 端口自动分配：创建规则不填监听端口时自动取节点池内最小可用端口。
- 防呆：节点上仍有活跃规则时拒绝删除。
- 一键编排:`docker compose up -d`(panel-server + web + sqlite volume)。

## 一键启动

```sh
cp .env.example .env
# 编辑 .env,设置 PANEL_JWT_SECRET 与 PANEL_BOOTSTRAP_ADMIN_PASSWORD
docker compose up -d --build
```

打开 `http://localhost`,用 `.env` 里的 admin 凭据登录。详见 [`docs/deployment.md`](./docs/deployment.md)。

## 开发模式

```sh
# 1. 后端
cp .env.example .env
# 编辑 .env(同上)
cargo run -p panel-server

# 2. 前端(另一窗口)
cd web
npm install
npm run dev   # vite dev server: http://localhost:5173
              # vite.config.ts 已把 /api 反代到 :8080

# 3. Agent(另一窗口,可选)
# 先在 Web 面板创建节点,复制返回的 agent_token
AGENT_NODE_ID=1 \
AGENT_TOKEN=<上一步的 token> \
AGENT_CONTROL_ENDPOINT=http://127.0.0.1:50051 \
cargo run -p node-agent
```

## 仓库结构

```
EMORELAY/
  Cargo.toml              Rust workspace 根
  crates/
    panel-server/         主控 HTTP + gRPC(Axum + SQLx + tonic)
    node-agent/           节点代理(Tokio TCP/UDP relay + tonic + sysinfo)
    common/               共享 protobuf 生成代码
  web/                    React + Vite + TS + Tailwind 前端
  migrations/             SQLx 迁移
  docker/                 Dockerfile + nginx + Caddy 反代示例
  docs/                   部署与 API 文档
  scripts/                辅助脚本(echo server 等)
  docker-compose.yml      一键编排
```

## 文档索引

- [`plan.md`](./plan.md) — 项目设计蓝本 + 各 Phase 实施状态附录（MVP / P1 / P2 / P3a / P3b 控制面）
- [`docs/superpowers/plans/2026-06-10-mvp-followups.md`](./docs/superpowers/plans/2026-06-10-mvp-followups.md) — MVP 后续阶段索引（P1 / P2 / P3）
- [`docs/superpowers/plans/2026-06-10-mvp-followups-phase-3.md`](./docs/superpowers/plans/2026-06-10-mvp-followups-phase-3.md) — P3 计划（P3a + P3b 控制面已交付；P3b 数据面 / P3c 待展开）
- [`docs/deployment.md`](./docs/deployment.md) — 部署与运维
- [`docs/api.md`](./docs/api.md) — REST + gRPC API 参考

## 验收状态(plan 第十三节,2026-06-09)

| # | 验收项 | 状态 |
|---|---|---|
| 1 | `docker compose up -d` 启动 panel-server + web + sqlite | ✅ |
| 2 | 管理员能登录 Web 面板 | ✅ |
| 3 | 能新增节点 | ✅ |
| 4 | Agent 连接主控并显示在线 | ⚠ 代码就绪,端到端实跑 Agent 验证留作 manual smoke |
| 5 | 能创建 TCP 转发规则 | ✅ |
| 6 | Agent 实际监听对应端口 | ✅(`cargo test -p node-agent` 含 TCP/UDP loopback) |
| 7 | TCP 流量能成功转发 | ✅(同上,TCP echo round-trip 验证) |
| 8 | 前端能看到规则流量统计 | ✅ 行内累计 + Dashboard 24h 聚合 + 规则/节点详情页时序 svg |
| 9 | 能禁用/启用/删除规则 | ✅ |
| 10 | Agent 重启恢复已有规则 | ✅(`agent-state.json` + `store.rs`) |
| 11 | README 一键部署 + 开发启动步骤 | ✅ |

MVP 之后已交付 P1（体验防呆）、P2（用户配额 + 限速 + 导入导出）、P3a（内置 CA + 默认 mTLS）、P3b 控制面（多跳隧道 DB/proto/REST）；逐 Phase 交付记录见 [`plan.md`](./plan.md) 附录·实施状态。后续 P3b 数据面 / P3c 见 [`docs/superpowers/plans/2026-06-10-mvp-followups-phase-3.md`](./docs/superpowers/plans/2026-06-10-mvp-followups-phase-3.md)。

## 安全

- 密码 Argon2 哈希;JWT secret 强制环境变量。
- Agent token DB 内只存 SHA-256 哈希,创建节点时面板**仅显示一次**明文。
- 保留端口默认 22/80/443/3306/5432,可在「设置」页改。
- 后端不拼 shell;Agent 只接受白名单 RPC(`ApplyRule`/`RemoveRule`/`EnableRule`/`DisableRule`/`RestartRule`)。
- gRPC 控制面默认走内置 CA 强制 mTLS — 首次启动自动签发 CA + server 证书,创建节点一次性下发四件套凭据,支持轮换/吊销(CRL),配置见 [`docs/deployment.md` §4.5](./docs/deployment.md) 与 [`docs/api.md` §"mTLS 与节点凭据"](./docs/api.md)。`PANEL_DEV_DISABLE_MTLS=1` 退回 plaintext(仅供 dev)。旧的 `PANEL_GRPC_TLS_*` 已弃用。

## License

MIT(占位)。
