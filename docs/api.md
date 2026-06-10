# EMORELAY REST API 参考

对应 plan.md 第七节。所有端点以 `/api` 为前缀,JSON in / JSON out。

---

## 一、鉴权

- 登录:`POST /api/auth/login` → `{ token, user }`,服务器使用 HS256 JWT。
- 登录 401 的 `message` 有两种:`invalid username or password`(常规失败)与 `account_expired`(密码正确但账号已过 `expires_at`)。
- 后续请求:`Authorization: Bearer <token>`。
- 401 自动让前端清 token 并跳登录(`web/src/lib/api.ts`)。
- 当前所有写操作 + 大多数读操作要求 `role=admin`;普通用户路径(`role=user`)规划中 — 见 `fix-plan.md` F8。

---

## 二、统一错误格式

```json
{ "error": "bad_request", "message": "listen_port outside node's port pool" }
```

| HTTP | `error` |
|---|---|
| 400 | `bad_request` |
| 401 | `unauthorized` |
| 403 | `forbidden` |
| 404 | `not_found` |
| 500 | `internal_error` |

---

## 三、端点一览

### 鉴权 `auth`

| 方法 | 路径 | 说明 |
|---|---|---|
| POST | `/api/auth/login` | 用户名+密码登录 |
| POST | `/api/auth/logout` | 无状态,前端清 token 即可 |
| GET  | `/api/auth/me` | 当前 token 持有者 |

### 节点 `nodes` (admin only)

| 方法 | 路径 | 说明 |
|---|---|---|
| GET    | `/api/nodes` | 分页(page, page_size, sort, order) |
| POST   | `/api/nodes` | 创建,响应一次性返回明文 `agent_token` |
| GET    | `/api/nodes/{id}` | 详情 |
| PATCH  | `/api/nodes/{id}` | 部分更新 |
| DELETE | `/api/nodes/{id}` | 软删 |
| GET    | `/api/nodes/{id}/stats` | 当前状态 + node_stats 时序 |

### 转发规则 `rules` (admin only)

| 方法 | 路径 | 说明 |
|---|---|---|
| GET    | `/api/rules` | 分页 + 过滤(node_id, protocol, search) |
| POST   | `/api/rules` | 创建,自动下发 Agent 的 ApplyRule |
| GET    | `/api/rules/{id}` | 详情 |
| PATCH  | `/api/rules/{id}` | 修改,自动下发更新 |
| DELETE | `/api/rules/{id}` | 软删,自动下发 RemoveRule |
| POST   | `/api/rules/{id}/enable` | 启用 |
| POST   | `/api/rules/{id}/disable` | 禁用 |
| POST   | `/api/rules/{id}/restart` | 重启(Agent 重建 listener) |
| GET    | `/api/rules/{id}/stats` | 当前状态 + rule_stats 时序 |
| GET    | `/api/rules/{id}/logs` | 该规则相关 audit_logs |
| GET    | `/api/rules/export` | 导出规则 JSON(见「Rules Export / Import」) |
| POST   | `/api/rules/import` | 导入规则,默认 dry-run(见「Rules Export / Import」) |

创建/修改要点:

- `listen_port` 可空 = 自动分配(节点池内最小可用,排除 reserved 与协议互斥占用;池满 → 400 `port pool exhausted`)。
- `bandwidth_profile_id` 创建可选;PATCH 传 `0` = 解除关联;不存在 → 400。
- 响应含 `bandwidth_mbps`(关联 profile 的当前值,无关联/已删 → `null`)。

### 用户 `users` (admin only)

| 方法 | 路径 | 说明 |
|---|---|---|
| GET    | `/api/users` | 分页 |
| POST   | `/api/users` | 创建,密码必须 ≥8 字符 |
| GET    | `/api/users/{id}` | 详情 |
| PATCH  | `/api/users/{id}` | 改密码 / 改角色 / 改到期与配额,禁止自降级,禁止删最后一个 admin |
| DELETE | `/api/users/{id}` | 软删,禁止删自己,禁止删最后一个 admin |

到期与流量配额字段(创建/PATCH 均可设):

