# EMORELAY REST API 参考

对应 plan.md 第七节。所有端点以 `/api` 为前缀,JSON in / JSON out。

---

## 一、鉴权

- 登录:`POST /api/auth/login` → `{ token, user }`,服务器使用 HS256 JWT。
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

### 用户 `users` (admin only)

| 方法 | 路径 | 说明 |
|---|---|---|
| GET    | `/api/users` | 分页 |
| POST   | `/api/users` | 创建,密码必须 ≥8 字符 |
| GET    | `/api/users/{id}` | 详情 |
| PATCH  | `/api/users/{id}` | 改密码 / 改角色,禁止自降级,禁止删最后一个 admin |
| DELETE | `/api/users/{id}` | 软删,禁止删自己,禁止删最后一个 admin |

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

### `traffic_limit_bytes` / `bandwidth_limit_mbps`
- `null` = 不限
- `traffic_limit_bytes` 超限将由 Agent 自动 stop(F1 规划中;当前 MVP 仅持久化)

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
