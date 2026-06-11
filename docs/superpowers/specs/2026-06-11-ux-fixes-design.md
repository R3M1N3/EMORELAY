# UX 修复批次（P4）设计文档

日期：2026-06-11
输入：`docs/ux-review-2026-06-11.md`（playwright 实测评审）
目标：修复评审发现的 P0 体系断裂 + P2 体验摩擦；P1 大型新功能（负载均衡、2FA-TOTP、i18n、图表库等）**不在本批次**，仅在第八节登记。

## 0. 侦察补充发现

评审报告之外，设计侦察期确认了一个更严重的隐藏 bug：

- **节点永不掉线**：`status='online'` 仅在 gRPC register/心跳/统计上报三处写入
  （`grpc/service.rs:155,194,334`），全仓库没有任何代码把它写回 `offline`。
  Agent 进程死掉后面板永远显示在线。本批次的「节点掉线 sweeper」同时修此 bug
  并充当 webhook 通知的事件源。

## 1. 范围内修复项与设计决策

### A. 节点掉线检测（修 bug + 通知事件源）

新增 `sweeper/node_offline.rs`，模式与 `sweeper/user_quota.rs` 一致（spawn + pub `tick_once` 供集成测试）：

- 周期 `PANEL_NODE_OFFLINE_SWEEP_SECS`（默认 30s，下限 5s）。
- 判定阈值 `PANEL_NODE_OFFLINE_AFTER_SECS`（默认 120s = 2 个 Agent 心跳周期，下限 10s）。
- 动作：`UPDATE nodes SET status='offline' WHERE status='online' AND last_seen_at < datetime('now','-N seconds') AND deleted_at IS NULL`，
  先 SELECT 捕获将翻转的节点（id/name），翻转后逐节点发 `node.offline` webhook + 聚合 audit
  （`node.offline_detected`，actor 为 None）。
- 恢复：gRPC 三处写 `online` 前先 `SELECT status`，旧值为 `offline` 则发 `node.online` webhook。
  接受三处各 ~4 行的小重复，换取路径清晰（恢复可能从 register 或心跳/统计任一路径发生）。

取舍：不用 `UPDATE ... RETURNING`（虽然 SQLite/PG 都支持，但 sqlx Any 路径下行为差异
不值得为省一次 SELECT 引入）；误判抖动由 120s 阈值（2×心跳）控制。

### B. 统计保留清理（修「配置是摆设」）

新增 `sweeper/stats_retention.rs`：

- 周期 `PANEL_STATS_RETENTION_SWEEP_SECS`（默认 3600s，下限 5s）。
- 每 tick 现读 `system_settings.stats_retention_days`（缺省 30），改完即生效。
- 删除语句（PG 兼容，不用 rowid / DELETE LIMIT）：
  `DELETE FROM rule_stats WHERE id IN (SELECT id FROM rule_stats WHERE bucket_at < datetime('now', ?) LIMIT 5000)`
  循环到 rows_affected=0；`node_stats` 同理。批量上限防长事务/锁库。
- 删除行数 >0 时 `tracing::info`；不写 audit（周期性内务，会刷屏）。
- **不清理 audit_logs**：审计保留是合规属性，超出本项语义（设置页说明文字同步写明）。

### C. Webhook 通知（最小可用通知渠道）

新增 `notify/mod.rs`：

- 配置：`system_settings` 新 key `notify_webhook_url`（加入 `ALLOWED` 白名单；
  校验：空=禁用，否则必须 http/https URL）。
- API：`notify::spawn_send(state, event: &'static str, data: serde_json::Value)` ——
  内部 `tokio::spawn`（fire-and-forget，绝不阻塞调用方）：读配置，空则返回；
  POST JSON `{ "event": ..., "occurred_at": <UTC ISO>, "data": {...} }`；
  超时 5s；失败重试 1 次；仍失败 `tracing::warn` 后放弃（事件允许丢失，v1 不做投递保证）。
