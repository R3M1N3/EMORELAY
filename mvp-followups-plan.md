# EMORELAY MVP 后续 · 总览与执行索引

> 仓库根索引副本。原版位于 [`docs/superpowers/plans/2026-06-10-mvp-followups.md`](./docs/superpowers/plans/2026-06-10-mvp-followups.md)。

**日期**: 2026-06-10
**Spec**: [`docs/superpowers/specs/2026-06-10-mvp-followups-design.md`](./docs/superpowers/specs/2026-06-10-mvp-followups-design.md)

## 三阶段索引

| Phase | Plan 文件 | 状态 |
|---|---|---|
| **P1 · 体验与防呆** | [`docs/superpowers/plans/2026-06-10-mvp-followups-phase-1.md`](./docs/superpowers/plans/2026-06-10-mvp-followups-phase-1.md) | 已展开，可执行 |
| **P2 · 用户与限速重构** | [`docs/superpowers/plans/2026-06-10-mvp-followups-phase-2.md`](./docs/superpowers/plans/2026-06-10-mvp-followups-phase-2.md) | 占位；P1 完成后展开 |
| **P3 · 隧道 + 内置 CA + mTLS** | [`docs/superpowers/plans/2026-06-10-mvp-followups-phase-3.md`](./docs/superpowers/plans/2026-06-10-mvp-followups-phase-3.md) | 占位；P2 完成后展开 |

## Phase 1 任务一览（已展开）

| # | Task | 文件 | 类型 |
|---|---|---|---|
| 1 | Toast Provider + `useToast` | `web/src/lib/toast.tsx` + 单测 | 前端 |
| 2 | App 接入 ToastProvider + Rules 走 Toast | `App.tsx` / `Rules.tsx` | 前端 |
| 3 | 创建规则默认 TCP+UDP | `Rules.tsx` | 前端 |
| 4 | 防删节点（后端 + 集成测试） | `routes/nodes.rs` + 新测试文件 | 后端 |
| 5 | 防删节点（前端 Toast） | `Nodes.tsx` | 前端 |
| 6 | Settings 加 `agent_control_endpoint`（后端） | `routes/system.rs` + `bootstrap.rs` + migration | 后端 |
| 7 | Settings 加 `agent_control_endpoint`（前端） | `Settings.tsx` | 前端 |
| 8 | `/install.sh` + `/dist/*` 端点 | `routes/install.rs` + `config.rs` + `main.rs` | 后端 |
| 9 | install 端点 rate limit | `routes/mod.rs` + `Cargo.toml` | 后端 |
| 10 | Dockerfile cross-compile + compose 挂载 | `docker/panel-server.Dockerfile` + `docker-compose.yml` | 运维 |
| 11 | 节点 Modal「复制安装命令」 | `Nodes.tsx` + `NodeDetail.tsx` + `lib/api.ts` | 前端 |
| 12 | P1 文档 + e2e smoke + plan.md 附录 | `README.md` / `docs/*` / `plan.md` | 文档 |

## 推进原则

- 严格按 `CLAUDE.md`「每完成一个原子单元 spawn 子代理 review」：每个 Task 完成后 spawn `general-purpose` 子代理调 `superpowers:code-reviewer` 审查；审查未通过不进下一个 Task
- 思考过程必须全程中文（CLAUDE.md 最高优先级规则）
- 每个 Phase 落地后再开始下一 Phase 的 plan 展开
- 每个 Task 用 TDD：先写失败测试 → 跑失败 → 写最少代码 → 跑通过 → commit
- 每个 commit 独立可回滚；commit message 遵循 `git log` 现有风格

## 工具与命令

- 后端测试: `cargo test --workspace`（或 `cargo test -p panel-server --test <name>`）
- 前端测试: `cd web && npm test`（vitest run）
- 前端 build: `cd web && npm run build`
- 一键起服务: `docker compose up -d --build`

## Phase 间的依赖关系

- P1 不依赖 P2/P3
- P2 不依赖 P3，但依赖 P1 的 Toast（错误展现）+ Settings 端点（导入提示用）
- P3 依赖 P1 的安装命令模板（要扩展三个 PEM base64 参数）+ P2 的 protobuf 字段重排（避免 Rule 字段号冲突）

## 完成定义（每个 Phase）

按 Spec 中各 Phase 的验收清单全部勾掉 + `cargo test --workspace` + `npm run build` + 全部 Task 的 review 通过。