- `expires_at` — 账号到期时刻(UTC),接受 `YYYY-MM-DDTHH:MM`;PATCH 传 `""` 清除。到期后登录被拒(401 `account_expired`),且 sweeper 自动停用名下全部规则。
- `traffic_limit_bytes_30d` — 滚动 30 天流量配额(字节);PATCH 传 `0` 清除。超额后 sweeper 自动停用名下全部规则。
- 响应附带只读字段:`period_used_bytes_cached`(最近一次计算的 30 天用量)、`period_used_calculated_at`(计算时刻)、`period_remaining_bytes`(剩余配额,无配额 → `null`)。
- 滚动 30 天用量由 `rule_stats` 聚合(含已删规则的历史流量,防止"删规则绕配额");若将来把 stats 保留期配置为 < 30 天会截短配额窗口(当前版本未实现 retention 清理,仅为前瞻提示)。

### 带宽模板 `bandwidth-profiles` (admin only)

| 方法 | 路径 | 说明 |
|---|---|---|
| GET    | `/api/bandwidth-profiles` | 分页(page, page_size) |
| POST   | `/api/bandwidth-profiles` | 创建 |
| GET    | `/api/bandwidth-profiles/{id}` | 详情 |
| PATCH  | `/api/bandwidth-profiles/{id}` | 部分更新 |
| DELETE | `/api/bandwidth-profiles/{id}` | 软删,被活跃规则引用时拒绝 |

详细语义见「Bandwidth Profiles」一节。

### 系统 `system` (admin only)

| 方法 | 路径 | 说明 |
|---|---|---|
| GET   | `/api/system/overview` | 总节点 / 在线 / 总规则 / 启用 / rx_tx 累计 |
| GET   | `/api/system/audit-logs` | 分页 + 过滤(action, target_type, result) |
| GET   | `/api/system/settings` | 当前 K/V 配置 |
| PATCH | `/api/system/settings` | 修改,key 走白名单 + 每键类型校验 |

