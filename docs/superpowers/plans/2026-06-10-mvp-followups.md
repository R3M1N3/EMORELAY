# EMORELAY MVP 后续 · 总览与执行索引

**日期**: 2026-06-10
**Spec**: [`../specs/2026-06-10-mvp-followups-design.md`](../specs/2026-06-10-mvp-followups-design.md)

> MVP 已交付后,按 spec 的 13 条增量建议分阶段推进。各阶段独立交付、独立 review、独立回归。

## 阶段索引

| Phase | 状态 | Plan |
|---|---|---|
| **P1 · 体验与防呆** | ✅ 已交付 | 计划文件已删;交付记录见 `plan.md` 附录 |
| **P2 · 用户与限速重构** | ✅ 已交付 | 计划文件已删;交付记录见 `plan.md` 附录 |
| **P3a · 内置 CA + 默认 mTLS** | ✅ 已交付 | [`phase-3.md`](./2026-06-10-mvp-followups-phase-3.md) P3a 段 |
| **P3b 控制面 · 隧道 DB/proto/REST** | ✅ 已交付 | [`phase-3.md`](./2026-06-10-mvp-followups-phase-3.md) P3b 控制面段 |
| **P3b 数据面 · Agent 隧道转发** | ⏳ 待展开 | [`phase-3.md`](./2026-06-10-mvp-followups-phase-3.md) P3b 数据面概要 |
| **P3c · 隧道前端 + e2e** | ⏳ 待展开 | [`phase-3.md`](./2026-06-10-mvp-followups-phase-3.md) P3c 概要 |

## 推进原则

- 严格按 `CLAUDE.md`「每完成一个原子单元 spawn 子代理 review」：每个 Task 完成后 spawn `general-purpose` 子代理调 `superpowers:code-reviewer`;审查未通过不进下一个 Task（P3 起采用 spec 合规 + code-quality 双重 review）。
- 思考过程必须全程中文（CLAUDE.md 最高优先级规则）。
- 每个子阶段落地后再展开下一段的 TDD plan（避免实现细节提前固化）。
- 每个 Task 用 TDD：先写失败测试 → 跑失败 → 写最少代码 → 跑通过 → commit。
- 每个 commit 独立可回滚;commit message 参照 `git log` 现有风格。

## 工具与命令

- 后端测试: `cargo test --workspace`（或 `cargo test -p panel-server --test <name>`）
- 前端测试: `cd web && npm test`（vitest run）;前端 build: `cd web && npm run build`
- 一键起服务: `docker compose up -d --build`
- gRPC mTLS: P3a 起 panel-server 启动自动签发内置 CA 并默认强制 mTLS;dev 走 plaintext 用 `PANEL_DEV_DISABLE_MTLS=1`

## 完成定义（每个子阶段）

按 Spec 中各 Phase 验收清单全部勾掉 + `cargo test --workspace` + `npm run build` + 全部 Task 的双重 review 通过。
