# EMORELAY REST API 参考

对应 plan.md 第七节。所有端点以 `/api` 为前缀,JSON in / JSON out。

---

## 一、鉴权

- 登录:`POST /api/auth/login` → `{ token, user }`,服务器使用 HS256 JWT。
- 登录限速(P4):per-IP 稳态 1 次/秒、突发 10 次,超出返回 429(统一 JSON + `Retry-After` 头)。
  限速键取 `X-Forwarded-For` 最左 IP(反代须**覆盖写**该头,见 `docker/web-nginx.conf`)。
- 登录 401 的 `message`:常规失败为「未登录或登录已过期」;密码正确但账号到期为机器码 `account_expired`(前端按此码展示文案)。
- 后续请求:`Authorization: Bearer <token>`。
- 401 自动让前端清 token 并跳登录(`web/src/lib/api.ts`)。
- 权限:`rules` 对普通用户(`role=user`)开放,仅能操作自己名下规则;`nodes` 的 **GET(list/detail)对普通用户开放但响应净化**(见「节点」一节),其余 nodes 操作与 `users` / `bandwidth-profiles` / `tunnels` / `settings` / `export` / `import` 为 admin only。

---

## 二、统一错误格式

P4 起用户可见 `message` 统一为中文;机器可读类别在 `error` 字段,集成方请匹配 `error` 而非 `message` 文本。

```json
{ "error": "bad_request", "message": "监听端口 22 超出节点端口池 [20000-20100]" }
```

| HTTP | `error` |
|---|---|
| 400 | `bad_request` |
| 401 | `unauthorized` |
| 403 | `forbidden` |
| 404 | `not_found` |
| 429 | `too_many_requests`(仅登录限速) |
| 500 | `internal_error` |

---

## 三、端点一览

### 鉴权 `auth`

| 方法 | 路径 | 说明 |
|---|---|---|
| POST | `/api/auth/login` | 用户名+密码登录(per-IP 限速) |
| POST | `/api/auth/logout` | 无状态,前端清 token 即可 |
| GET  | `/api/auth/me` | 当前 token 持有者(P4 起为扩展视图,见下) |

`GET /api/auth/me`(P4 扩展,用户自助概览数据源):除 `id`/`username`/`role` 外返回
`expires_at`、`traffic_limit_bytes_30d`、`period_used_bytes_cached`、
`period_used_calculated_at`、`rule_count`、`total_traffic_bytes`。
登录响应里的 `user` 保持轻量三字段不变。

### 节点 `nodes` (写操作 admin only;GET 对普通用户开放但净化)

| 方法 | 路径 | 说明 |
|---|---|---|
| GET    | `/api/nodes` | 分页(page, page_size, sort, order, search) |
| POST   | `/api/nodes` | 创建,响应一次性返回四件套凭据(见下) |
| GET    | `/api/nodes/{id}` | 详情 |
| PATCH  | `/api/nodes/{id}` | 部分更新 |
| DELETE | `/api/nodes/{id}` | 软删,被活跃规则引用或参与活跃隧道时拒绝(400) |
| GET    | `/api/nodes/{id}/stats` | 当前状态 + node_stats 时序 |
| POST   | `/api/nodes/{id}/revoke-credentials` | 吊销旧证书入 CRL + 重签,返回新三件套(见下) |

创建节点(POST `/api/nodes`)响应**一次性**返回四件套凭据,关闭后不可再取:

- `agent_token` — Agent 注册明文 token(DB 仅存 SHA-256 哈希)。
- `ca_pem` — 内置 CA 证书(Agent 用以校验 server)。
- `client_cert_pem` — 该节点的 mTLS 客户端证书。
- `client_key_pem` — 客户端私钥。**DB 永不持久化**,只存 `cert_serial` + `cert_fingerprint`;丢失即不可找回,只能走「轮换凭据」重签。

P4 起的补充语义:

- `search` 参数:服务端 LIKE 匹配 name / region / public_ip(`%`/`_` 按字面量处理)。
- 响应新增 `agent_version`(Agent register 时上报落库;从未注册为空串)。
- **普通用户视角净化**:`role=user` 调 GET 时响应形状不变,但
  `grpc_endpoint`/`agent_version` 置空、`cpu_usage`/`memory_usage`/`load_average`/
  `rx_bytes_total`/`tx_bytes_total` 置 0;保留 id/name/region/public_ip/status/
  last_seen_at/port_pool(自助建规则所需)。`/api/nodes/{id}/stats` 仍 admin only。

