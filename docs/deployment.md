# EMORELAY 部署指南

本文档对应 plan.md 第十二节第 18-19 步,覆盖一键编排、生产部署、反代/TLS、备份与故障排查。

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
| `PANEL_DATABASE_URL` | `sqlite:///data/emorelay.db` | DB 路径;`create_if_missing` 已开启 |
| `PANEL_JWT_SECRET` | **必填** | JWT HMAC 密钥,缺失则启动失败 |
| `PANEL_JWT_EXPIRY_HOURS` | `24` | 颁发的 token 有效期 |
| `PANEL_CORS_ORIGIN` | `http://localhost:5173`(代码默认)/ compose 内 `http://localhost` | 允许的前端 origin |
| `PANEL_BOOTSTRAP_ADMIN_USERNAME` | `admin` | 首次启动自动创建的管理员账号 |
| `PANEL_BOOTSTRAP_ADMIN_PASSWORD` | **必填(首次)** | 同上;已有 admin 则忽略 |
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

### 4.3 gRPC TLS

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

### 4.4 gRPC mTLS(双向认证,推荐生产)

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

- **备份**(panel-server runtime 镜像不带 sqlite3 CLI,用 alpine 侧车直接拷文件):
  ```sh
  docker run --rm \
    -v emorelay_sqlite-data:/data \
    -v "$PWD/backup":/backup \
    alpine sh -c 'cp /data/emorelay.db /backup/emorelay-$(date +%F).db'
  ```
  (compose 项目名默认是目录名,例如 `emorelay_sqlite-data`,用 `docker volume ls` 确认。)
- **升级**: `docker compose pull && docker compose up -d --build`。Migration 由 panel-server 启动时 `sqlx::migrate!` 自动跑。
- **回滚**: 备份目录里恢复 db 文件(`docker run --rm -v emorelay_sqlite-data:/data -v "$PWD/backup":/backup alpine cp /backup/<file>.db /data/emorelay.db`),降级镜像 tag。

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
