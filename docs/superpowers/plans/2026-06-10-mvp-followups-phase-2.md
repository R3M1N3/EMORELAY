# Phase 2 · 用户与限速重构 Implementation Plan（占位）

> **状态**: 占位，待 Phase 1 落地完成后展开为 TDD 任务清单。

**Spec**: [`../specs/2026-06-10-mvp-followups-design.md` §3](../specs/2026-06-10-mvp-followups-design.md)

## 待展开的 Task 概要

按 spec §3 拆出预估 Task 列表（最终展开时再细化）:

1. migration 0002（users 扩展 / forward_rules DROP 三列 / bandwidth_profiles 新表）
2. 后端 sweeper（user_quota.rs：expiry 60s + quota 300s + 30d cache 刷新）
3. 后端 `POST /api/auth/login` 加 `expires_at` 拒登录
4. 后端 `routes/users.rs` 扩展 `expires_at` + `traffic_limit_bytes_30d` + 返回 `period_used_bytes_cached`
5. 后端 `routes/rules.rs::create` 端口自动分配（listen_port = Option<u16>）
6. protobuf `Rule` 字段重排（删 traffic/bandwidth/expires；加 `bandwidth_mbps`）
7. Agent token bucket（`crates/node-agent/src/limit/token_bucket.rs` + TCP/UDP relay 接入）
8. Agent 移除自管 traffic_limit / expires_at（搬给 server）
9. 后端 `routes/bandwidth_profiles.rs` CRUD
10. 后端 `GET /api/rules/export` + `POST /api/rules/import?strategy=&dry_run=`
11. 前端 `/bandwidth-profiles` 路由 + Sidebar 入口
12. 前端 Users 表单扩展 `expires_at` + `traffic_limit_bytes_30d` + 30d 用量进度条
13. 前端 Rules 表单：去三字段 + 加 bandwidth_profile_id 下拉 + listen_port 可空
14. 前端 Rules 工具栏「导入」「导出」按钮 + 导入预览 modal
15. 文档更新（README / docs/api.md / docs/deployment.md / plan.md 附录）

## 启动条件

- Phase 1 全部 Task 子代理 review 通过
- `git log` 内 Phase 1 commit 已在 master
- 用户确认启动 P2

## 启动流程

1. 用户告知「启动 P2」
2. 重新调 `superpowers:writing-plans` 把 spec §3 转成 P2 完整 TDD plan
3. 按 P1 同款节奏推进（每 Task 子代理 review）