### 转发规则 `rules` (普通用户可用,仅限自己名下规则;export/import 除外)

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

- `listen_port` 可空 = 自动分配(节点池内最小可用,排除 reserved、协议互斥占用与该节点上活跃隧道 inter_port;池满 → 400「已无可用端口」)。
- `bandwidth_profile_id` 创建可选;PATCH 传 `0` = 解除关联;不存在 → 400。**仅 admin 可设/可改**(P4 收紧:普通用户传该字段一律 400,防止解除 admin 挂的限速)。
- `tunnel_id` 创建可选,把该规则挂到一条隧道作为业务入口规则;**仅创建时设定,PATCH 不可改;仅 admin 可设**。给定时 `node_id` 必须等于该隧道入口(ordinal 0)节点,否则 → 400(message 含「入口」)。挂隧道的入口规则其 `listen_port` **豁免节点池范围检查**(隧道入口是面向业务的端口,与节点转发池相互独立),但仍受保留端口红线约束(22/80/443/3306/5432 一律拒绝)。
- `user_id`(P4 新增,创建可选):**仅 admin** 可把规则归属到任意未删用户(规则计入该用户配额,随其到期/超额停用);普通用户传他人 id → 400(传自己 id 等价于不传)。归属创建后不可改。
- 响应含 `bandwidth_mbps`(关联 profile 的当前值,无关联/已删 → `null`)、`tunnel_id`(未挂隧道 → `null`)与 `user_name`(P4 新增,归属用户名)。
- 关联隧道的规则会拆成 entry/mid/exit 实例分发到链上各节点;流量统计与限速只在 entry 计。

### 用户 `users` (admin only)

| 方法 | 路径 | 说明 |
|---|---|---|
| GET    | `/api/users` | 分页 + search(用户名 LIKE,通配符按字面量) |
| POST   | `/api/users` | 创建,密码必须 ≥8 字符 |
| GET    | `/api/users/{id}` | 详情 |
| PATCH  | `/api/users/{id}` | 改密码 / 改角色 / 改到期与配额,禁止自降级,禁止降级最后一个 admin |
| DELETE | `/api/users/{id}` | 软删,禁止删自己,禁止删最后一个 admin |

到期与流量配额字段(创建/PATCH 均可设):

- `expires_at` — 账号到期时刻(UTC),接受 `YYYY-MM-DDTHH:MM`;PATCH 传 `""` 清除。到期后登录被拒(401 `account_expired`),且 sweeper 自动停用名下全部规则。
- `traffic_limit_bytes_30d` — 滚动 30 天流量配额(字节);PATCH 传 `0` 清除。超额后 sweeper 自动停用名下全部规则。
- 响应附带只读字段:`period_used_bytes_cached`(最近一次计算的 30 天用量)、`period_used_calculated_at`(计算时刻)、`period_remaining_bytes`(剩余配额,无配额 → `null`)。
- 滚动 30 天用量由 `rule_stats` 聚合(含已删规则的历史流量,防止"删规则绕配额")。
  **P4 起 `stats_retention_days` 清理任务已生效**:把保留期配置为 < 30 天会真实截短配额
  计算窗口(用量被低估),设置页有同款警告;保留期相关行为见「系统」一节。

### 带宽模板 `bandwidth-profiles` (admin only)

| 方法 | 路径 | 说明 |
|---|---|---|
| GET    | `/api/bandwidth-profiles` | 分页(page, page_size) |
| POST   | `/api/bandwidth-profiles` | 创建 |
| GET    | `/api/bandwidth-profiles/{id}` | 详情 |
| PATCH  | `/api/bandwidth-profiles/{id}` | 部分更新 |
| DELETE | `/api/bandwidth-profiles/{id}` | 软删,被活跃规则引用时拒绝 |

详细语义见「Bandwidth Profiles」一节。

### 隧道 `tunnels` (admin only)