- 依赖：panel-server 新增 `reqwest`（default-features=false + json + rustls-tls）。
- 事件 v1（四个）：
  - `node.offline` / `node.online`：data = {node_id, name}
  - `user.quota_exceeded` / `user.expired`：data = {user_id, disabled_rule_count}
    （挂在 `user_quota.rs` 两个 sweeper 的既有命中点）
- SSRF 说明：URL 仅 admin 可配，面板机本就由 admin 控制，不做内网地址过滤（记录在案）。
- Telegram/邮件适配器：后续在此模块上扩展，本批次不做。

### D. 普通用户体系（修最大断裂）

后端：

1. `GET /api/auth/me` 扩展为 `MeView`：原 id/username/role +
   `expires_at`、`traffic_limit_bytes_30d`、`period_used_bytes_cached`、
   `period_used_calculated_at`、`rule_count`、`total_traffic_bytes`
   （单行聚合 SQL，与 users::list 的 JOIN 同构）。`LoginResponse.user` 保持轻量不变。
2. `GET /api/nodes`（list 与 get）放行普通用户，但响应经
   `NodeView::sanitize_for_user()`：`grpc_endpoint` / `agent_version` 置空、
   `cpu_usage`/`memory_usage`/`load_average` 置 0、`rx_bytes_total`/`tx_bytes_total` 置 0；
   保留 id/name/region/public_ip/status/last_seen_at/port_pool_min/max/created/updated。
   理由：建规则需要节点身份/在线状态/端口池/入口 IP；运维指标与控制面地址不给。
   JSON 形状不变 → 前端类型零分叉。`/api/nodes/{id}/stats` 维持 admin-only。
3. `POST /api/rules`：`CreateRuleRequest` 加 `user_id: Option<i64>`。
   admin 传 → 校验目标用户存在未删，规则归属该用户；非 admin 传 → 400「仅管理员可指定归属用户」。
   不做 update 改归属（建错删了重建，软删无代价）。
4. 权限收紧（堵现存漏洞）：非 admin 的 create 中 `bandwidth_profile_id`/`tunnel_id`
   必须为空，否则 400「仅管理员可配置限速/隧道」；非 admin 的 update 中
   `bandwidth_profile_id` 同理（tunnel_id 本就不可 update）。
   理由：现状普通用户可 PATCH 自己规则解除 admin 挂的限速档——必须封死。
5. `RuleView` 加 `user_name: String`（list/get JOIN users.username）供归属列显示。
6. `GET /api/rules/{id}/stats` 与 `/logs` 确认 owner 检查一致（logs 已有，stats 实现时核对补齐）。
7. `GET /api/nodes` 与 `GET /api/users` 加 `search` 参数（服务端 LIKE，`%`/`_` 转义，
   nodes 匹配 name/region/public_ip，users 匹配 username），替换前端「搜索当前页」陷阱。
8. `GET /api/system/overview` 加 `rx_bytes_24h`/`tx_bytes_24h`
   （`rule_stats` 24h SUM——**转发流量口径**，供 Dashboard 替换网卡口径的 24h 卡片）。

前端：

1. 导航按角色渲染：user 只见「概览/规则」；admin 全量。
2. Dashboard 按角色分流：user 渲染用户版概览（我的规则数/启用数、累计流量、
   30d 用量与配额进度条、到期时间；数据源 = 扩展后的 me + rules.list）；
   admin 版「过去 24h 流量」卡片改调 overview 新字段，标签改「24h 转发流量」，
   「总流量」hint 注明「规则转发累计」；删除原先逐节点拉 node_stats 聚合的二阶段逻辑（简化）。
3. Rules 页 user 模式：隐藏 导入/导出按钮、限速下拉、隧道下拉、归属列；
   节点下拉用净化后的同一 API，新增规则按钮恢复可用。
4. Rules 页 admin 模式：列表加「归属」列（user_name）；表单加「归属用户」下拉
   （users.list page_size=100；>100 用户时下拉不全为已知限制，登记在第八节）。
5. 403 兜底：admin-only 页面（Nodes/NodeDetail/Tunnels/TunnelDetail/Users/
   BandwidthProfiles/Settings）入口处 `role !== 'admin'` 直接渲染 `ForbiddenCard`
   （ui.tsx 新组件：友好说明卡），杜绝裸 "forbidden"。

