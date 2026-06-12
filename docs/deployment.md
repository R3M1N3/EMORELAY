# EMORELAY 部署指南

本文档覆盖一键编排、生产部署、反代/TLS、备份与故障排查。

> **最快路径**:Debian 12/13 上直接 `curl -fsSL https://raw.githubusercontent.com/Remine1337/EMORELAY/master/deploy.sh | bash`,菜单选「快速安装」拉取 GitHub Release 预编译二进制(由 `.github/workflows/release.yml` 在打 `v*` tag 时构建,含双架构 musl 静态二进制 + 前端产物 + SHA256SUMS),免编译约 1 分钟装完。本文其余章节为 Docker/手动部署细节。

---

## 一、组件拓扑

```
                                       ┌───────────────────────┐
                  浏览器 ──── 80/443 ──>│  web (nginx)          │
                                       │  ├─ SPA 静态资源        │
                                       │  └─ /api/* 反代          │
                                       └──────────┬────────────┘
                                                  │ 容器内网
                                                  ▼
                                       ┌───────────────────────┐
                                       │  panel-server          │
                                       │  ├─ REST    :8080      │
                                       │  └─ gRPC    :50051     │
                                       └──────────┬────────────┘
                                                  │
                                          sqlite-data (volume)

  各转发节点的 node-agent ─── gRPC :50051 ──> panel-server
                              (token + session,生产应叠 TLS)
```

REST 与 gRPC 是两条独立链路。前端不直连 Agent,Agent 也不暴露无鉴权 gRPC。

---

## 二、一键编排(docker compose)

> 对应 plan 第十三节验收第 1 条。

1. 准备 `.env`:
   ```sh
   cp .env.example .env
   ```
   必填:
   - `PANEL_JWT_SECRET` — 任意 ≥32 字符随机串(`openssl rand -hex 32` 生成)
   - `PANEL_BOOTSTRAP_ADMIN_PASSWORD` — 首次启动自动创建 admin 用此密码

2. 起服务:
   ```sh
   docker compose up -d --build
   ```

3. 访问:
   - Web 控制台: `http://localhost`
   - panel-server REST: `http://127.0.0.1:8080`(默认绑本机,生产请改 Caddy 反代)
   - Agent 接入端: `host:50051`

4. 关掉/重置:
   ```sh
   docker compose down            # 停容器,保留 sqlite
   docker compose down -v         # 同时删 sqlite-data volume
   ```

---

## 三、环境变量参考

| 变量 | 默认 | 说明 |
|---|---|---|
| `PANEL_BIND_ADDR` | `0.0.0.0:8080` | REST 监听 |
| `PANEL_GRPC_BIND_ADDR` | `0.0.0.0:50051` | gRPC 监听 |
| `PANEL_DEV_DISABLE_MTLS` | `0` | 置 `1` 退回 plaintext gRPC(仅开发);默认走内置 CA 强制 mTLS |
| `PANEL_PUBLIC_HOST` | — | 内置 CA 写入 server 证书的对外主机名(Agent 校验);留空仅含 `127.0.0.1`/`localhost`。**必须是 Agent 能直连的主机名/IP**:不能填 CDN/Cloudflare 橙云代理域名(CDN 不转发 50051 且会终结 TLS),Web 域名挂了 CDN 时请给 gRPC 单独配一个直连子域(灰云) |
| `PANEL_GRPC_TLS_CERT` | — | **P3a 起弃用**,被忽略(gRPC TLS 走内置 CA) |
| `PANEL_GRPC_TLS_KEY` | — | **P3a 起弃用**,被忽略 |
| `PANEL_GRPC_TLS_CLIENT_CA` | — | **P3a 起弃用**,被忽略 |
| `PANEL_DATABASE_URL` | `sqlite:///data/emorelay.db` | DB 路径;`create_if_missing` 已开启 |
| `PANEL_JWT_SECRET` | **必填** | JWT HMAC 密钥,缺失则启动失败 |
| `PANEL_JWT_EXPIRY_HOURS` | `24` | 颁发的 token 有效期 |
| `PANEL_CORS_ORIGIN` | `http://localhost:5173`(代码默认)/ compose 内 `http://localhost` | 允许的前端 origin |
| `PANEL_BOOTSTRAP_ADMIN_USERNAME` | `admin` | 首次启动自动创建的管理员账号 |
| `PANEL_BOOTSTRAP_ADMIN_PASSWORD` | **必填(首次)** | 同上;已有 admin 则忽略 |
| `PANEL_USER_EXPIRY_SWEEP_SECS` | `60` | 用户级 sweeper:到期扫描间隔;到期用户名下全部规则自动停用并记聚合 audit |
| `PANEL_USER_QUOTA_SWEEP_SECS` | `300` | 用户级 sweeper:滚动 30 天流量配额扫描间隔;超额同样自动停用名下全部规则 |
| `RUST_LOG` | `info` | `tracing` EnvFilter |
| `AGENT_NODE_ID` | — | Agent 上报使用,创建节点后由 panel-server 返回 |
| `AGENT_CONTROL_ENDPOINT` | `http://127.0.0.1:50051` | Agent 连接的 server gRPC URL |
| `AGENT_TOKEN` | — | 创建节点时一次性返回的明文 token(只显示一次,DB 仅存哈希) |
| `AGENT_STATE_PATH` | `./agent-state.json` | Agent 本地规则持久化 |
| `AGENT_STATS_INTERVAL_SECS` | `60` | 规则+节点统计上报间隔 |