| 方法 | 路径 | 说明 |
|---|---|---|
| GET    | `/api/tunnels` | 列表,每项含 `hops_count` + `rules_count` |
| POST   | `/api/tunnels` | 创建 `{ name, transport, node_ids: [...] }` |
| GET    | `/api/tunnels/{id}` | 详情,含 hops 明细(含 `inter_port`) + `rules_count` + `rules[]` |
| PATCH  | `/api/tunnels/{id}` | 仅改 `name`,响应含 `rules_count` |
| DELETE | `/api/tunnels/{id}` | 软删,被活跃规则引用时拒绝(400) |
| POST   | `/api/tunnels/{id}/restart` | 重签凭据 + per-hop 重启,返回 `dispatched` |
| GET    | `/api/tunnels/{id}/status` | 实时聚合 hop 心跳并回写,返回 `{ id, status }` |

详细语义见「Tunnels」一节。

### 系统 `system` (admin only)

| 方法 | 路径 | 说明 |
|---|---|---|
| GET   | `/api/system/overview` | 总节点 / 在线 / 总规则 / 启用 / rx_tx 累计 + `rx_bytes_24h`/`tx_bytes_24h`(P4:过去 24h **规则转发流量**,rule_stats 口径,区别于 nodes 表的网卡累计) |
| GET   | `/api/system/audit-logs` | 分页 + 过滤(action, target_type, result) |
| GET   | `/api/system/settings` | 当前 K/V 配置(未配置过的 key 不出现在响应中) |
| PATCH | `/api/system/settings` | 修改,key 走白名单 + 每键类型校验 |

settings 白名单键:

- `reserved_ports` — JSON 整数数组,规则监听端口黑名单。
- `stats_retention_days` — 时序分钟桶保留天数(≥1,默认 30)。**P4 起生效**:后台每小时
  (`PANEL_STATS_RETENTION_SWEEP_SECS`)分批删除 `rule_stats`/`node_stats` 超期行;
  不清理 `audit_logs`。设 < 30 会截短 30 天滚动配额计算窗口。
- `agent_control_endpoint` — Agent 连入地址,安装命令嵌入用(http/https)。
- `notify_webhook_url`(P4 新增) — 出站通知地址(http/https,空 = 关闭),见「Webhook 通知」一节。

### Webhook 通知(P4)

配置 `notify_webhook_url` 后,以下事件以 `POST` JSON 推送(fire-and-forget,5s 超时,
失败重试 1 次后丢弃;事件不保证必达):

```json
{ "event": "node.offline", "occurred_at": "2026-06-11T08:00:00+00:00", "data": { "node_id": 1, "name": "hk-a" } }
```

| event | 触发 | data |
|---|---|---|
| `node.offline` | 掉线 sweeper 把心跳超时(默认 120s)的 online 节点置 offline | `{node_id, name}` |
| `node.online` | 离线节点经 register/心跳/统计上报恢复 | `{node_id}` |
| `user.expired` | 到期用户名下规则被自动停用 | `{user_id, disabled_rule_count}` |
| `user.quota_exceeded` | 超额用户名下规则被自动停用 | `{user_id, disabled_rule_count}` |

https 端点须公网受信证书(rustls webpki roots,不读系统证书库);内网接收器可用 http。

### 健康检查

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/health` | DB 探活;返回 `{ ok: true, db: "ok" }` |

---

## 四、关键字段语义

### `nodes.status`
- `online` — register / heartbeat / ReportNodeStats 任一路径写入
- `offline` — 掉线 sweeper(P4)检测到心跳超过 `PANEL_NODE_OFFLINE_AFTER_SECS`(默认 120s)后置位
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
# 注意响应里的 agent_token + ca_pem + client_cert_pem + client_key_pem —— 四件套仅此一次显示。
# 私钥 DB 不持久化,丢失只能走「轮换凭据」(见「mTLS 与节点凭据」)。
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
- `SubscribeCommands` — server → agent 命令流(ApplyRule / RemoveRule / RestartRule / Enable / Disable;`tunnel_credentials` / `revoke_tunnel_credentials` 两支用于隧道凭据下发/吊销,见「隧道凭据下发」小节)
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
- DELETE 时若仍被活跃规则引用 → 400,message 含引用数(「限速配置仍被 N 条规则引用」),需先解除关联。
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

---

## 十、mTLS 与节点凭据

P3a 起 gRPC 控制面默认**强制 mTLS**,证书由 panel-server 内置 CA 自动管理,不再依赖 `scripts/gen-dev-tls.sh` 或外部 CA。

### 内置 CA

- 首次启动时 panel-server 在 `${PANEL_DATA_DIR}/tls/` 自动生成一份 ECDSA P-256 CA + server 证书(幂等,私钥权限 0600)。已存在则复用。
- server 证书的 SAN 主机名取自 `PANEL_PUBLIC_HOST`(Agent 连入时校验)。留空 → 仅含 `127.0.0.1` / `localhost`,远程 Agent 会因 SAN 不匹配握手失败。
- 开发逃生阀 `PANEL_DEV_DISABLE_MTLS=1` 退回 plaintext(Agent 端 `AGENT_CONTROL_ENDPOINT` 须用 `http://` 配合)。**仅供本地开发**。
- 旧的 `PANEL_GRPC_TLS_CERT` / `PANEL_GRPC_TLS_KEY` / `PANEL_GRPC_TLS_CLIENT_CA` 三项已**弃用**,serve 时被忽略(保留仅为兼容旧 `.env`)。