### E. 登录防爆破（廉价高收益，纳入）

`/api/auth/login` 套独立 `tower_governor` layer（依赖已存在）：per_second(1)、
burst_size(10)、SmartIpKeyExtractor。稳态 1 次/秒、突发 10 次，对正常用户无感。
2FA-TOTP 不在本批次。

### F. 体验止血（前端）

1. **自动刷新**：新 hook `useAutoRefresh(cb, ms)`（`document.hidden` 时跳过该 tick；
   cb 存 ref 防闭包陈旧）。接入：Dashboard/Rules/Tunnels/NodeDetail/RuleDetail 30s，
   Nodes/TunnelDetail 15s。刷新只重拉列表数据，不触碰 Modal 表单 state。
2. **删除预检**：节点删除 Modal 打开时拉 `rules.list({node_id, page_size:1})` 的 total，
   >0 → 显示「有 N 条规则，需先删除或迁移」并禁用确认按钮；
   隧道删除用已有 `rules_count`（同时修正「关联规则将失去隧道绑定」的错误文案）；
   用户删除用已有 `rule_count` 同理提示。
3. **凭据 Modal 流程**：新增节点表单在 `agent_control_endpoint` 未配置时顶部黄条提示
   （附设置页链接，仍允许创建）；凭据 Modal 未配置分支文案改为
   「未配置 Agent 上报端点，本次凭据请手动保存；配置后可在节点行轮换凭据重新生成安装命令」，
   删除「再回到这里」死路话术。
4. **隧道徽章**：Rules 列表在监听单元格下显示「隧道 <name>」徽章
   （tunnel_id → 已拉取的 tunnelList 映射）；RuleDetail 配置区加「隧道」行。
5. **30d 用量新鲜度**：Users 列表 30d 用量单元格 `title` 显示
   「计算于 <period_used_calculated_at>，约每 5 分钟更新」。
6. **时间本地化**：`shortTime()` 改为把后端 UTC 时间（`YYYY-MM-DD HH:MM:SS` 或 ISO）
   解析为 UTC 并按浏览器本地时区格式化输出。全站显示统一受益。
7. **到期时间输入**：Users 表单 `expires_at` 改 `datetime-local`（本地时区填写），
   提交时转 UTC `YYYY-MM-DDTHH:MM`；编辑回填时 UTC→本地。标签改「到期时间（本地时区）」。
8. **Sparkline**：数据点 <2 时渲染「数据不足」占位（不再画满幅三角）；
   右上角加峰值标注（max 值 formatBytes）。
9. **服务端搜索接线**：Nodes/Users 搜索框改为服务端 search（与 Rules 同交互：
   输入 + 搜索按钮/回车），placeholder 去掉「当前页」字样。
10. **a11y 粘连**：顶栏 username 与 role 徽章、Dashboard 错误行 action 与 target
    之间补空格分隔文本。

### G. 错误信息中文化（统一语言）

- `error.rs`：`BadRequest` Display 去掉 `bad request: ` 前缀（机器码已在 `error` 字段）；
  `NotFound`→「资源不存在」、`Unauthorized`→「未登录或登录已过期」、
  `Forbidden`→「无权限执行此操作」、`Internal`/`Database`→「服务器内部错误」。
- 全部 routes 中用户可见的 `BadRequest(...)` 英文 message 改中文
  （rules/nodes/users/tunnels/bandwidth_profiles/system/rules_io/auth）。
- 集成测试中断言英文 message 的地方同步修正。

### H. Agent 版本可见（小改进）

- migration `0008_node_agent_version.sql`：`ALTER TABLE nodes ADD COLUMN agent_version TEXT NOT NULL DEFAULT ''`。
- gRPC register 已携带 version（审计可见）→ register 时落库。
- `NodeView` 加 `agent_version`；节点列表状态单元格下显示 `v0.1.0` 小字（空则不显示）；
  user 净化视图置空。

### I. 设置页

- 新增「通知 Webhook URL」字段（说明四个事件 + POST JSON 格式）。
- 「统计保留天数」说明文字更新：清理任务已生效，并注明不清理审计日志。

