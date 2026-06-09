# Phase 3 · 多跳隧道 + 内置 CA + mTLS Implementation Plan（占位）

> **状态**: 占位，待 Phase 2 落地完成后展开为 TDD 任务清单。

**Spec**: [`../specs/2026-06-10-mvp-followups-design.md` §4](../specs/2026-06-10-mvp-followups-design.md)

## 待展开的 Task 概要（对应 spec §4.9 的 8 个内部 review 单元）

1. 引入 `rcgen` + CA / server cert 生成 + 启动落盘 + nodes 表加 cert_serial / cert_fingerprint 列
2. 强制 mTLS（含 `ClientCertVerifier` + CRL 文件监听热加载 + `PANEL_DEV_DISABLE_MTLS` override）
3. 创建节点响应增加四件套（token + CA + client cert + key）+ install.sh 接收三个 base64 PEM
4. migration 0003（tunnels + tunnel_hops + forward_rules.tunnel_id）+ REST API + 前端 `/tunnels` CRUD
5. proto Rule.tunnel 字段 + Command oneof 加 TunnelCredentials/RevokeTunnelCredentials + dispatcher 按 hop 拆 Rule
6. Agent `tunnel/` 模块 + 三 transport（TCP 先，TLS 次，WSS 最后）+ entry/mid/exit 三角色
7. 节点删除保护扩展（查 tunnel_hops）+ 吊销 API `POST /api/nodes/:id/revoke-credentials` + CRL 热加载
8. 端到端 e2e 测试（双跳 + 三跳 + TCP/TLS 矩阵；UDP-over-tunnel 帧重组用例）

## 启动条件

- Phase 1 + Phase 2 全部 Task 子代理 review 通过
- 用户确认启动 P3（注意 P3 落地等同 fleet-wide Agent 重装，因为存量节点要重新签 client cert，参见 spec §5.1）

## 启动流程

1. 用户告知「启动 P3」
2. 重新调 `superpowers:writing-plans` 把 spec §4 转成 P3 完整 TDD plan
3. 按 P1/P2 同款节奏推进；P3 内部 8 个单元各自独立 review