---

## 四、反向代理 / TLS

plan 第三节要求"默认给出 Caddy 配置",对应 `docker/Caddyfile.example`。两种部署形态:

### 4.1 Host 上的 Caddy(默认 example)

适合:已有一台 host 跑多个项目,Caddy 在 host 上统一反代。

**前置改动**(避免 Caddy 与 web 容器抢 host:80):

1. 编辑 `docker-compose.yml`,把 web 服务端口改为:
   ```yaml
   ports:
     - "127.0.0.1:8081:80"
   ```
2. 编辑 `.env`,把 `PANEL_CORS_ORIGIN` 设为最终对外域名:
   ```sh
   PANEL_CORS_ORIGIN=https://emorelay.example.com
   ```
3. 重启 compose:`docker compose up -d`

然后部署 Caddyfile:

```sh
sudo cp docker/Caddyfile.example /etc/caddy/Caddyfile
# 改 emorelay.example.com 为你的域名;Caddyfile.example 已对齐到 :8081 与 :8080
sudo systemctl reload caddy
```

Caddy 会自动申请并续期 TLS 证书(把域名 A 记录指向 host)。

### 4.2 把 Caddy 也加入 compose

```yaml
caddy:
  image: caddy:2-alpine
  ports:
    - "80:80"
    - "443:443"
  volumes:
    - ./docker/Caddyfile.example:/etc/caddy/Caddyfile:ro
    - caddy-data:/data
    - caddy-config:/config
  depends_on:
    - web
    - panel-server
  restart: unless-stopped

volumes:
  caddy-data:
  caddy-config:
```

并把 `Caddyfile.example` 内的 `localhost:8080` / `localhost:80` 改成 compose 服务名 `panel-server:8080` / `web:80`,移除 web 的 80 端口外露。

### 4.3 gRPC TLS（P3a 起弃用）

> **⚠️ P3a 起本节流程已弃用。** gRPC 控制面默认走内置 CA 强制 mTLS(见 §4.5),`PANEL_GRPC_TLS_CERT` / `PANEL_GRPC_TLS_KEY` / `PANEL_GRPC_TLS_CLIENT_CA` 与 `scripts/gen-dev-tls.sh` 不再生效,以下内容仅作历史参考。

panel-server 支持 gRPC 通道 TLS。两步:

1. 生成自签 CA + server cert(开发):
   ```sh
   sh scripts/gen-dev-tls.sh ./tls
   ```
   Windows 用 git bash / WSL,或 `docker run --rm -v "%cd%:/work" -w /work alpine/openssl sh scripts/gen-dev-tls.sh`。
   生产请用 Let's Encrypt / 公司 CA 签发真实证书,跳过本步。

