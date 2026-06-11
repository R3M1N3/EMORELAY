# EMORELAY

开源流量转发管理面板。在 Web 面板上创建 TCP/UDP 端口转发规则与多跳隧道，由部署在各台服务器上的 Rust Agent 实际执行转发并回报流量统计。

适合管理自有服务器、NAT 节点和端口转发业务，参考形态：Flux-panel / Nyanpass / ForwardX / Aurora。

## 它能做什么

- **端口转发**：TCP / UDP 规则可视化管理，流量统计、连接数、启停一目了然
- **多跳隧道**：TCP / TLS / WSS 多跳中继（入口 → 中转 → 出口），UDP-over-tunnel
- **多用户**：普通用户自助建规则、看用量；支持账号到期与滚动 30 天流量配额，超限自动停用
- **限速**：带宽模板关联规则，Agent 端实际执行
- **安全默认**：gRPC 控制面内置 CA 强制 mTLS、节点凭据一键签发/轮换/吊销、全量审计日志、登录限速
- **省心运维**：节点掉线检测 + Webhook 通知、统计数据自动清理、规则导入导出、Agent 断联后规则照常运行
- **主题**：液态玻璃暗色界面，管理员可在设置页自定义全局强调色，所有客户端实时跟进

技术栈：Rust（Axum + Tokio + SQLx + tonic）+ React 19 + SQLite（兼容 PostgreSQL）。

## 一键安装（推荐）

准备一台 Debian 12/13 的 VPS，root 执行：

```sh
curl -fsSL https://raw.githubusercontent.com/Remine1337/EMORELAY/master/deploy.sh | bash
```

按菜单选择「快速安装」即可：直接下载 GitHub Release 预编译静态二进制（amd64 / arm64），不需要 Rust/Node 工具链，约 1 分钟完成。装完后：

1. 浏览器打开 `http://<服务器IP>`，用安装时设置的管理员账号登录
2. 在「设置」页填写 Agent 上报端点（如 `https://<服务器IP>:50051`）
3. 「节点」页新建节点 → 复制安装命令 → 到目标服务器粘贴执行，节点即接入

再次运行同一脚本可进入 **升级 / 状态 / 备份 / 卸载** 菜单。脚本也提供 Docker Compose 与源码编译两种安装方式。

详细部署、反向代理与 HTTPS 配置见 [`docs/deployment.md`](./docs/deployment.md)。

## 其它运行方式

```sh
# 本机 Docker 一键启动
cp .env.example .env   # 设置 PANEL_JWT_SECRET 与管理员密码
docker compose up -d --build
# 打开 http://localhost
```

开发模式（cargo run + vite dev）与 Agent 本地调试见 [`docs/deployment.md`](./docs/deployment.md)。

## 文档

- [`docs/deployment.md`](./docs/deployment.md) — 部署与运维手册
- [`docs/api.md`](./docs/api.md) — REST + gRPC API 参考

## License

MIT（占位）。
