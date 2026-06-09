# EMORELAY MVP 后续 · 总览与执行索引

**日期**: 2026-06-10
**Spec**: [`../specs/2026-06-10-mvp-followups-design.md`](../specs/2026-06-10-mvp-followups-design.md)

## 三阶段索引

| Phase | Plan 文件 | 状态 |
|---|---|---|
| **P1 · 体验与防呆** | [`2026-06-10-mvp-followups-phase-1.md`](./2026-06-10-mvp-followups-phase-1.md) | 已展开，可执行 |
| **P2 · 用户与限速重构** | [`2026-06-10-mvp-followups-phase-2.md`](./2026-06-10-mvp-followups-phase-2.md) | 占位；P1 完成后展开 |
| **P3 · 隧道 + 内置 CA + mTLS** | [`2026-06-10-mvp-followups-phase-3.md`](./2026-06-10-mvp-followups-phase-3.md) | 占位；P2 完成后展开 |

## 推进原则

- 严格按 `CLAUDE.md`「每完成一个原子单元 spawn 子代理 review」：每个 Task 完成后 spawn `general-purpose` 子代理调 `superpowers:code-reviewer` 审查；审查未通过不进下一个 Task
- 思考过程必须全程中文（CLAUDE.md 最高优先级规则）
- 每个 Phase 落地后再开始下一 Phase 的 plan 展开（避免实现细节提前固化）
- 每个 Task 用 TDD：先写失败测试 → 跑失败 → 写最少代码 → 跑通过 → commit
- 每个 commit 必须独立可回滚；commit message 用现有项目风格（中文 / 英文均可，参照 `git log`）

## 工具与命令

- 后端测试: `cargo test --workspace`（或 `cargo test -p panel-server --test <name>`）
- 前端测试: `cd web && npm test`（vitest run）
- 前端 build: `cd web && npm run build`
- 一键起服务: `docker compose up -d --build`
- gRPC 自签证书脚本: `bash scripts/gen-dev-tls.sh ./tls`（dev 期用）

## Phase 间的依赖关系

- P1 不依赖 P2/P3
- P2 不依赖 P3，但依赖 P1 的 Toast（错误展现）+ Settings 端点（导入提示用）
- P3 依赖 P1 的安装命令模板（要扩展三个 PEM base64 参数）+ P2 的 protobuf 字段重排（避免 Rule 字段号冲突）

## 完成定义（每个 Phase）

按 Spec 中各 Phase 的验收清单全部勾掉 + `cargo test --workspace` + `npm run build` + 全部 Task 的 review 通过。