### 四件套语义

创建节点(POST `/api/nodes`)一次性返回 `agent_token` + `ca_pem` + `client_cert_pem` + `client_key_pem`。私钥 DB 永不持久化(只存 `cert_serial` + `cert_fingerprint`),关闭返回后**不可找回**,丢失只能走「轮换凭据」重签。

前端把四件套(含私钥)嵌进可复制的一键安装命令(见 §"一键安装节点")。

> **安全提示**:四件套(含私钥)随安装命令以明文出现在命令行,执行期间会留在 shell history 与 `ps` 输出里 —— 与既有 `--token=` 同一暴露级别。自托管面板可接受,但在共享/多人机器上安装后应清理 shell history(如 `history -c` / 删除 `~/.bash_history` 对应行)。

### 轮换 / 吊销凭据

`POST /api/nodes/{id}/revoke-credentials`(admin):

- 把该节点当前证书指纹写入持久化 CRL(`${PANEL_DATA_DIR}/tls/crl.json`,原子写),并重新签发一套新证书。
- 响应返回 `{ ca_pem, client_cert_pem, client_key_pem }`(**不含** token —— token 不变,只换证书)。
- gRPC register 时被吊销的客户端证书会被拒(`PermissionDenied`,失败原因 `revoked_cert`)。
- 记 audit `node.credentials_revoked`。

适用场景:私钥泄漏、四件套丢失需重装、或下线某台 Agent。

### 升级到 P3a = 全节点重装

升级到 P3a 时,存量(P1/P2)活跃节点会在启动时自动补 `cert_serial` / `cert_fingerprint`(audit `node.mtls_credentials_issued`),但**拿不到私钥明文**,因此无法直接连入。管理员须逐个进节点详情页点「轮换凭据」拿到新四件套并重装 Agent。换言之,**升级到 P3a 等于全节点 Agent 重装**;dev/staging 可改设 `PANEL_DEV_DISABLE_MTLS=1` 走 plaintext 过渡。

### 新增 audit actions

- `node.credentials_revoked` — 轮换/吊销凭据。
- `node.mtls_credentials_issued` — 升级时存量节点自动补签证书元数据。
- gRPC register 失败原因新增 `revoked_cert`(证书已吊销)。

---

## 十一、Tunnels(多跳隧道)

多跳隧道把多个节点串成一条转发链:入口(entry)→ 中继(mid…)→ 出口(exit)。全部 admin only。

### 端点

| 方法 | 路径 | 说明 |
|---|---|---|
| GET    | `/api/tunnels` | 列表,每项含 `hops_count`(链路节点数)+ `rules_count`(关联规则数) |
| POST   | `/api/tunnels` | 创建 `{ name, transport, node_ids: [...] }` |
| GET    | `/api/tunnels/{id}` | 详情,含 hops 明细(`ordinal` / `node_id` / `inter_port`)+ `rules_count` + `rules[]` |
| PATCH  | `/api/tunnels/{id}` | 仅可改 `name`(链路拓扑不可后改),响应含 `rules_count` |
| DELETE | `/api/tunnels/{id}` | 软删,被活跃规则引用时拒绝(400) |
| POST   | `/api/tunnels/{id}/restart` | 重签下发 hop 凭据 + 对隧道全部活跃规则 per-hop 重启,返回 `dispatched` |
| GET    | `/api/tunnels/{id}/status` | 实时聚合 hop 心跳,刷新并**回写** `tunnels.status`,返回 `{ id, status }` |