## 2. 数据与接口变更汇总

| 类型 | 变更 |
|---|---|
| migration | 0008：nodes.agent_version |
| settings key | + `notify_webhook_url` |
| env | + `PANEL_NODE_OFFLINE_SWEEP_SECS` / `PANEL_NODE_OFFLINE_AFTER_SECS` / `PANEL_STATS_RETENTION_SWEEP_SECS` |
| REST | me 扩展；nodes list/get 放行 user（净化）+search；users list +search；rules create +user_id、权限收紧、RuleView+user_name；overview +24h 转发流量；login 加限速 |
| 依赖 | panel-server + reqwest（rustls） |
| webhook | 4 事件 POST JSON |

兼容性：所有 REST 变更为加字段/放宽（除「非 admin 传 profile/tunnel 改为 400」——
原行为是漏洞，无合法依赖方）。Agent/gRPC/proto 零变更。

## 3. 测试策略

- Rust 集成测试（panel-server/tests）：
  - offline sweeper：旧心跳节点 tick 后置 offline；register 后恢复 online。
  - retention sweeper：旧 bucket 删除、新 bucket 保留；settings 改值即生效。
  - webhook：测试内起一次性本地 HTTP 接收器（tokio TcpListener 手写读取），
    断言 node.offline 事件 POST 的 JSON 形状。
  - me 扩展字段；nodes user 净化（200 且敏感字段为空/0）；user 建规则闭环
    （user 创建自己的规则成功、传 user_id/profile/tunnel 被 400）；
    admin user_id 归属；search；login 连发 12 次出现 429；overview 24h 字段。
  - 既有全量测试保持绿（错误文案断言修正属于本批次工作）。
- 前端 vitest：现有测试修复 + 沿用既有模式补 Rules user 模式渲染断言。
- 手动验收：实现完成后重起 dev 栈 + playwright 冒烟：
  普通用户登录见用户概览并能自建规则；杀 agent 后节点 ~2 分钟变 offline 且 webhook 接收器收到事件；列表自动刷新。

## 4. 实施单元（每单元：实现 → cargo/vitest 验证 → 子代理 code review → commit）

| # | 单元 | 依赖 |
|---|---|---|
| T1 | 错误中文化 + login 限速 | — |
| T2 | stats retention sweeper | — |
| T3 | 节点掉线 sweeper + 恢复检测 + agent_version（migration/register/NodeView） | — |
| T4 | notify webhook 模块 + settings key + 四事件接入 | T3 |
| T5 | REST 用户体系后端（me/nodes 净化/user_id/收紧/user_name/search/overview 24h/stats owner） | — |
| T6 | 前端角色体系（导航/用户概览/Rules 两模式/归属/ForbiddenCard/Dashboard 24h 改源） | T5 |
| T7 | 前端体验止血（F.1-F.10） | T5 |
| T8 | 设置页（webhook/保留天数说明）+ 节点版本列 + 文档（README/api.md/.env.example/plan.md 附录/评审报告标注） | T4 |

## 5. 风险与对策

- **错误文案改动破坏测试**：预期内，T1 内一次性修完断言再 commit。
- **nodes 放行 user 的安全面**：净化函数单测 + 集成断言敏感字段为空；
  spec 明确保留字段清单防漂移。
- **自动刷新与表单竞态**：刷新仅 setState 列表数据，Modal 持本地副本；
  评审走查时验证编辑中不丢输入。
- **reqwest 引入编译时间/体积**：rustls + 最小 features；可接受。
- **sweeper 多实例**：单进程部署前提（与现状 user_quota sweeper 相同假设），不做分布式锁。

## 6. 非目标（本批次明确不做）

多目标负载均衡/故障转移、图表库时序图、DNS 周期重解析、TOTP 2FA、节点分组、
连接数限制、IP ACL、整库备份恢复、Agent 自动升级、i18n、Telegram/邮件适配器、
audit_logs 保留策略、规则归属 update、>100 用户的归属下拉分页。
以上连同「用户自助改密码」登记为后续候选。