2. 配置 panel-server `.env`:
   ```sh
   PANEL_GRPC_TLS_CERT=/path/to/server.crt
   PANEL_GRPC_TLS_KEY=/path/to/server.key
   ```
   并把 Agent 的 endpoint 改成 `https://...`,然后:
   ```sh
   AGENT_CONTROL_ENDPOINT=https://your-domain:50051
   AGENT_GRPC_CA_CERT=/path/to/ca.crt   # 自签;真实证书可留空走系统根证书
   ```

panel-server 启动时会日志确认 `grpc control plane listening (server TLS - no client cert required)`;两个 TLS env 都空时会 warn `running in PLAINTEXT (...not recommended for production)`。

### 4.4 gRPC mTLS(双向认证) — P3a 起弃用

> **⚠️ P3a 起本节流程已弃用。** mTLS 现已是内置 CA 的默认行为,无需手动配 CA 与客户端证书,见 §4.5。以下内容仅作历史参考。

单向 TLS(4.3)只让 Agent 验证 server 身份;mTLS 在此基础上让 server 也验证 Agent 的客户端证书,真正"双向认证"。开启步骤(在 4.3 已配置 server TLS 的基础上):

1. 用同一份 CA 给 Agent 签客户端证书(脚本已支持):
   ```sh
   sh scripts/gen-dev-tls.sh ./tls
   # 产物含 ca.crt / server.crt / server.key / agent.crt / agent.key
   ```

2. panel-server `.env` 增加:
   ```sh
   PANEL_GRPC_TLS_CLIENT_CA=/path/to/ca.crt
   ```
   该 CA 签发的客户端证书都被信任;未提供证书或证书链不被信任的 Agent 连接直接被 TLS 层拒绝(早于 gRPC 鉴权)。

3. Agent 端 `.env` 增加:
   ```sh
   AGENT_GRPC_CLIENT_CERT=/path/to/agent.crt
   AGENT_GRPC_CLIENT_KEY=/path/to/agent.key
   ```
   两者必须同时配置,否则 Agent 启动时直接 fail-fast。

启动后日志确认 `grpc control plane listening (mTLS - client cert required)`。Settings 页"安全状态"也会显示"Token + mTLS"。

注意:生产中如果 Agent 数量多,可让每台 Agent 用独立 client cert 便于撤销。撤销机制 MVP 未实现(不维护 CRL/OCSP),需要时直接换 CA 重签所有合法 Agent。

故障排查:mTLS 握手失败时 Agent 拿到的是 TLS 层错误(传输 transport error),而不是 gRPC `PermissionDenied`。常见原因:
- Agent 未配 `AGENT_GRPC_CLIENT_CERT/KEY`,或路径不可读
- Agent 证书不是同一 CA 签发的(指纹对不上)
- Agent 证书缺少 `extendedKeyUsage = clientAuth`(`scripts/gen-dev-tls.sh` 已加,自己签的需手动加)
- Agent 端 `AGENT_CONTROL_ENDPOINT` 是 `http://` 而非 `https://`(此时 client cert 被忽略,Agent 日志会 warn 但实际仍裸跑)

### 4.5 升级到 P3a（启用 mTLS，当前默认）

P3a 起 gRPC 控制面默认走 panel-server **内置 CA 的强制 mTLS**:首次启动自动在 `${PANEL_DATA_DIR}/tls/` 生成 CA + server 证书,创建节点时一次性下发四件套凭据(token + CA + client cert + client key),无需再用 `scripts/gen-dev-tls.sh` 或外部 CA。详细 API 语义见 [`docs/api.md` §"mTLS 与节点凭据"](./api.md)。

升级步骤:

1. **首次 P3a 启动前**先把 `PANEL_PUBLIC_HOST` 设为面板对 Agent **可直连**的主机名(如 `grpc.example.com`)。内置 CA 据此写 server 证书 SAN;留空则只含 `127.0.0.1`/`localhost`,远程 Agent 握手会因 SAN 不匹配失败。注意它与 Web 访问域名是两回事:Web 域名走 CDN/Cloudflare 橙云时,这里必须用直连源站的域名/IP(deploy.sh 安装时会分开两问),否则 Agent 报 `invalid peer certificate: certificate not valid for name`。SAN 在首次启动后固化,改值需删除 `${PANEL_DATA_DIR}/tls/` 重启并给所有已接入 Agent 重装凭据。
2. 启动后,存量(P1/P2)活跃节点会自动补证书元数据,但**拿不到私钥明文,在轮换前无法连入**。逐个进**节点详情页 → 轮换凭据 → 复制新四件套安装命令 → 在目标机重装 Agent**。
3. dev/staging 若不想逐个重装,可改设 `PANEL_DEV_DISABLE_MTLS=1` 走 plaintext 过渡(Agent 端 `AGENT_CONTROL_ENDPOINT` 同步改 `http://`)。**仅限非生产**。

> **⚠️ 升级到 P3a = 全节点 Agent 重装。** 存量节点必须逐个「轮换凭据」并重装才能恢复连接;请安排维护窗口。
>
> 旧的 `PANEL_GRPC_TLS_CERT` / `PANEL_GRPC_TLS_KEY` / `PANEL_GRPC_TLS_CLIENT_CA` 升级后被忽略,可从 `.env` 移除(保留亦不报错)。

---

## 五、Agent 部署

每台转发节点跑一个 `node-agent`(单独构建,不在 compose 内,因为它要绑节点本地端口)。

```sh
cargo build --release -p node-agent
AGENT_NODE_ID=42 \
AGENT_CONTROL_ENDPOINT=https://agent.emorelay.example.com \
AGENT_TOKEN=<面板上一次性显示的明文> \
AGENT_STATE_PATH=/var/lib/emorelay/agent-state.json \
./target/release/node-agent
```

或写 systemd unit `/etc/systemd/system/emorelay-agent.service`:

```ini
[Unit]
Description=EMORELAY node-agent
After=network-online.target
Wants=network-online.target

[Service]
EnvironmentFile=/etc/emorelay/agent.env
ExecStart=/usr/local/bin/node-agent
Restart=on-failure
RestartSec=5
User=emorelay
Group=emorelay

[Install]
WantedBy=multi-user.target
```

---

## 六、备份与升级

- **备份**:库开着 WAL,服务运行中直接 `cp` 活库可能拷到未 checkpoint 的半截状态。
  两种安全姿势任选:
  - **在线备份(推荐,不停服)**:用带 sqlite3 CLI 的侧车跑 `.backup`(会自动处理 WAL):
    ```sh
    docker run --rm \
      -v emorelay_sqlite-data:/var/lib/emorelay \
      -v "$PWD/backup":/backup \
      keinos/sqlite3 sqlite3 /var/lib/emorelay/emorelay.db \
      ".backup /backup/emorelay-$(date +%F).db"
    ```
    systemd 部署同理(宿主机 `apt install sqlite3`):
    ```sh
    sqlite3 /var/lib/emorelay/emorelay.db ".backup /root/backup/emorelay-$(date +%F).db"
    ```
  - **停服拷贝**:`docker compose stop panel-server`(或 `systemctl stop emorelay-panel`)后
    把 `emorelay.db`、`emorelay.db-wal`、`emorelay.db-shm` 三个文件一起拷走再起服务。
  - 除 db 外,`${PANEL_DATA_DIR}/tls/` 整目录(内置 CA 私钥)必须随库一起备份——
    丢了 CA 等于全部 Agent 凭据作废,需逐节点轮换重装。
  (compose 项目名默认是目录名,例如 `emorelay_sqlite-data`,用 `docker volume ls` 确认。)
- **升级**: `docker compose pull && docker compose up -d --build`。Migration 由 panel-server 启动时 `sqlx::migrate!` 自动跑。
- **回滚**: 备份目录里恢复 db 文件(`docker run --rm -v emorelay_sqlite-data:/var/lib/emorelay -v "$PWD/backup":/backup alpine cp /backup/<file>.db /var/lib/emorelay/emorelay.db`),降级镜像 tag。

---

## 七、故障排查