### 创建语义与校验

`POST /api/tunnels`,请求体:

```json
{ "name": "hk-jp-us", "transport": "tcp", "node_ids": [3, 7, 9] }
```

- `name` 非空。
- `transport` ∈ `{ tcp, tls, wss }`,全链路统一一种 transport。`wss` 不保证 TCP 半关语义,依赖半关的业务流选 `tcp` 或 `tls`。
- `node_ids` 是有序链路(`[entry, mid…, exit]`),要求**≥2 个**、**不重复**、**全部 online**;任一不在线 → 400(message 含「在线」)。
- `ordinal ≥ 1` 的节点必须有 `public_ip`(将被上一跳拨号),否则 → 400。
- **inter_port 自动分配**:`ordinal ≥ 1` 的每个 hop 从**其自身节点**的 `port_pool` 分配一个 inter_port(排除 reserved + 该节点上活跃 `forward_rules.listen_port` + 该节点上活跃 `tunnel_hops.inter_port`);`ordinal 0`(入口)的 inter_port 为 `null`。
- 整个创建是事务性的。并发创建撞同一端口由 DB 偏函数唯一索引(`tunnel_hops(node_id, inter_port) WHERE inter_port IS NOT NULL`)兜底 → 400(message:`中继端口分配冲突(可能有并发创建),请重试`)。

### 删除保护

- 删隧道:仍被活跃规则(`forward_rules.tunnel_id` 引用且未软删)引用时 → 400,需先解除规则关联。
- 删节点:节点参与任一活跃隧道时 → 400(与既有「被活跃规则引用」保护并列)。

### 字段语义

- `tunnels.transport` — `tcp` / `tls` / `wss`。
- `tunnels.status` — `up` / `degraded` / `down` / `unknown`。由 hop 心跳聚合得出(最近 30s 内收到心跳 = 该 hop 存活):全部存活 → `up`;部分存活 → `degraded`;全部超窗 → `down`;无 hop → `unknown`(防御值)。`GET /api/tunnels/{id}` 与 `GET /api/tunnels/{id}/status` 均**实时计算并写回**存储值——即这两个 GET 端点有写副作用;`GET /api/tunnels`(列表)返回上次刷新写入的存储值,避免分页 N 次聚合。
- `tunnel_hops.ordinal` — 链路序号,`0` = 入口。
- `tunnel_hops.inter_port` — 该 hop 监听的链路内部端口;入口(ordinal 0)为 `null`。
- `rules_count` — 挂在该隧道下的活跃转发规则数(列表 TunnelView + 详情 TunnelDetail + PATCH 响应均含此字段)。
- `rules` — 仅 `GET /api/tunnels/{id}`（TunnelDetail）含此字段,为该隧道关联规则简要列表,每项结构：`{ id, name, protocol, listen_port, enabled }`。

### 规则关联隧道

转发规则可通过 `tunnel_id` 挂到一条隧道作为业务入口规则,详见「转发规则」一节的 `tunnel_id` 要点:仅创建时设定(PATCH 不可改),`node_id` 必须等于隧道入口节点,入口规则 `listen_port` 豁免节点池范围检查但仍守保留端口红线。关联隧道的规则会拆成 entry/mid/exit 实例分发到链上各节点;流量统计与限速只在 entry 计。

### 隧道凭据下发

Server 为每条隧道各 hop 即时签发凭据并通过 gRPC `Command.tunnel_credentials` 下发;凭据不入 DB。

- `Command.tunnel_credentials` 含 `tunnel_id`、`ordinal`、节点 TLS cert/key、`ca_pem`(自包含,Agent 无需额外信任链)。
- 创建隧道时即时签发并下发到链上全部节点;reconcile(Agent 重连)重发。
- `restart` 重新签发全链 hop 凭据(轮换),再对全部活跃规则 per-hop 重启。
- 删除隧道时 Server 向各 hop 节点发送 `Command.revoke_tunnel_credentials`，Agent 清理本地 `${AGENT_DATA_DIR}/tunnels/<id>/hop-<ordinal>/` 目录。

### audit actions

- `tunnel.create` — 创建隧道。
- `tunnel.update` — 改名。
- `tunnel.delete` — 软删隧道。
- `tunnel.restart` — 重签凭据 + per-hop 规则重启。