### 健康检查

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/health` | DB 探活;返回 `{ ok: true, db: "ok" }` |

---

## 四、关键字段语义

### `nodes.status`
- `online` — 最近一次 heartbeat / ReportNodeStats 在窗口内
- `offline` — 超出窗口
- `unknown` — 从未连接

### `forward_rules.protocol`
- `tcp` / `udp` / `tcp_udp`

### `forward_rules.listen_port`
- 必须落在节点的 `port_pool_min..=port_pool_max` 内
- 必须不在系统 `reserved_ports`(默认 22/80/443/3306/5432)
- POST 时可省略 = 自动分配(节点池内最小可用,排除 reserved 与协议互斥占用;池满 → 400)

---

## 五、curl 示例

登录:
```sh
TOKEN=$(curl -sX POST http://localhost:8080/api/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"<your-pwd>"}' | jq -r .token)
```

创建节点:
```sh
curl -sX POST http://localhost:8080/api/nodes \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"name":"hk-relay-01","region":"HK","public_ip":"1.2.3.4"}' | jq .
# 注意响应里的 agent_token —— 仅此一次显示,DB 不再存明文。
```

创建规则:
```sh
curl -sX POST http://localhost:8080/api/rules \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"node_id":1,"name":"to-jp","protocol":"tcp","listen_port":20000,"target_host":"5.6.7.8","target_port":443}' | jq .
```

查 Dashboard 概览:
```sh
curl -s http://localhost:8080/api/system/overview -H "Authorization: Bearer $TOKEN" | jq .
```

---

## 六、gRPC 控制面

REST 之外,panel-server 同时监听 gRPC `:50051` 用于 Agent。协议见 `crates/common/proto/control.proto`:

- `Register` — Agent 上线(node_id + agent_token)
- `SubscribeCommands` — server → agent 命令流(ApplyRule / RemoveRule / RestartRule / Enable / Disable)
- `Heartbeat` — 10s 心跳(含 CPU/MEM/LOAD)
- `ReportNodeStats(stream)` — 60s 上报 NodeStatsBatch
- `ReportRuleStats(stream)` — 60s 上报 RuleStatsBatch

session_token 通过 gRPC metadata `x-emorelay-session` 携带。

---

## 七、安装相关（公开，带 rate limit）

### `GET /install.sh?node=<id>`

返回参数化 bash 脚本，Content-Type `text/x-shellscript`。需要调用时附加 `--token=<明文>`：

```sh
curl -fsSL https://relay.example.com/install.sh?node=42 | sudo bash -s -- --token=<明文>
```

依赖 `system_settings.agent_control_endpoint` 已配 + `PANEL_PUBLIC_BASE_URL` env 已配。

Rate limit：60 req/分钟/IP（看 §"Rate limit 与反代 header"）。

### `GET /dist/node-agent-linux-{amd64,arm64}`

提供预编译 agent 二进制下载（musl 静态链接）。文件名严格白名单，其他文件名 → 404。Rate limit 同上。

---

## 八、Bandwidth Profiles

带宽模板:规则不再各自带限速值,而是关联一个可复用的 profile。全部 admin only。

| 方法 | 路径 | 说明 |
|---|---|---|
| GET    | `/api/bandwidth-profiles` | 分页(page, page_size) |
| POST   | `/api/bandwidth-profiles` | 创建 `{ name, bandwidth_mbps, description? }` |
| GET    | `/api/bandwidth-profiles/{id}` | 详情 |
| PATCH  | `/api/bandwidth-profiles/{id}` | 部分更新 |
| DELETE | `/api/bandwidth-profiles/{id}` | 软删 |

约束与行为:

- `name` 在活跃(未软删)profile 中唯一,重名 → 400。
- `bandwidth_mbps` 必须 > 0。
- DELETE 时若仍被活跃规则引用 → 400,message 含引用数(`referenced by N active rule(s)`),需先解除关联。
- PATCH 修改 `bandwidth_mbps` 后,引用该 profile 的活跃规则**即时重下发**(Agent 重建 token bucket)。注意:存量 TCP 连接持旧限速直到自然断开,新连接用新值;UDP 即时生效。
- 规则侧用法见「转发规则」一节的 `bandwidth_profile_id`。

---

## 九、Rules Export / Import

跨实例迁移/备份规则。全部 admin only。

### `GET /api/rules/export?node_id=&user_id=`

- `node_id` / `user_id` 均可选,用于过滤;不传 = 导出全部活跃规则。
- 响应为 attachment JSON(`Content-Disposition: attachment`),用名称(`node_name` / `bandwidth_profile_name`)而非 id 做跨实例映射,**不含** `id` / `user_id` / `created_at`。
- `tunnel_name` 恒为 `null`,为 P3 隧道功能预留。

单条导出项格式(共 10 个字段):

```json
{
  "name": "game-jp-route",
  "protocol": "tcp",
  "listen_ip": "0.0.0.0",
  "listen_port": 20001,
  "target_host": "game-us.example.com",
  "target_port": 443,
  "enabled": true,
  "node_name": "hk-relay-01",
  "tunnel_name": null,
  "bandwidth_profile_name": "vip-100m"
}
```

### `POST /api/rules/import?strategy=skip|overwrite&dry_run=1|0`

- `strategy` 默认 `skip`;`dry_run` 默认 `1`(只预览不落库)。
- 请求体为导出格式的 JSON 数组,逐项处理并返回报告:

```json
{
  "dry_run": true,
  "strategy": "skip",
  "items": [
    { "index": 0, "action": "create", "reason": "..." },
    { "index": 1, "action": "skip", "reason": "..." }
  ]
}
```

`action` 取值 `create | skip | overwrite | error`。映射与冲突规则:

- `node_name` 必须存在于本实例,找不到 → 该项 `error`。
- `bandwidth_profile_name` 找不到 → 置空导入(不自动建 profile)。
- `tunnel_name` 非空 → `error`(隧道留待 P3)。
- 精确同 binding(同 node/protocol/listen_ip/listen_port)的已有规则按 `strategy` 处理(skip 跳过 / overwrite 覆盖);互斥协议冲突(如 `tcp_udp` 撞 `tcp`)→ `error`。
- 非 dry-run 落库后记一条聚合 audit `rule.import`(含 created/overwritten/skipped/errors 计数)。