| 症状 | 排查 |
|---|---|
| `panel-server` 启动失败 "PANEL_JWT_SECRET is required" | `.env` 缺该项 |
| Agent 注册 "permission denied" | token 不对 / 节点已被软删 |
| Web 登录提示 "用户名或密码错误" | 检查 `PANEL_BOOTSTRAP_ADMIN_*` 是否生效;首次启动后 bootstrap 不再触发 |
| Dashboard 一直显示"加载中…" | 浏览器 DevTools 看 `/api/*` 响应;CORS 报错改 `PANEL_CORS_ORIGIN` |
| 节点显示 offline 但 Agent 在跑 | `docker logs panel-server` 看 register/heartbeat 日志;时钟漂移可致 session 过期 |
| `cargo build` 内存爆 | builder 默认无 jobserver 限制,可在 Dockerfile 加 `RUN cargo build -j 2 ...` |

---

## 八、安全清单

部署前确认:

- [ ] `PANEL_JWT_SECRET` 是高熵随机串
- [ ] `PANEL_BOOTSTRAP_ADMIN_PASSWORD` 已改过且不再保留在历史 shell
- [ ] `PANEL_CORS_ORIGIN` 只列出真实前端域
- [ ] gRPC 50051 与 REST 8080 不直接对外暴露(走 Caddy/Nginx 反代)
- [ ] Agent token 在面板创建节点时复制后立即清屏,DB 内仅存哈希
- [ ] 系统设置中的 `reserved_ports` 已根据机器实际占用调整

---

## 九、Rate limit 与反代 header

`/install.sh` 与 `/dist/*` 端点带 IP 维度 rate limit（60 req/min），登录另有 per-IP 限速。生产部署时 panel-server 通常在反向代理（Caddy / Nginx）之后，**反代必须传递 `X-Forwarded-For` 或 `X-Real-IP` 头部**：取不到这两个头时中间件会回退到 TCP 对端地址（即反代自身 IP），所有客户端共享同一个限流桶，高峰期会出现无差别 429（登录被陌生人挤兑限速）。

Caddy 默认会自动添加；Nginx 需要显式：

```nginx
proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
proxy_set_header X-Real-IP $remote_addr;
```

---

## 十、一键安装节点（P1）

前置：在 Web 面板「设置」页填 **Agent 上报端点**（如 `https://relay.example.com:50051`），并确保 `PANEL_PUBLIC_BASE_URL` env 配为面板对外可访问的 origin（如 `https://relay.example.com`）。

步骤：

1. 面板「节点」页点「新增节点」，提交后弹出 Modal 显示四件套凭据（token + CA + client cert + client key）+ 一键安装命令。
2. 复制命令（P3a 起命令带 base64 编码的 mTLS PEM，形如 `curl -fsSL https://relay.example.com/install.sh?node=42 | sudo bash -s -- --token=<明文> --ca-pem-b64=<...> --client-cert-pem-b64=<...> --client-key-pem-b64=<...>`）。
3. 在目标机以 root 执行该命令。脚本会：
   - 下载 `/dist/node-agent-linux-<amd64|arm64>` 到 `/usr/local/bin/emorelay-agent`
   - 写 `/etc/emorelay/tls/{ca,client,client-key}.pem`（权限 0600）并把 `AGENT_GRPC_CA_CERT`/`CLIENT_CERT`/`CLIENT_KEY` 写进 agent.env
   - 写 `/etc/emorelay/agent.env`（权限 0600）
   - 写 `/etc/systemd/system/emorelay-agent.service`
   - `systemctl enable --now emorelay-agent`
4. 回到面板「节点」页，节点状态在 1-2 分钟内变 `online`。

四件套仅创建时显示一次（私钥 DB 不持久化）；丢失只能走节点详情页「轮换凭据」重签，详见 [`docs/api.md` §"mTLS 与节点凭据"](./api.md)。

> 不带证书参数重跑安装脚本会**保留**已有 mTLS 配置（不降级为 plaintext）。

> **安全提示**：四件套（含私钥）以明文出现在安装命令里，执行期间会留在 shell history 与 `ps` 输出（与 `--token=` 同一暴露级别）。共享机器上安装后请清理 shell history。
