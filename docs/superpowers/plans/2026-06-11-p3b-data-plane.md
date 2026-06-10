# P3b 数据面 · 多跳隧道转发层 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 Agent 真正执行多跳隧道转发：`tunnel/` 模块（TCP/TLS/WSS 三 transport + entry/mid/exit `TunnelTask`）+ panel-server 真实 split+dispatch（含 reconcile）+ 隧道 TLS 凭据签发下发 + 隧道状态心跳聚合。

**Architecture:** Agent 收到带 `tunnel` 上下文的 Rule 时启动 `TunnelTask` 而非普通 relay：entry 监听业务端口、每连接 dial 下一跳并写 1 字节 stream preamble；mid 纯字节 bridge；exit 按 preamble 直连业务 target（TCP）或拆 UDP 帧。三 transport 实现统一 `TunnelTransport` trait（dial/bind/accept → boxed 字节流）。panel-server 把关联隧道的规则用既有 `split_tunnel_rule` 拆成 per-hop Rule 分发到各节点，reconcile 时同样按 hop 重放；tls/wss 隧道由内置 CA 即时签发 hop 凭据经 `Command.tunnel_credentials` 下发。隧道状态由 hop 节点心跳聚合（30s 窗口）。

**Tech Stack:** Rust（tokio relay + tokio-rustls/rustls 0.23 + tokio-tungstenite WSS + rcgen 签发）+ SQLx/SQLite。

**Spec:** `docs/superpowers/specs/2026-06-10-mvp-followups-design.md` §4.3/§4.6 + §5.1（UDP 帧/状态汇报风险条目）。控制面前置已交付（见 `2026-06-10-mvp-followups-phase-3.md` P3b 控制面段）。

---

## 关键设计决策（实现前必读）

1. **计量与限速只在 entry**：`split_tunnel_rule` 已把 mid/exit 的 `bandwidth_mbps` 置 0；同理 **rule_stats 只由 entry 计**（mid/exit 的 TunnelTask 不调 `stats.ensure`，不上报该 rule 的字节/连接数）。否则 server 端按 rule_id UPSERT 累加会把流量记成 N 倍（service.rs `report_rule_stats` 不区分来源节点）。
2. **连接级 1 字节 stream preamble**：entry dial 下一跳后先写 1 字节（`0x01`=TCP 业务流 / `0x02`=UDP 帧流），exit accept 后先读 1 字节决定模式；mid 不读内容纯 bridge。这让 `protocol=tcp_udp` 的规则也能走隧道（spec §4.6 未覆盖 tcp_udp 歧义，本计划补充）。
3. **UDP-over-tunnel**：per `client_addr` 一条独立隧道连接（NAT session 语义）；帧 = 2 字节大端长度前缀 + payload（spec §4.6）；session 闲置 120s 回收（与 `relay/udp.rs` 一致）；mid 对帧无感知。单包 ≤ 65507 字节天然放进 u16 长度。
4. **proto 补两个字段（向后兼容追加）**：`TunnelContext.self_ordinal = 7`（Agent 推导凭据目录与 dial SNI 需要本跳序号，控制面版漏配）；`TunnelCredentials.ca_pem = 7`（凭据自包含，dev plaintext 模式 Agent 没有控制面 CA 文件也能跑 TLS 隧道）。
5. **隧道证书不入 DB**：创建/restart/reconcile 时由内置 CA **即时重签**并下发。验证靠「链到 CA + SAN/SNI 匹配」，不 pin 具体证书，重签幂等无害。SAN/SNI = `tunnel-<id>-hop-<ordinal>.emorelay.internal`（dial 方 SNI 用下一跳 ordinal = `self_ordinal + 1`）。
6. **Agent 凭据落盘** `${AGENT_DATA_DIR}/tunnels/<tunnel_id>/hop-<ordinal>/{server.pem,server.key,client.pem,client.key,ca.pem}`，0600（Unix）。`AGENT_DATA_DIR` 是新 env，默认 `./agent-data`；install.sh 写 `/var/lib/emorelay`。`RevokeTunnelCredentials` 删除整个 `tunnels/<id>/` 目录。
7. **transport 形态**：trait object（`Arc<dyn TunnelTransport>`，conn = `Box<dyn AsyncRead+AsyncWrite+Send+Unpin>`），用 `#[tonic::async_trait]`（tonic 重导出 async-trait，零新增依赖）。TLS 握手在 `accept()` 内串行完成——hop 只被上一跳连入（信任域内），MVP 接受，注释说明。
7b. **WSS 不保证 TCP 半关（half-close）语义——接受并明示**：WebSocket 没有单方向关闭，`WsByteStream::poll_shutdown` 走 `poll_close`（发 Close frame 终结整条连接，保证资源释放、不泄漏），代价是「一端 shutdown 写半后，对端尚未发出的反向数据可能被截断」。依赖半关终止的业务流（如 HTTP/1.0 无 Content-Length 靠 FIN 界定 body）应选 tcp/tls transport。此限制写进 wss_transport.rs 模块注释（Task 7）与 docs/api.md（Task 9）。备选的「flush-only 不发 Close」方案会导致 EOF 永不传递、连接泄漏死锁，已否决。
8. **隧道 status** = hop 节点心跳聚合（spec §5.1：「最近 30s 内有 hop 心跳 = up」）：全部 hop 节点 `last_seen_at` 在 30s 内 → `up`；全部超窗 → `down`；部分 → `degraded`。`GET /api/tunnels/:id` 与 `/status` 实时计算并回写 `tunnels.status`；list 返回存储值。
9. **reconcile 重构为纯查询函数** `reconcile_commands_for_node(state, node_id) -> Vec<Command>`：非隧道规则 + 该节点参与的每个活跃隧道（凭据命令先行，再发本 hop 的拆分 Rule）。`subscribe_commands` 逐条 dispatch（unbounded channel FIFO 保证凭据先于规则到达）。
10. **dispatch 替换面（共 9 处）**：rules.rs ×5（create/update/delete/enable-disable/restart）、rules_io.rs ×2、bandwidth_profiles.rs ×1、user_quota.rs ×1 统一改走 `tunnel_dispatch::dispatch_rule_apply/_remove/_restart`（`tunnel_id=None` 时行为与原单节点 dispatch 完全等价）；service.rs reconcile 改走 `reconcile_commands_for_node`。
11. **被 dial 的 hop（ordinal ≥ 1）必须有 public_ip**：`POST /api/tunnels` 补校验，否则 split 出的 `next_hop_addr` 为空，entry dial 必败。
12. **实现前置**：Task 6/7 动手前先用 context7 查 `tokio-rustls 0.26`（rustls 0.23 重导出路径、`ring` provider feature）、`rustls-pemfile 2`、`tokio-tungstenite 0.24`（`Message::Binary` 载荷类型）的当前 API。本计划骨架是行为契约，**测试断言是验收标准**，实现体按真实 API 调整，不改测试断言语义。
13. 每个 Task 收尾：跑该 Task 测试 + `cargo test --workspace`，全绿 → commit → spawn `general-purpose` 子代理走 `superpowers:code-reviewer`（只读、三段式回报），通过才进下一 Task（CLAUDE.md 流程）。

## 文件结构（变更面）

**Create（Agent）：**
- `crates/node-agent/src/tunnel/mod.rs` — 模块声明 + `make_transport` 工厂
- `crates/node-agent/src/tunnel/transport.rs` — `TunnelTransport`/`TunnelListener` trait + `TunnelConn` 类型
- `crates/node-agent/src/tunnel/tcp_transport.rs` — 裸 TCP transport
- `crates/node-agent/src/tunnel/frame.rs` — stream preamble 常量 + UDP 帧读写
- `crates/node-agent/src/tunnel/task.rs` — `TunnelTask`（entry/mid/exit）
- `crates/node-agent/src/tunnel/creds.rs` — 隧道凭据落盘/清理
- `crates/node-agent/src/tunnel/tls_transport.rs` — TLS transport（rustls）
- `crates/node-agent/src/tunnel/wss_transport.rs` — WSS transport（tokio-tungstenite）+ `WsByteStream` 适配器
- `crates/node-agent/src/tunnel/testutil.rs` — `#[cfg(test)]` 测试证书签发 helper

**Create（panel-server）：**
- `crates/panel-server/src/grpc/tunnel_dispatch.rs` — split 下发 / 凭据下发 / reconcile 命令构造
- `crates/panel-server/tests/api_tunnel_dispatch.rs` — dispatch/reconcile/凭据下发集成测试

**Modify：**
- `crates/common/proto/control.proto` — `TunnelContext.self_ordinal=7`、`TunnelCredentials.ca_pem=7`
- `crates/common/tests/proto_tunnel.rs` — 构造同步 + 新字段断言
- `crates/panel-server/src/grpc/tunnel_split.rs` — split 填 `self_ordinal`
- `crates/panel-server/tests/tunnel_split.rs` — `self_ordinal` 断言
- `crates/panel-server/src/tls/issue.rs` — `issue_tunnel_hop_certs`（`rebuild_issuer_cert` 复用）
- `crates/panel-server/tests/tls_ca.rs` — 隧道证书签发测试
- `crates/panel-server/src/models/rule.rs` — `list_active_for_tunnel`
- `crates/panel-server/src/models/tunnel.rs` — `Tunnel::compute_status`/`set_status`、`TunnelHop::list_tunnel_ids_for_node`/`find_for_node`
- `crates/panel-server/src/routes/rules.rs`、`rules_io.rs`、`bandwidth_profiles.rs`、`sweeper/user_quota.rs` — dispatch 调用替换
- `crates/panel-server/src/routes/tunnels.rs` — create 校验 public_ip + 凭据下发；delete 吊销；restart 真实下发；status/get 聚合
- `crates/panel-server/src/grpc/service.rs` — reconcile 改走 `reconcile_commands_for_node`
- `crates/panel-server/src/grpc/mod.rs` — `pub mod tunnel_dispatch;`
- `crates/panel-server/tests/api_tunnels.rs` — status 聚合测试
- `crates/node-agent/Cargo.toml` — `tokio-rustls`/`rustls-pemfile`/`tokio-tungstenite`/`futures-util` + dev `rcgen`/`tempfile`
- `crates/node-agent/src/config.rs` — `data_dir`（`AGENT_DATA_DIR`）
- `crates/node-agent/src/store.rs` — `RuleJson.tunnel` 持久化
- `crates/node-agent/src/manager.rs` — tunnel Rule → `TunnelTask`
- `crates/node-agent/src/main.rs` — `mod tunnel;`、rustls provider 安装、`handle_command` 凭据分支、`RuleManager::new` 签名
- `crates/panel-server/src/routes/install.rs` + `tests/api_install.rs` — env 加 `AGENT_DATA_DIR`
- `.env.example`、`docs/api.md`、`README.md`、`plan.md`、`CLAUDE.md`、`docs/superpowers/plans/2026-06-10-mvp-followups-phase-3.md` — 文档收尾

---

## Task 1: proto 补字段 + Agent config/store 地基

**Files:**
- Modify: `crates/common/proto/control.proto`、`crates/common/tests/proto_tunnel.rs`
- Modify: `crates/panel-server/src/grpc/tunnel_split.rs`、`crates/panel-server/tests/tunnel_split.rs`
- Modify: `crates/node-agent/src/config.rs`、`crates/node-agent/src/store.rs`、`crates/node-agent/Cargo.toml`（dev 加 `tempfile = "3"`）

- [ ] **Step 1: 写失败测试（proto 新字段 + split 填值 + store round-trip）**

`crates/common/tests/proto_tunnel.rs` 的 `rule_carries_tunnel_context` 里 `TunnelContext` 字面量追加 `self_ordinal: 0,`，并在断言区追加：

```rust
    assert_eq!(r.tunnel.as_ref().unwrap().self_ordinal, 0);
```

`command_oneof_has_tunnel_credentials` 的 `TunnelCredentials` 字面量追加 `ca_pem: "CA".into(),`，断言区追加（在 `matches!` 之前解构取值或直接重新匹配）：

```rust
    if let Some(Body::TunnelCredentials(ref tc)) = c.body {
        assert_eq!(tc.ca_pem, "CA");
    } else {
        panic!("expected TunnelCredentials body");
    }
```

`crates/panel-server/tests/tunnel_split.rs` 两个测试追加断言：

```rust
    // two_hop_split_entry_and_exit 内:
    assert_eq!(t0.self_ordinal, 0);
    assert_eq!(t1.self_ordinal, 1);
    // three_hop_split_has_mid 内:
    assert_eq!(t_mid.self_ordinal, 1);
    assert_eq!(out[2].1.tunnel.as_ref().unwrap().self_ordinal, 2);
```

`crates/node-agent/src/store.rs` 文件尾加测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use emorelay_common::control::v1::{TunnelContext, TunnelRole};

    /// save → load 后 tunnel 上下文必须完整还原(P3b 数据面:Agent 断网重启要能恢复隧道角色)。
    #[tokio::test]
    async fn save_load_round_trips_tunnel_context() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("state.json");
        let store = ConfigStore::new(path);

        let rule = Rule {
            id: 5,
            protocol: "tcp".into(),
            listen_ip: "0.0.0.0".into(),
            listen_port: 20000,
            target_host: "9.9.9.9".into(),
            target_port: 443,
            enabled: true,
            bandwidth_mbps: 30,
            tunnel: Some(TunnelContext {
                tunnel_id: 7,
                role: TunnelRole::Mid as i32,
                next_hop_addr: "10.0.0.3".into(),
                next_hop_inter_port: 30002,
                self_inter_port: 30001,
                transport: "tls".into(),
                self_ordinal: 1,
            }),
        };
        store.save(&[rule.clone()]).await.unwrap();
        let loaded = store.load().await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].tunnel, rule.tunnel, "tunnel 上下文必须持久化");
    }

    /// 旧版 agent-state.json(无 tunnel 字段)必须能加载(serde default 兼容)。
    #[tokio::test]
    async fn load_legacy_state_without_tunnel_field() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("state.json");
        tokio::fs::write(
            &path,
            r#"[{"id":1,"protocol":"tcp","listen_ip":"0.0.0.0","listen_port":1000,
                "target_host":"1.1.1.1","target_port":80,"enabled":true,"bandwidth_mbps":0}]"#,
        )
        .await
        .unwrap();
        let loaded = ConfigStore::new(path).load().await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].tunnel.is_none());
    }
}
```

`crates/node-agent/Cargo.toml` 的 `[dev-dependencies]` 加 `tempfile = "3"`。

- [ ] **Step 2: 跑测试验证失败**

Run: `cargo test -p emorelay-common --test proto_tunnel`
Expected: 编译 FAIL（`TunnelContext` 无 `self_ordinal` 字段）。

- [ ] **Step 3: 改 proto**

`crates/common/proto/control.proto`：

`message TunnelContext` 在 `string transport = 6;` 之后加：

```proto
  // 本跳在链中的序号(0=entry)。TLS/WSS 凭据目录与 dial SNI(下一跳 = self_ordinal+1)推导用。
  uint32 self_ordinal = 7;
```

`message TunnelCredentials` 在 `string client_key_pem = 6;` 之后加：

```proto
  // 验证链根(面板内置 CA 公钥)。随凭据自包含下发,不依赖控制面 mTLS 是否启用/落盘。
  string ca_pem = 7;
```

- [ ] **Step 4: 同步所有 `TunnelContext {}` 构造点**

proto 加字段后以下构造处编译失败，逐处补：

- `crates/panel-server/src/grpc/tunnel_split.rs` 的 `TunnelContext { ... }` 构造加 `self_ordinal: i as u32,`（`i` 是 enumerate 序号，正是 ordinal——这是功能性填值，不是占位）。
- `crates/common/tests/proto_tunnel.rs`、`crates/panel-server/tests/tunnel_split.rs` 已在 Step 1 改。

- [ ] **Step 5: Agent config.rs 加 data_dir**

`crates/node-agent/src/config.rs` 的 `Config` struct 在 `state_path` 之后加：

```rust
    /// Agent 本地数据目录(隧道 TLS 凭据等)。隧道凭据落
    /// `${AGENT_DATA_DIR}/tunnels/<id>/hop-<ordinal>/`。
    pub data_dir: String,
```

`from_env()` 在 `state_path` 之后加：

```rust
        let data_dir = env::var("AGENT_DATA_DIR").unwrap_or_else(|_| "./agent-data".into());
```

struct 构造加 `data_dir,`。

- [ ] **Step 6: store.rs 持久化 tunnel**

`crates/node-agent/src/store.rs`：

文件头注释把「P3b 的 tunnel 上下文是有意例外」一段删掉（例外不再成立），改为「P3b 数据面起 tunnel 上下文随规则持久化,断网重启恢复隧道角色」。

加 `TunnelJson` 并扩展 `RuleJson`：

```rust
/// 镜像 proto TunnelContext(prost 类型未派生 Serialize)。
#[derive(Serialize, Deserialize, Clone)]
struct TunnelJson {
    tunnel_id: i64,
    role: i32,
    next_hop_addr: String,
    next_hop_inter_port: u32,
    self_inter_port: u32,
    transport: String,
    #[serde(default)]
    self_ordinal: u32,
}
```

`RuleJson` 加字段（`bandwidth_mbps` 之后）：

```rust
    /// P3b 数据面新增。`#[serde(default)]` 兼容旧版 agent-state.json(缺字段 → 非隧道规则)。
    #[serde(default)]
    tunnel: Option<TunnelJson>,
```

`From<&Rule> for RuleJson` 加：

```rust
            tunnel: r.tunnel.as_ref().map(|t| TunnelJson {
                tunnel_id: t.tunnel_id,
                role: t.role,
                next_hop_addr: t.next_hop_addr.clone(),
                next_hop_inter_port: t.next_hop_inter_port,
                self_inter_port: t.self_inter_port,
                transport: t.transport.clone(),
                self_ordinal: t.self_ordinal,
            }),
```

`From<RuleJson> for Rule` 把 `tunnel: None,`（及其注释）改为：

```rust
            tunnel: r.tunnel.map(|t| emorelay_common::control::v1::TunnelContext {
                tunnel_id: t.tunnel_id,
                role: t.role,
                next_hop_addr: t.next_hop_addr,
                next_hop_inter_port: t.next_hop_inter_port,
                self_inter_port: t.self_inter_port,
                transport: t.transport,
                self_ordinal: t.self_ordinal,
            }),
```

- [ ] **Step 7: 跑测试验证通过**

Run: `cargo test -p emorelay-common --test proto_tunnel && cargo test -p panel-server --test tunnel_split && cargo test -p node-agent && cargo test --workspace`
Expected: 全 PASS。

- [ ] **Step 8: Commit**

```bash
git add crates/common/proto/control.proto crates/common/tests/proto_tunnel.rs crates/panel-server/src/grpc/tunnel_split.rs crates/panel-server/tests/tunnel_split.rs crates/node-agent/src/config.rs crates/node-agent/src/store.rs crates/node-agent/Cargo.toml
git commit -m "feat(p3b): TunnelContext.self_ordinal + TunnelCredentials.ca_pem; agent persists tunnel context, AGENT_DATA_DIR"
```

---

## Task 2: TunnelTransport trait + TCP transport

**Files:**
- Create: `crates/node-agent/src/tunnel/mod.rs`、`crates/node-agent/src/tunnel/transport.rs`、`crates/node-agent/src/tunnel/tcp_transport.rs`
- Modify: `crates/node-agent/src/main.rs`（`mod tunnel;`）

- [ ] **Step 1: 写失败测试（tcp_transport.rs 文件内）**

`crates/node-agent/src/tunnel/tcp_transport.rs` 尾部：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::transport::TunnelTransport;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn tcp_transport_dial_bind_roundtrip() {
        let t = TcpTransport;
        let mut listener = t.bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local_addr");

        let server = tokio::spawn(async move {
            let mut conn = listener.accept().await.expect("accept");
            let mut buf = [0u8; 4];
            conn.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"ping");
            conn.write_all(b"pong").await.unwrap();
        });

        let mut conn = t.dial(&addr.to_string()).await.expect("dial");
        conn.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        conn.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"pong");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn make_transport_tcp_ok_unknown_rejected() {
        use emorelay_common::control::v1::TunnelContext;
        let ctx = |transport: &str| TunnelContext {
            tunnel_id: 1,
            role: 1,
            next_hop_addr: "127.0.0.1".into(),
            next_hop_inter_port: 1,
            self_inter_port: 0,
            transport: transport.into(),
            self_ordinal: 0,
        };
        assert!(crate::tunnel::make_transport(&ctx("tcp"), "./x").is_ok());
        assert!(crate::tunnel::make_transport(&ctx("quic"), "./x").is_err());
    }
}
```

- [ ] **Step 2: 跑测试验证失败**

Run: `cargo test -p node-agent tunnel`
Expected: 编译 FAIL（`tunnel` 模块不存在）。

- [ ] **Step 3: 实现 transport.rs + tcp_transport.rs + mod.rs**

`crates/node-agent/src/main.rs` 的 `mod` 声明区加 `mod tunnel;`（字母序，`store` 之后 `system` 之前不对——按现有顺序放 `stats`/`store` 之后：实际按字母序排进现有列表）。

`crates/node-agent/src/tunnel/transport.rs`：

```rust
//! 隧道 transport 抽象(P3b)。三实现(TCP/TLS/WSS)对上层 TunnelTask 同形:
//! dial 连下一跳、bind+accept 被上一跳连入,连接统一是 boxed 双向字节流。
use anyhow::Result;
use std::net::SocketAddr;
use tokio::io::{AsyncRead, AsyncWrite};

/// 隧道连接:双向字节流。TLS/WSS 在 transport 内完成握手,对上层透明。
pub trait TunnelStream: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> TunnelStream for T {}

pub type TunnelConn = Box<dyn TunnelStream>;

#[tonic::async_trait]
pub trait TunnelTransport: Send + Sync {
    /// 主动连下一跳(entry/mid)。addr 形如 "1.2.3.4:30001"。
    async fn dial(&self, addr: &str) -> Result<TunnelConn>;
    /// 监听被上一跳连入(mid/exit)。
    async fn bind(&self, addr: &str) -> Result<Box<dyn TunnelListener>>;
}

#[tonic::async_trait]
pub trait TunnelListener: Send {
    async fn accept(&mut self) -> Result<TunnelConn>;
    /// 实际监听地址(测试 bind :0 时取真实端口)。
    fn local_addr(&self) -> Result<SocketAddr>;
}
```

`crates/node-agent/src/tunnel/tcp_transport.rs`：

```rust
//! 裸 TCP transport(P3b)。无加密——仅适合内网/测试,生产推荐 tls/wss。
use anyhow::{Context, Result};
use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream};

use crate::tunnel::transport::{TunnelConn, TunnelListener, TunnelTransport};

pub struct TcpTransport;

#[tonic::async_trait]
impl TunnelTransport for TcpTransport {
    async fn dial(&self, addr: &str) -> Result<TunnelConn> {
        let s = TcpStream::connect(addr)
            .await
            .with_context(|| format!("tunnel tcp dial {addr}"))?;
        Ok(Box::new(s))
    }

    async fn bind(&self, addr: &str) -> Result<Box<dyn TunnelListener>> {
        let l = TcpListener::bind(addr)
            .await
            .with_context(|| format!("tunnel tcp bind {addr}"))?;
        Ok(Box::new(TcpTunnelListener { inner: l }))
    }
}

struct TcpTunnelListener {
    inner: TcpListener,
}

#[tonic::async_trait]
impl TunnelListener for TcpTunnelListener {
    async fn accept(&mut self) -> Result<TunnelConn> {
        let (s, _) = self.inner.accept().await.context("tunnel tcp accept")?;
        Ok(Box::new(s))
    }

    fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.inner.local_addr()?)
    }
}
```

`crates/node-agent/src/tunnel/mod.rs`：

```rust
//! 多跳隧道数据面(P3b)。模块边界:transport(链路) / task(角色编排) /
//! frame(线协议) / creds(凭据落盘)。RuleManager 是唯一调用入口。
pub mod tcp_transport;
pub mod transport;

use anyhow::Result;
use emorelay_common::control::v1::TunnelContext;
use std::sync::Arc;

use self::tcp_transport::TcpTransport;
use self::transport::TunnelTransport;

/// 按 TunnelContext.transport 构建 transport。data_dir 用于 tls/wss 读隧道凭据。
pub fn make_transport(ctx: &TunnelContext, data_dir: &str) -> Result<Arc<dyn TunnelTransport>> {
    let _ = data_dir; // tls/wss 落地(后续 Task)前未用。
    match ctx.transport.as_str() {
        "tcp" => Ok(Arc::new(TcpTransport)),
        "tls" => anyhow::bail!("tls tunnel transport not implemented yet (later task)"),
        "wss" => anyhow::bail!("wss tunnel transport not implemented yet (later task)"),
        other => anyhow::bail!("unknown tunnel transport: {other}"),
    }
}
```

- [ ] **Step 4: 跑测试验证通过**

Run: `cargo test -p node-agent tunnel && cargo test --workspace`
Expected: 全 PASS。

- [ ] **Step 5: Commit**

```bash
git add crates/node-agent/src/tunnel crates/node-agent/src/main.rs
git commit -m "feat(agent): TunnelTransport trait + plain TCP transport + make_transport factory"
```

---

## Task 3: frame.rs + TunnelTask（TCP 业务流，entry/mid/exit）

**Files:**
- Create: `crates/node-agent/src/tunnel/frame.rs`、`crates/node-agent/src/tunnel/task.rs`
- Modify: `crates/node-agent/src/tunnel/mod.rs`（`pub mod frame; pub mod task;`）

- [ ] **Step 1: 写失败测试**

`crates/node-agent/src/tunnel/frame.rs`（先建文件只放测试也无法编译——直接整文件按 Step 3 写，TDD 红灯由 task.rs 测试承担亦可；为保持纪律，frame 测试与实现同文件，先写测试 + 空实现签名让其编译失败）。实际操作：先写 `task.rs` 测试（引用未实现 API → 编译失败即红灯），frame 测试随实现一并写。

`crates/node-agent/src/tunnel/task.rs` 尾部测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::StatsCollector;
    use crate::tunnel::tcp_transport::TcpTransport;
    use emorelay_common::control::v1::{Rule, TunnelContext, TunnelRole};
    use std::net::TcpListener as StdTcpListener;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    fn ephemeral_port() -> u16 {
        StdTcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    async fn spawn_echo_server() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else { break };
                tokio::spawn(async move {
                    let (mut r, mut w) = socket.split();
                    let _ = tokio::io::copy(&mut r, &mut w).await;
                });
            }
        });
        port
    }

    /// 构造带 tunnel 上下文的 Rule。entry 监听 listen_port;mid/exit 监听 self_inter;
    /// exit 的 target 是业务目标。无关字段给 0/空。
    pub(super) fn tunnel_rule(
        role: TunnelRole,
        ordinal: u32,
        protocol: &str,
        listen_port: u16,
        target_port: u16,
        self_inter: u16,
        next_inter: u16,
    ) -> Rule {
        Rule {
            id: 42,
            protocol: protocol.into(),
            listen_ip: "127.0.0.1".into(),
            listen_port: listen_port as u32,
            target_host: "127.0.0.1".into(),
            target_port: target_port as u32,
            enabled: true,
            bandwidth_mbps: 0,
            tunnel: Some(TunnelContext {
                tunnel_id: 9,
                role: role as i32,
                next_hop_addr: "127.0.0.1".into(),
                next_hop_inter_port: next_inter as u32,
                self_inter_port: self_inter as u32,
                transport: "tcp".into(),
                self_ordinal: ordinal,
            }),
        }
    }

    #[tokio::test]
    async fn two_hop_tcp_roundtrip_counts_only_entry() {
        let echo = spawn_echo_server().await;
        let exit_port = ephemeral_port();
        let entry_port = ephemeral_port();
        let entry_stats = Arc::new(StatsCollector::new());
        let exit_stats = Arc::new(StatsCollector::new());
        let t: Arc<dyn crate::tunnel::transport::TunnelTransport> = Arc::new(TcpTransport);

        let exit = start(
            tunnel_rule(TunnelRole::Exit, 1, "tcp", 0, echo, exit_port, 0),
            exit_stats.clone(),
            None,
            t.clone(),
        )
        .await
        .expect("exit start");
        let entry = start(
            tunnel_rule(TunnelRole::Entry, 0, "tcp", entry_port, echo, 0, exit_port),
            entry_stats.clone(),
            None,
            t.clone(),
        )
        .await
        .expect("entry start");
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut conn = TcpStream::connect(("127.0.0.1", entry_port)).await.unwrap();
        conn.write_all(b"hello").await.unwrap();
        let mut buf = [0u8; 5];
        conn.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello");
        conn.shutdown().await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        entry.stop().await;
        exit.stop().await;

        let snap = entry_stats.drain_snapshot();
        let s = snap.iter().find(|s| s.rule_id == 42).expect("entry stats");
        assert_eq!(s.connection_count, 1);
        assert!(s.tx_bytes >= 5 && s.rx_bytes >= 5, "tx={} rx={}", s.tx_bytes, s.rx_bytes);
        // 计量只在 entry:exit 不得产生该 rule 的统计。
        assert!(
            exit_stats.drain_snapshot().is_empty(),
            "exit 不应计 rule stats(避免 server 端按 rule_id 重复累加)"
        );
    }

    #[tokio::test]
    async fn three_hop_tcp_roundtrip_via_mid() {
        let echo = spawn_echo_server().await;
        let exit_port = ephemeral_port();
        let mid_port = ephemeral_port();
        let entry_port = ephemeral_port();
        let stats = || Arc::new(StatsCollector::new());
        let t: Arc<dyn crate::tunnel::transport::TunnelTransport> = Arc::new(TcpTransport);

        let exit = start(
            tunnel_rule(TunnelRole::Exit, 2, "tcp", 0, echo, exit_port, 0),
            stats(), None, t.clone(),
        ).await.expect("exit");
        let mid = start(
            tunnel_rule(TunnelRole::Mid, 1, "tcp", 0, echo, mid_port, exit_port),
            stats(), None, t.clone(),
        ).await.expect("mid");
        let entry = start(
            tunnel_rule(TunnelRole::Entry, 0, "tcp", entry_port, echo, 0, mid_port),
            stats(), None, t.clone(),
        ).await.expect("entry");
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut conn = TcpStream::connect(("127.0.0.1", entry_port)).await.unwrap();
        conn.write_all(b"three-hop").await.unwrap();
        let mut buf = [0u8; 9];
        conn.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"three-hop");

        entry.stop().await;
        mid.stop().await;
        exit.stop().await;
    }

    #[tokio::test]
    async fn entry_stop_releases_listen_port() {
        let entry_port = ephemeral_port();
        let t: Arc<dyn crate::tunnel::transport::TunnelTransport> = Arc::new(TcpTransport);
        let entry = start(
            tunnel_rule(TunnelRole::Entry, 0, "tcp", entry_port, 1, 0, 1),
            Arc::new(StatsCollector::new()), None, t,
        ).await.expect("entry");
        tokio::time::sleep(Duration::from_millis(30)).await;
        entry.stop().await;
        TcpListener::bind(("127.0.0.1", entry_port))
            .await
            .expect("port should be released after stop");
    }
}
```

- [ ] **Step 2: 跑测试验证失败**

Run: `cargo test -p node-agent tunnel::task`
Expected: 编译 FAIL（`frame`/`task` 模块与 `start` 不存在）。

- [ ] **Step 3: 实现 frame.rs（含自测）**

```rust
//! 隧道线协议(P3b)。
//! - 连接级 1 字节 stream preamble:entry dial 后写,exit accept 后读;mid 不感知。
//!   0x01 = TCP 业务流(后续为裸字节);0x02 = UDP 帧流(后续为帧序列)。
//! - UDP 帧:2 字节大端长度前缀 + payload(spec §4.6)。单包 ≤65507 天然放进 u16。
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const STREAM_TCP: u8 = 0x01;
pub const STREAM_UDP: u8 = 0x02;

pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, payload: &[u8]) -> std::io::Result<()> {
    let len = u16::try_from(payload.len()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "udp payload > 65535")
    })?;
    w.write_all(&len.to_be_bytes()).await?;
    w.write_all(payload).await?;
    // WSS 等带缓冲 transport 需要 flush 把整帧推成一条消息。
    w.flush().await
}

/// 读一帧进 buf(覆盖式 resize),返回 payload 长度。EOF/对端关闭 → Err(UnexpectedEof)。
pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R, buf: &mut Vec<u8>) -> std::io::Result<usize> {
    let mut len_bytes = [0u8; 2];
    r.read_exact(&mut len_bytes).await?;
    let len = u16::from_be_bytes(len_bytes) as usize;
    buf.resize(len, 0);
    r.read_exact(buf).await?;
    Ok(len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frame_round_trip_including_empty() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        write_frame(&mut a, b"hello").await.unwrap();
        write_frame(&mut a, b"").await.unwrap();
        let mut buf = Vec::new();
        assert_eq!(read_frame(&mut b, &mut buf).await.unwrap(), 5);
        assert_eq!(&buf[..5], b"hello");
        assert_eq!(read_frame(&mut b, &mut buf).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn oversized_payload_rejected() {
        let (mut a, _b) = tokio::io::duplex(1024);
        let big = vec![0u8; 65536];
        assert!(write_frame(&mut a, &big).await.is_err());
    }
}
```

- [ ] **Step 4: 实现 task.rs（TCP 路径；UDP 留桩）**

```rust
//! TunnelTask(P3b 数据面)。per rule 一个实例,按 TunnelContext.role 三形态:
//! - entry: 监听业务 listen_port(按 protocol 起 TCP/UDP),每个 TCP 连接/UDP session
//!   dial 下一跳,先写 1 字节 stream preamble 再桥接。限速与 rule_stats 只在 entry 计。
//! - mid:   transport.bind(self_inter_port) → accept → dial 下一跳 → 纯字节 bridge
//!   (preamble 随流原样经过,不拆)。
//! - exit:  transport.bind(self_inter_port) → accept → 读 preamble:
//!   TCP → TcpStream::connect 业务 target 直连 bridge;UDP → 拆帧 ↔ UDP socket。
//! stop 语义与 relay/tcp.rs 一致:停 listener,存量连接自然跑完。
use anyhow::{Context as _, Result};
use emorelay_common::control::v1::{Rule, TunnelContext, TunnelRole};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::limit::TokenBucket;
use crate::stats::{RuleCounter, StatsCollector};
use crate::tunnel::frame::{STREAM_TCP, STREAM_UDP};
use crate::tunnel::transport::{TunnelConn, TunnelTransport};

pub struct TunnelTaskHandle {
    stop_tx: oneshot::Sender<()>,
    join: JoinHandle<()>,
}

impl TunnelTaskHandle {
    pub async fn stop(self) {
        let _ = self.stop_tx.send(());
        let _ = self.join.await;
    }
}

pub async fn start(
    rule: Rule,
    stats: Arc<StatsCollector>,
    bucket: Option<Arc<TokenBucket>>,
    transport: Arc<dyn TunnelTransport>,
) -> Result<TunnelTaskHandle> {
    let ctx = rule.tunnel.clone().context("rule has no tunnel context")?;
    match TunnelRole::try_from(ctx.role) {
        Ok(TunnelRole::Entry) => start_entry(rule, ctx, stats, bucket, transport).await,
        Ok(TunnelRole::Mid) => start_relay_hop(rule.id, ctx, transport, HopMode::Mid).await,
        Ok(TunnelRole::Exit) => {
            let target_port = u16::try_from(rule.target_port)
                .with_context(|| format!("target_port out of u16 range: {}", rule.target_port))?;
            start_relay_hop(
                rule.id,
                ctx,
                transport,
                HopMode::Exit { target_host: rule.target_host.clone(), target_port },
            )
            .await
        }
        _ => anyhow::bail!("unspecified tunnel role for rule {}", rule.id),
    }
}

// ============= entry =============

async fn start_entry(
    rule: Rule,
    ctx: TunnelContext,
    stats: Arc<StatsCollector>,
    bucket: Option<Arc<TokenBucket>>,
    transport: Arc<dyn TunnelTransport>,
) -> Result<TunnelTaskHandle> {
    let listen_ip: IpAddr = rule
        .listen_ip
        .parse()
        .with_context(|| format!("invalid listen_ip: {}", rule.listen_ip))?;
    let listen_port = u16::try_from(rule.listen_port)
        .with_context(|| format!("listen_port out of u16 range: {}", rule.listen_port))?;
    let addr = SocketAddr::new(listen_ip, listen_port);
    let next_hop = format!("{}:{}", ctx.next_hop_addr, ctx.next_hop_inter_port);
    let counter = stats.ensure(rule.id);
    let rule_id = rule.id;

    let want_tcp = matches!(rule.protocol.as_str(), "tcp" | "tcp_udp");
    let want_udp = matches!(rule.protocol.as_str(), "udp" | "tcp_udp");

    let tcp_listener = if want_tcp {
        Some(TcpListener::bind(addr).await.with_context(|| format!("bind {addr}"))?)
    } else {
        None
    };
    let udp_socket = if want_udp {
        Some(Arc::new(
            UdpSocket::bind(addr).await.with_context(|| format!("udp bind {addr}"))?,
        ))
    } else {
        None
    };
    info!(rule_id, %addr, tunnel_id = ctx.tunnel_id, "tunnel entry listening");

    let (stop_tx, mut stop_rx) = oneshot::channel::<()>();
    let join = tokio::spawn(async move {
        let tcp_loop = async {
            match tcp_listener {
                Some(l) => entry_tcp_loop(rule_id, l, &transport, &next_hop, &counter, &bucket).await,
                None => std::future::pending().await,
            }
        };
        let udp_loop = async {
            match udp_socket {
                Some(s) => entry_udp_loop(rule_id, s, &transport, &next_hop, &counter, &bucket).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = &mut stop_rx => info!(rule_id, "tunnel entry stopping"),
            _ = tcp_loop => warn!(rule_id, "tunnel entry tcp loop ended unexpectedly"),
            _ = udp_loop => warn!(rule_id, "tunnel entry udp loop ended unexpectedly"),
        }
    });
    Ok(TunnelTaskHandle { stop_tx, join })
}

async fn entry_tcp_loop(
    rule_id: i64,
    listener: TcpListener,
    transport: &Arc<dyn TunnelTransport>,
    next_hop: &str,
    counter: &Arc<RuleCounter>,
    bucket: &Option<Arc<TokenBucket>>,
) {
    loop {
        match listener.accept().await {
            Ok((client, peer)) => {
                counter.connection_count.fetch_add(1, Ordering::Relaxed);
                let transport = transport.clone();
                let next_hop = next_hop.to_string();
                let counter = counter.clone();
                let bucket = bucket.clone();
                tokio::spawn(async move {
                    if let Err(e) = entry_tcp_conn(client, transport, &next_hop, &counter, bucket).await {
                        counter.error_count.fetch_add(1, Ordering::Relaxed);
                        warn!(rule_id, %peer, error = ?e, "tunnel entry tcp bridge error");
                    }
                });
            }
            Err(e) => {
                counter.error_count.fetch_add(1, Ordering::Relaxed);
                warn!(rule_id, error = ?e, "tunnel entry accept error");
            }
        }
    }
}

async fn entry_tcp_conn(
    mut client: TcpStream,
    transport: Arc<dyn TunnelTransport>,
    next_hop: &str,
    counter: &Arc<RuleCounter>,
    bucket: Option<Arc<TokenBucket>>,
) -> Result<()> {
    let mut tunnel = transport.dial(next_hop).await?;
    tunnel.write_all(&[STREAM_TCP]).await.context("write stream preamble")?;
    tunnel.flush().await.context("flush stream preamble")?;
    let (mut c_r, mut c_w) = client.split();
    let (mut t_r, mut t_w) = tokio::io::split(tunnel);
    // 命名对齐 relay/tcp.rs:tx = client → 隧道(发出),rx = 隧道 → client。
    let c2t = copy_counted(&mut c_r, &mut t_w, bucket.as_deref(), &counter.tx_bytes);
    let t2c = copy_counted(&mut t_r, &mut c_w, bucket.as_deref(), &counter.rx_bytes);
    tokio::try_join!(c2t, t2c)?;
    Ok(())
}

/// 8KB chunk 复制 + 计数 + 可选限速。与 relay/tcp.rs::copy_limited 同构,多了
/// bucket 可选分支;未合并以不动既有 relay hot path。EOF 时半关写端。
async fn copy_counted<R, W>(
    r: &mut R,
    w: &mut W,
    bucket: Option<&TokenBucket>,
    counted: &std::sync::atomic::AtomicI64,
) -> std::io::Result<u64>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut buf = [0u8; 8192];
    let mut total = 0u64;
    loop {
        let n = r.read(&mut buf).await?;
        if n == 0 {
            let _ = w.shutdown().await;
            return Ok(total);
        }
        if let Some(b) = bucket {
            b.acquire(n).await;
        }
        w.write_all(&buf[..n]).await?;
        counted.fetch_add(n as i64, Ordering::Relaxed);
        total += n as u64;
    }
}

/// UDP-over-tunnel 在下一 Task 落地;此前 udp 流量不可达(规则仍可 apply)。
async fn entry_udp_loop(
    rule_id: i64,
    _socket: Arc<UdpSocket>,
    _transport: &Arc<dyn TunnelTransport>,
    _next_hop: &str,
    _counter: &Arc<RuleCounter>,
    _bucket: &Option<Arc<TokenBucket>>,
) {
    warn!(rule_id, "udp-over-tunnel not implemented yet; udp traffic dropped");
    std::future::pending::<()>().await;
}

// ============= mid / exit =============

#[derive(Clone)]
enum HopMode {
    Mid,
    Exit { target_host: String, target_port: u16 },
}

async fn start_relay_hop(
    rule_id: i64,
    ctx: TunnelContext,
    transport: Arc<dyn TunnelTransport>,
    mode: HopMode,
) -> Result<TunnelTaskHandle> {
    let bind_addr = format!("0.0.0.0:{}", ctx.self_inter_port);
    let mut listener = transport.bind(&bind_addr).await?;
    let next_hop = format!("{}:{}", ctx.next_hop_addr, ctx.next_hop_inter_port);
    info!(rule_id, %bind_addr, tunnel_id = ctx.tunnel_id, role = ctx.role, "tunnel hop listening");

    let (stop_tx, mut stop_rx) = oneshot::channel::<()>();
    let join = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut stop_rx => {
                    info!(rule_id, "tunnel hop stopping");
                    break;
                }
                res = listener.accept() => match res {
                    Ok(conn) => {
                        let transport = transport.clone();
                        let next_hop = next_hop.clone();
                        let mode = mode.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_hop_conn(conn, transport, &next_hop, mode).await {
                                warn!(rule_id, error = ?e, "tunnel hop conn error");
                            }
                        });
                    }
                    // TLS 握手失败也走这里:计 warn 继续 accept,不退出 loop。
                    Err(e) => warn!(rule_id, error = ?e, "tunnel hop accept error"),
                }
            }
        }
    });
    Ok(TunnelTaskHandle { stop_tx, join })
}

async fn handle_hop_conn(
    conn: TunnelConn,
    transport: Arc<dyn TunnelTransport>,
    next_hop: &str,
    mode: HopMode,
) -> Result<()> {
    match mode {
        HopMode::Mid => {
            // preamble 不拆,随字节流原样转发给下一跳。
            let upstream = transport.dial(next_hop).await?;
            bridge_raw(conn, upstream).await
        }
        HopMode::Exit { target_host, target_port } => {
            let mut conn = conn;
            let mut preamble = [0u8; 1];
            conn.read_exact(&mut preamble).await.context("read stream preamble")?;
            match preamble[0] {
                STREAM_TCP => {
                    let upstream = TcpStream::connect((target_host.as_str(), target_port))
                        .await
                        .with_context(|| format!("connect target {target_host}:{target_port}"))?;
                    bridge_raw(conn, Box::new(upstream)).await
                }
                STREAM_UDP => exit_udp_conn(conn, &target_host, target_port).await,
                other => anyhow::bail!("unknown stream preamble: {other:#04x}"),
            }
        }
    }
}

/// 双向纯字节复制(不计数不限速——计量只在 entry)。EOF 时半关写端。
async fn bridge_raw(a: TunnelConn, b: TunnelConn) -> Result<()> {
    let (mut a_r, mut a_w) = tokio::io::split(a);
    let (mut b_r, mut b_w) = tokio::io::split(b);
    let a2b = async {
        let n = tokio::io::copy(&mut a_r, &mut b_w).await;
        let _ = b_w.shutdown().await;
        n
    };
    let b2a = async {
        let n = tokio::io::copy(&mut b_r, &mut a_w).await;
        let _ = a_w.shutdown().await;
        n
    };
    tokio::try_join!(a2b, b2a)?;
    Ok(())
}

/// UDP-over-tunnel exit 端,下一 Task 实现。
async fn exit_udp_conn(_conn: TunnelConn, _target_host: &str, _target_port: u16) -> Result<()> {
    anyhow::bail!("udp-over-tunnel not implemented yet (next task)")
}
```

`tunnel/mod.rs` 加 `pub mod frame;`、`pub mod task;`（字母序）。

- [ ] **Step 5: 跑测试验证通过**

Run: `cargo test -p node-agent tunnel && cargo test --workspace`
Expected: 全 PASS（frame 2 测试 + task 3 测试 + 既有全绿）。

- [ ] **Step 6: Commit**

```bash
git add crates/node-agent/src/tunnel
git commit -m "feat(agent): TunnelTask entry/mid/exit with stream preamble; TCP business flow over tunnel"
```

---

## Task 4: UDP-over-tunnel（帧流 + entry session + exit 拆帧）

**Files:**
- Modify: `crates/node-agent/src/tunnel/task.rs`

- [ ] **Step 1: 写失败测试（追加 task.rs tests）**

```rust
    async fn spawn_udp_echo_server() -> u16 {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = socket.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            loop {
                let Ok((n, peer)) = socket.recv_from(&mut buf).await else { break };
                let _ = socket.send_to(&buf[..n], peer).await;
            }
        });
        port
    }

    #[tokio::test]
    async fn two_hop_udp_roundtrip_with_session_reuse() {
        let echo = spawn_udp_echo_server().await;
        let exit_port = ephemeral_port();
        let entry_port = ephemeral_port();
        let entry_stats = Arc::new(StatsCollector::new());
        let t: Arc<dyn crate::tunnel::transport::TunnelTransport> = Arc::new(TcpTransport);

        let exit = start(
            tunnel_rule(TunnelRole::Exit, 1, "udp", 0, echo, exit_port, 0),
            Arc::new(StatsCollector::new()), None, t.clone(),
        ).await.expect("exit");
        let entry = start(
            tunnel_rule(TunnelRole::Entry, 0, "udp", entry_port, echo, 0, exit_port),
            entry_stats.clone(), None, t.clone(),
        ).await.expect("entry");
        tokio::time::sleep(Duration::from_millis(50)).await;

        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let mut buf = [0u8; 64];
        // 同一 client 发两包:第二包复用 session,connection_count 应保持 1。
        for payload in [b"ping-1" as &[u8], b"ping-2"] {
            client.send_to(payload, ("127.0.0.1", entry_port)).await.unwrap();
            let (n, _) = tokio::time::timeout(
                Duration::from_millis(800),
                client.recv_from(&mut buf),
            )
            .await
            .expect("udp recv timed out")
            .unwrap();
            assert_eq!(&buf[..n], payload);
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
        entry.stop().await;
        exit.stop().await;

        let snap = entry_stats.drain_snapshot();
        let s = snap.iter().find(|s| s.rule_id == 42).expect("entry stats");
        assert_eq!(s.connection_count, 1, "同 client 两包应复用一条隧道 session");
        assert!(s.tx_bytes >= 12 && s.rx_bytes >= 12, "tx={} rx={}", s.tx_bytes, s.rx_bytes);
    }

    /// tcp_udp 协议:同一 entry 同时通 TCP 与 UDP(preamble 区分)。
    #[tokio::test]
    async fn tcp_udp_protocol_serves_both_over_tunnel() {
        let tcp_echo = spawn_echo_server().await;
        let exit_port = ephemeral_port();
        let entry_port = ephemeral_port();
        let t: Arc<dyn crate::tunnel::transport::TunnelTransport> = Arc::new(TcpTransport);

        // exit 的 udp 目标用同端口的 udp echo;tcp 目标用 tcp echo。
        // 简化:业务 target 都指向 tcp_echo 端口,UDP 单独再起 echo 并另建一对 task 验证
        // 会重复——这里只验证 TCP 流在 tcp_udp 协议下仍通,UDP 已由上个测试覆盖。
        let exit = start(
            tunnel_rule(TunnelRole::Exit, 1, "tcp_udp", 0, tcp_echo, exit_port, 0),
            Arc::new(StatsCollector::new()), None, t.clone(),
        ).await.expect("exit");
        let entry = start(
            tunnel_rule(TunnelRole::Entry, 0, "tcp_udp", entry_port, tcp_echo, 0, exit_port),
            Arc::new(StatsCollector::new()), None, t.clone(),
        ).await.expect("entry");
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut conn = TcpStream::connect(("127.0.0.1", entry_port)).await.unwrap();
        conn.write_all(b"dual").await.unwrap();
        let mut buf = [0u8; 4];
        conn.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"dual");

        entry.stop().await;
        exit.stop().await;
    }
```

- [ ] **Step 2: 跑测试验证失败**

Run: `cargo test -p node-agent tunnel::task`
Expected: `two_hop_udp_roundtrip_with_session_reuse` FAIL（udp 流量被丢，recv 超时 panic）。

- [ ] **Step 3: 实现 UDP 路径（替换 task.rs 两处桩）**

task.rs 顶部 import 区补：

```rust
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::{interval, Instant, MissedTickBehavior};

use crate::tunnel::frame::{read_frame, write_frame};
```

常量（`pub struct TunnelTaskHandle` 之前）：

```rust
/// UDP session 闲置回收阈值/扫描周期,与 relay/udp.rs 对齐。
const UDP_SESSION_TIMEOUT: Duration = Duration::from_secs(120);
const UDP_SWEEP_INTERVAL: Duration = Duration::from_secs(30);
const MAX_UDP_PACKET: usize = 65535;
```

替换 `entry_udp_loop` 桩为：

```rust
struct UdpTunnelSession {
    /// 主 loop → writer task 的入站包通道;drop 即关闭隧道连接(写半 shutdown)。
    frame_tx: mpsc::Sender<Vec<u8>>,
    last_seen: Instant,
}

/// per client_addr 一条隧道连接(NAT session 语义)。sessions 由本 loop 独占,无锁;
/// 过期 retain 丢弃 → frame_tx 关闭 → writer 退出 → 连接关 → reader EOF 退出,链式清理。
async fn entry_udp_loop(
    rule_id: i64,
    socket: Arc<UdpSocket>,
    transport: &Arc<dyn TunnelTransport>,
    next_hop: &str,
    counter: &Arc<RuleCounter>,
    bucket: &Option<Arc<TokenBucket>>,
) {
    let mut sessions: HashMap<SocketAddr, UdpTunnelSession> = HashMap::new();
    let mut buf = vec![0u8; MAX_UDP_PACKET];
    let mut sweep = interval(UDP_SWEEP_INTERVAL);
    sweep.set_missed_tick_behavior(MissedTickBehavior::Delay);
    sweep.tick().await;

    loop {
        tokio::select! {
            res = socket.recv_from(&mut buf) => match res {
                Ok((n, client_addr)) => {
                    // recv 即计 tx:被限速丢掉的包仍算"收到过"(与 relay/udp.rs 一致)。
                    counter.tx_bytes.fetch_add(n as i64, Ordering::Relaxed);
                    if let Some(b) = bucket {
                        if !b.try_acquire(n) {
                            counter.error_count.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                    }
                    if let Some(s) = sessions.get_mut(&client_addr) {
                        s.last_seen = Instant::now();
                        // writer 背压满 → 丢包计 error,不阻塞事件循环。
                        if s.frame_tx.try_send(buf[..n].to_vec()).is_err() {
                            counter.error_count.fetch_add(1, Ordering::Relaxed);
                        }
                        continue;
                    }
                    match open_udp_session(
                        rule_id, transport, next_hop, socket.clone(),
                        client_addr, counter.clone(), bucket.clone(),
                    ).await {
                        Ok(frame_tx) => {
                            counter.connection_count.fetch_add(1, Ordering::Relaxed);
                            let _ = frame_tx.try_send(buf[..n].to_vec());
                            sessions.insert(client_addr, UdpTunnelSession {
                                frame_tx,
                                last_seen: Instant::now(),
                            });
                        }
                        Err(e) => {
                            counter.error_count.fetch_add(1, Ordering::Relaxed);
                            warn!(rule_id, %client_addr, error = ?e, "open udp tunnel session failed");
                        }
                    }
                }
                Err(e) => {
                    counter.error_count.fetch_add(1, Ordering::Relaxed);
                    warn!(rule_id, error = ?e, "tunnel entry udp recv error");
                }
            },
            _ = sweep.tick() => {
                let now = Instant::now();
                sessions.retain(|_, s| now.duration_since(s.last_seen) <= UDP_SESSION_TIMEOUT);
            }
        }
    }
}

/// 建 session:dial → preamble 0x02 → split。writer:mpsc → write_frame;
/// reader:read_frame → send_to(client) + rx 计数(回程同样过桶,不足丢弃)。
async fn open_udp_session(
    rule_id: i64,
    transport: &Arc<dyn TunnelTransport>,
    next_hop: &str,
    listener: Arc<UdpSocket>,
    client_addr: SocketAddr,
    counter: Arc<RuleCounter>,
    bucket: Option<Arc<TokenBucket>>,
) -> Result<mpsc::Sender<Vec<u8>>> {
    let mut tunnel = transport.dial(next_hop).await?;
    tunnel.write_all(&[STREAM_UDP]).await.context("write stream preamble")?;
    tunnel.flush().await.context("flush stream preamble")?;
    let (mut t_r, mut t_w) = tokio::io::split(tunnel);
    let (frame_tx, mut frame_rx) = mpsc::channel::<Vec<u8>>(64);

    tokio::spawn(async move {
        while let Some(payload) = frame_rx.recv().await {
            if let Err(e) = write_frame(&mut t_w, &payload).await {
                warn!(rule_id, error = ?e, "udp tunnel write_frame error");
                break;
            }
        }
        let _ = t_w.shutdown().await;
    });

    tokio::spawn(async move {
        let mut fbuf = Vec::new();
        loop {
            match read_frame(&mut t_r, &mut fbuf).await {
                Ok(n) => {
                    counter.rx_bytes.fetch_add(n as i64, Ordering::Relaxed);
                    if let Some(b) = &bucket {
                        if !b.try_acquire(n) {
                            counter.error_count.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                    }
                    if let Err(e) = listener.send_to(&fbuf[..n], client_addr).await {
                        counter.error_count.fetch_add(1, Ordering::Relaxed);
                        warn!(rule_id, %client_addr, error = ?e, "udp send_to client error");
                        break;
                    }
                }
                Err(_) => break, // EOF/对端关闭(含 session 过期链式清理)。
            }
        }
    });

    Ok(frame_tx)
}
```

替换 `exit_udp_conn` 桩为：

```rust
/// exit 端 UDP 帧流:拆帧 → UDP send;UDP recv → 打帧回写。
/// 任一方向断(隧道 EOF / udp 错误)即结束,UDP socket 随之释放。
async fn exit_udp_conn(conn: TunnelConn, target_host: &str, target_port: u16) -> Result<()> {
    let udp = UdpSocket::bind("0.0.0.0:0").await.context("bind exit udp socket")?;
    udp.connect((target_host, target_port))
        .await
        .with_context(|| format!("connect udp target {target_host}:{target_port}"))?;
    let (mut t_r, mut t_w) = tokio::io::split(conn);

    let inbound = async {
        let mut fbuf = Vec::new();
        loop {
            let n = match read_frame(&mut t_r, &mut fbuf).await {
                Ok(n) => n,
                Err(_) => return,
            };
            if udp.send(&fbuf[..n]).await.is_err() {
                return;
            }
        }
    };
    let outbound = async {
        let mut buf = vec![0u8; MAX_UDP_PACKET];
        loop {
            let n = match udp.recv(&mut buf).await {
                Ok(n) => n,
                Err(_) => return,
            };
            if write_frame(&mut t_w, &buf[..n]).await.is_err() {
                return;
            }
        }
    };
    tokio::select! {
        _ = inbound => {}
        _ = outbound => {}
    }
    Ok(())
}
```

- [ ] **Step 4: 跑测试验证通过**

Run: `cargo test -p node-agent tunnel && cargo test --workspace`
Expected: 全 PASS。

- [ ] **Step 5: Commit**

```bash
git add crates/node-agent/src/tunnel/task.rs
git commit -m "feat(agent): UDP-over-tunnel with 2-byte length frames and per-client sessions"
```

---

## Task 5: RuleManager 接入（tunnel Rule → TunnelTask + 重启恢复）

**Files:**
- Modify: `crates/node-agent/src/manager.rs`、`crates/node-agent/src/main.rs`

- [ ] **Step 1: 写失败测试（manager.rs 尾部新增 tests 模块）**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use emorelay_common::control::v1::{TunnelContext, TunnelRole};
    use std::net::TcpListener as StdTcpListener;
    use std::time::Duration;
    use tokio::net::TcpListener;

    fn ephemeral_port() -> u16 {
        StdTcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
    }

    fn entry_tunnel_rule(listen_port: u16) -> Rule {
        Rule {
            id: 77,
            protocol: "tcp".into(),
            listen_ip: "127.0.0.1".into(),
            listen_port: listen_port as u32,
            target_host: "127.0.0.1".into(),
            target_port: 1,
            enabled: true,
            bandwidth_mbps: 0,
            tunnel: Some(TunnelContext {
                tunnel_id: 3,
                role: TunnelRole::Entry as i32,
                next_hop_addr: "127.0.0.1".into(),
                next_hop_inter_port: 1,
                self_inter_port: 0,
                transport: "tcp".into(),
                self_ordinal: 0,
            }),
        }
    }

    /// 带 tunnel 的 Rule 走 TunnelTask:apply 占用 listen 端口,remove 释放。
    #[tokio::test]
    async fn apply_tunnel_rule_starts_task_and_remove_releases_port() {
        let stats = Arc::new(StatsCollector::new());
        let mut mgr = RuleManager::new(stats, "./unused-data-dir".into());
        let port = ephemeral_port();

        mgr.apply(entry_tunnel_rule(port)).await.expect("apply tunnel rule");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(mgr.current_rules().len(), 1);
        assert!(
            TcpListener::bind(("127.0.0.1", port)).await.is_err(),
            "entry 应占用业务端口"
        );

        mgr.remove(77).await;
        TcpListener::bind(("127.0.0.1", port))
            .await
            .expect("remove 后端口应释放");
    }

    /// 未知 transport(tls 未落地阶段同理)→ apply 报错,不留半启动状态。
    #[tokio::test]
    async fn apply_tunnel_rule_with_unknown_transport_errors() {
        let stats = Arc::new(StatsCollector::new());
        let mut mgr = RuleManager::new(stats, "./unused".into());
        let mut rule = entry_tunnel_rule(ephemeral_port());
        rule.tunnel.as_mut().unwrap().transport = "quic".into();
        assert!(mgr.apply(rule).await.is_err());
        assert!(mgr.current_rules().is_empty());
    }
}
```

- [ ] **Step 2: 跑测试验证失败**

Run: `cargo test -p node-agent manager`
Expected: 编译 FAIL（`RuleManager::new` 还是单参数）。

- [ ] **Step 3: 实现 manager.rs 扩展**

`RuleHandles` 加字段与 stop：

```rust
use crate::tunnel::task::TunnelTaskHandle;

#[derive(Default)]
struct RuleHandles {
    rule: Option<Rule>,
    tcp: Option<TcpRelayHandle>,
    udp: Option<UdpRelayHandle>,
    tunnel: Option<TunnelTaskHandle>,
}

impl RuleHandles {
    async fn stop_all(mut self) {
        if let Some(h) = self.tcp.take() {
            h.stop().await;
        }
        if let Some(h) = self.udp.take() {
            h.stop().await;
        }
        if let Some(h) = self.tunnel.take() {
            h.stop().await;
        }
    }
}
```

`RuleManager` 加 `data_dir`：

```rust
pub struct RuleManager {
    handles: HashMap<i64, RuleHandles>,
    stats: Arc<StatsCollector>,
    /// 隧道凭据根目录(AGENT_DATA_DIR);tls/wss transport 从这里读证书。
    data_dir: String,
}

impl RuleManager {
    pub fn new(stats: Arc<StatsCollector>, data_dir: String) -> Self {
        Self {
            handles: HashMap::new(),
            stats,
            data_dir,
        }
    }
```

`apply` 在 `if !rule.enabled` 块之后、`let mut bundle` 之前插入隧道分支：

```rust
        // P3b:带 tunnel 上下文 → TunnelTask(entry/mid/exit),不走普通 relay。
        if let Some(ctx) = rule.tunnel.as_ref() {
            // split 已保证仅 entry 的 bandwidth_mbps 非 0,mid/exit 自然拿 None。
            let bucket = crate::limit::TokenBucket::from_mbps(rule.bandwidth_mbps);
            let transport = crate::tunnel::make_transport(ctx, &self.data_dir)?;
            let handle =
                crate::tunnel::task::start(rule.clone(), self.stats.clone(), bucket, transport)
                    .await?;
            self.handles.insert(
                rule.id,
                RuleHandles {
                    rule: Some(rule),
                    tunnel: Some(handle),
                    ..Default::default()
                },
            );
            return Ok(());
        }
```

- [ ] **Step 4: main.rs 同步**

`crates/node-agent/src/main.rs` 的 `RuleManager::new(stats.clone())` 改为：

```rust
    let manager = Arc::new(Mutex::new(RuleManager::new(
        stats.clone(),
        config.data_dir.clone(),
    )));
```

（重启恢复链 `store.load → mgr.apply` 现状已覆盖隧道规则——Task 1 的 store 持久化 + 本 Task 的 apply 分支共同生效，无需新代码。）

- [ ] **Step 5: 跑测试验证通过**

Run: `cargo test -p node-agent && cargo test --workspace`
Expected: 全 PASS。

- [ ] **Step 6: Commit**

```bash
git add crates/node-agent/src/manager.rs crates/node-agent/src/main.rs
git commit -m "feat(agent): RuleManager routes tunnel rules to TunnelTask; restores tunnel roles after restart"
```

---

## Task 6: TLS——隧道证书签发 + Agent 凭据落盘 + TLS transport

**Files:**
- Modify: `crates/panel-server/src/tls/issue.rs`、`crates/panel-server/tests/tls_ca.rs`
- Create: `crates/node-agent/src/tunnel/creds.rs`、`crates/node-agent/src/tunnel/tls_transport.rs`、`crates/node-agent/src/tunnel/testutil.rs`
- Modify: `crates/node-agent/src/tunnel/mod.rs`、`crates/node-agent/src/main.rs`、`crates/node-agent/Cargo.toml`

> **实现前置**：先 context7 查 `tokio-rustls 0.26` / `rustls 0.23`（重导出路径 `tokio_rustls::rustls`、`pki_types`、`WebPkiClientVerifier`、crypto provider feature）与 `rustls-pemfile 2`。骨架按 rustls 0.23 写；若 API 形态不符，以「测试通过」为准调整，不改测试断言语义。crypto provider 用 **ring**（与 tonic 0.12 的 tls 栈对齐，避免拉入 aws-lc-rs）。

- [ ] **Step 1: Cargo.toml 加依赖**

`crates/node-agent/Cargo.toml` `[dependencies]` 加：

```toml
# P3b 隧道 TLS/WSS transport。ring provider 与 tonic 0.12 tls 栈对齐。
tokio-rustls = { version = "0.26", default-features = false, features = ["ring"] }
rustls-pemfile = "2"
```

`[dev-dependencies]` 加：

```toml
# 隧道 TLS 测试用自签证书。
rcgen = "0.13"
```

- [ ] **Step 2: 写失败测试**

`crates/panel-server/tests/tls_ca.rs` 追加：

```rust
use panel_server::tls::issue::issue_tunnel_hop_certs;

#[test]
fn issue_tunnel_hop_certs_produces_server_and_client_pairs() {
    let dir = TempDir::new().unwrap();
    let ca = bootstrap_ca(&tls_dir(&dir), None).unwrap();
    let c = issue_tunnel_hop_certs(&ca, 7, 1).expect("issue tunnel hop certs");
    assert!(c.server_cert_pem.contains("BEGIN CERTIFICATE"));
    assert!(c.client_cert_pem.contains("BEGIN CERTIFICATE"));
    for key in [&c.server_key_pem, &c.client_key_pem] {
        assert!(key.contains("BEGIN PRIVATE KEY") || key.contains("BEGIN EC PRIVATE KEY"));
    }
    // server 与 client 是两张独立叶子。
    assert_ne!(c.server_cert_pem, c.client_cert_pem);
}
```

`crates/node-agent/src/tunnel/creds.rs` 尾部（文件按 Step 4 创建，测试随实现同文件——红灯由编译失败承担）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use emorelay_common::control::v1::TunnelCredentials;

    #[tokio::test]
    async fn store_writes_five_files_and_remove_cleans_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let data_dir = dir.path().display().to_string();
        let c = TunnelCredentials {
            tunnel_id: 7,
            ordinal: 1,
            server_cert_pem: "S".into(),
            server_key_pem: "SK".into(),
            client_cert_pem: "C".into(),
            client_key_pem: "CK".into(),
            ca_pem: "CA".into(),
        };
        store(&data_dir, &c).await.expect("store");
        let hop = hop_dir(&data_dir, 7, 1);
        for f in ["server.pem", "server.key", "client.pem", "client.key", "ca.pem"] {
            assert!(hop.join(f).exists(), "missing {f}");
        }
        remove_tunnel(&data_dir, 7).await.expect("remove");
        assert!(!hop.exists());
        // 幂等:再删不存在的目录不报错。
        remove_tunnel(&data_dir, 7).await.expect("remove twice");
    }
}
```

`crates/node-agent/src/tunnel/tls_transport.rs` 尾部测试（同上随实现创建）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::testutil::write_hop_creds_pair;
    use crate::tunnel::transport::TunnelTransport;
    use emorelay_common::control::v1::TunnelContext;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn ctx(tunnel_id: i64, ordinal: u32) -> TunnelContext {
        TunnelContext {
            tunnel_id,
            role: 0,
            next_hop_addr: String::new(),
            next_hop_inter_port: 0,
            self_inter_port: 0,
            transport: "tls".into(),
            self_ordinal: ordinal,
        }
    }

    /// hop-0(dial,SNI=hop-1) ↔ hop-1(accept,server SAN=hop-1):双向 mTLS 通,字节往返。
    #[tokio::test]
    async fn tls_transport_roundtrip_with_mutual_auth() {
        let dir = tempfile::TempDir::new().unwrap();
        let data_dir = dir.path().display().to_string();
        write_hop_creds_pair(&data_dir, 9, 0, 1).await;

        let server_t = TlsTransport::load(&data_dir, &ctx(9, 1)).expect("server load");
        let client_t = TlsTransport::load(&data_dir, &ctx(9, 0)).expect("client load");

        let mut listener = server_t.bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut conn = listener.accept().await.expect("tls accept");
            let mut buf = [0u8; 6];
            conn.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"secret");
            conn.write_all(b"shhh").await.unwrap();
        });

        let mut conn = client_t.dial(&addr.to_string()).await.expect("tls dial");
        conn.write_all(b"secret").await.unwrap();
        let mut buf = [0u8; 4];
        conn.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"shhh");
        server.await.unwrap();
    }

    /// 非 TLS 裸连必须被拒(握手失败即 accept Err;「合法 TLS 但无 client cert」
    /// 场景由 rustls WebPkiClientVerifier 强制,P3c e2e 真链路再覆盖)。
    #[tokio::test]
    async fn tls_transport_rejects_non_tls_client() {
        let dir = tempfile::TempDir::new().unwrap();
        let data_dir = dir.path().display().to_string();
        write_hop_creds_pair(&data_dir, 9, 0, 1).await;
        let server_t = TlsTransport::load(&data_dir, &ctx(9, 1)).unwrap();
        let mut listener = server_t.bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // 裸 TCP 连入后立刻关闭(不做 TLS):accept 端握手必失败。
        let client = tokio::spawn(async move {
            let _ = tokio::net::TcpStream::connect(addr).await.unwrap();
        });
        assert!(listener.accept().await.is_err(), "无 TLS/无 client cert 必须被拒");
        let _ = client.await;
    }
}
```

- [ ] **Step 3: 跑测试验证失败**

Run: `cargo test -p panel-server --test tls_ca`
Expected: 编译 FAIL（`issue_tunnel_hop_certs` 不存在）。

- [ ] **Step 4: 实现 panel-server issue.rs**

`crates/panel-server/src/tls/issue.rs` 追加（`rebuild_issuer_cert` 保持私有即可，同文件可调）：

```rust
/// 隧道 hop 的 TLS 凭据(P3b 数据面)。server/client 各一张叶子,
/// SAN 同为 tunnel-<id>-hop-<ordinal>.emorelay.internal(dial 方 SNI 校验 server SAN;
/// client 叶子链验证即可,SAN 不参与 server 端校验)。不入 DB,即时签发即时下发。
pub struct TunnelHopCerts {
    pub server_cert_pem: String,
    pub server_key_pem: String,
    pub client_cert_pem: String,
    pub client_key_pem: String,
}

pub fn issue_tunnel_hop_certs(ca: &CaBundle, tunnel_id: i64, ordinal: i64) -> Result<TunnelHopCerts> {
    let san = format!("tunnel-{tunnel_id}-hop-{ordinal}.emorelay.internal");
    let issuer_key = KeyPair::from_pem(&ca.ca_key_pem).context("从 PEM 重建 CA 私钥失败")?;
    let issuer_cert = rebuild_issuer_cert(&issuer_key).context("重建 issuer 证书失败")?;

    let (server_cert_pem, server_key_pem) =
        issue_tunnel_leaf(&san, ExtendedKeyUsagePurpose::ServerAuth, &issuer_cert, &issuer_key)?;
    let (client_cert_pem, client_key_pem) =
        issue_tunnel_leaf(&san, ExtendedKeyUsagePurpose::ClientAuth, &issuer_cert, &issuer_key)?;

    Ok(TunnelHopCerts {
        server_cert_pem,
        server_key_pem,
        client_cert_pem,
        client_key_pem,
    })
}

fn issue_tunnel_leaf(
    san: &str,
    eku: ExtendedKeyUsagePurpose,
    issuer_cert: &Certificate,
    issuer_key: &KeyPair,
) -> Result<(String, String)> {
    let key = KeyPair::generate().context("生成隧道叶子密钥失败")?;
    let now = OffsetDateTime::now_utc();
    let mut params = CertificateParams::new(vec![san.to_string()])
        .context("构造隧道叶子 CertificateParams 失败")?;
    params.is_ca = IsCa::NoCa;
    params.distinguished_name.push(DnType::CommonName, san);
    params.key_usages.push(KeyUsagePurpose::DigitalSignature);
    params.extended_key_usages.push(eku);
    params.serial_number = Some(SerialNumber::from(rand::random::<u64>() | 1));
    params.use_authority_key_identifier_extension = true;
    params.not_before = now - Duration::days(1);
    params.not_after = now + Duration::days(1825);
    let cert = params
        .signed_by(&key, issuer_cert, issuer_key)
        .context("CA 签发隧道叶子失败")?;
    Ok((cert.pem(), key.serialize_pem()))
}
```

- [ ] **Step 5: 实现 Agent creds.rs**

```rust
//! 隧道 TLS 凭据落盘(P3b)。布局:
//! ${AGENT_DATA_DIR}/tunnels/<tunnel_id>/hop-<ordinal>/{server.pem,server.key,client.pem,client.key,ca.pem}
//! 0600(Unix;Windows 跳过权限)。store 幂等覆盖(reconcile 重签重发);
//! RevokeTunnelCredentials 删整个 tunnels/<id>/。
use anyhow::{Context, Result};
use emorelay_common::control::v1::TunnelCredentials;
use std::path::{Path, PathBuf};

pub fn hop_dir(data_dir: &str, tunnel_id: i64, ordinal: u32) -> PathBuf {
    Path::new(data_dir)
        .join("tunnels")
        .join(tunnel_id.to_string())
        .join(format!("hop-{ordinal}"))
}

pub async fn store(data_dir: &str, c: &TunnelCredentials) -> Result<()> {
    let ordinal = u32::try_from(c.ordinal).context("negative tunnel hop ordinal")?;
    let dir = hop_dir(data_dir, c.tunnel_id, ordinal);
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("create {}", dir.display()))?;
    for (name, content) in [
        ("server.pem", &c.server_cert_pem),
        ("server.key", &c.server_key_pem),
        ("client.pem", &c.client_cert_pem),
        ("client.key", &c.client_key_pem),
        ("ca.pem", &c.ca_pem),
    ] {
        let path = dir.join(name);
        tokio::fs::write(&path, content)
            .await
            .with_context(|| format!("write {name}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .await
                .with_context(|| format!("chmod 0600 {name}"))?;
        }
    }
    Ok(())
}

pub async fn remove_tunnel(data_dir: &str, tunnel_id: i64) -> Result<()> {
    let dir = Path::new(data_dir).join("tunnels").join(tunnel_id.to_string());
    match tokio::fs::remove_dir_all(&dir).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).context("remove tunnel creds dir"),
    }
}
```

（测试见 Step 2。）

- [ ] **Step 6: 实现 testutil.rs（测试证书 helper，tls/wss 测试共用）**

`crates/node-agent/src/tunnel/testutil.rs`：

```rust
//! 测试专用:rcgen 自签 CA + 为相邻两 hop 写凭据目录(模拟 TunnelCredentials 落盘)。
#![cfg(test)]
use emorelay_common::control::v1::TunnelCredentials;
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose,
};

fn make_ca() -> (KeyPair, Certificate) {
    let key = KeyPair::generate().unwrap();
    let mut p = CertificateParams::new(Vec::new()).unwrap();
    p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    p.distinguished_name.push(DnType::CommonName, "test-tunnel-ca");
    let cert = p.self_signed(&key).unwrap();
    (key, cert)
}

fn issue_leaf(
    san: &str,
    eku: ExtendedKeyUsagePurpose,
    ca_cert: &Certificate,
    ca_key: &KeyPair,
) -> (String, String) {
    let key = KeyPair::generate().unwrap();
    let mut p = CertificateParams::new(vec![san.to_string()]).unwrap();
    p.is_ca = IsCa::NoCa;
    p.key_usages.push(KeyUsagePurpose::DigitalSignature);
    p.extended_key_usages.push(eku);
    let cert = p.signed_by(&key, ca_cert, ca_key).unwrap();
    (cert.pem(), key.serialize_pem())
}

/// 为 (tunnel_id, ordinal_a/ordinal_b) 两个相邻 hop 各写一套凭据(同一 CA)。
/// server SAN 按各自 hop;dial 方 SNI = 对端 hop,链验证 + SAN 校验都能过。
pub async fn write_hop_creds_pair(data_dir: &str, tunnel_id: i64, ordinal_a: u32, ordinal_b: u32) {
    // 单测不经 main.rs:显式装 ring provider(幂等,重复安装返回 Err 忽略),
    // 避免依赖图日后混入第二个 provider 时 rustls builder panic。
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    let (ca_key, ca_cert) = make_ca();
    let ca_pem = ca_cert.pem();
    for ordinal in [ordinal_a, ordinal_b] {
        let san = format!("tunnel-{tunnel_id}-hop-{ordinal}.emorelay.internal");
        let (server_cert_pem, server_key_pem) =
            issue_leaf(&san, ExtendedKeyUsagePurpose::ServerAuth, &ca_cert, &ca_key);
        let (client_cert_pem, client_key_pem) =
            issue_leaf(&san, ExtendedKeyUsagePurpose::ClientAuth, &ca_cert, &ca_key);
        crate::tunnel::creds::store(
            data_dir,
            &TunnelCredentials {
                tunnel_id,
                ordinal: ordinal as i32,
                server_cert_pem,
                server_key_pem,
                client_cert_pem,
                client_key_pem,
                ca_pem: ca_pem.clone(),
            },
        )
        .await
        .expect("write test hop creds");
    }
}
```

- [ ] **Step 7: 实现 tls_transport.rs**

```rust
//! TLS transport(P3b)。隧道 TLS 与控制面 mTLS 复用同一内置 CA;凭据由
//! Command.tunnel_credentials 下发落盘(creds.rs)。dial 方强制 SNI =
//! tunnel-<id>-hop-<self_ordinal+1>.emorelay.internal——身份验证用 SNI/SAN,
//! next_hop_addr 只用于路由。server 端 WebPkiClientVerifier 强制 client cert
//! 链到同 CA。TLS 握手在 accept() 内串行完成:hop 仅被上一跳(信任域内)连入,
//! 恶意半开连接风险低,MVP 接受。
use anyhow::{Context, Result};
use emorelay_common::control::v1::TunnelContext;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use tokio_rustls::rustls::server::WebPkiClientVerifier;
use tokio_rustls::rustls::{ClientConfig, RootCertStore, ServerConfig};
use tokio_rustls::{TlsAcceptor, TlsConnector};

use crate::tunnel::creds::hop_dir;
use crate::tunnel::transport::{TunnelConn, TunnelListener, TunnelTransport};

pub struct TlsTransport {
    pub(crate) connector: TlsConnector,
    pub(crate) acceptor: TlsAcceptor,
    pub(crate) dial_sni: ServerName<'static>,
}

impl TlsTransport {
    pub fn load(data_dir: &str, ctx: &TunnelContext) -> Result<Self> {
        let dir = hop_dir(data_dir, ctx.tunnel_id, ctx.self_ordinal);

        let mut roots = RootCertStore::empty();
        for cert in load_certs(&dir.join("ca.pem"))? {
            roots.add(cert).context("add tunnel ca root")?;
        }
        let roots = Arc::new(roots);

        let client_cfg = ClientConfig::builder()
            .with_root_certificates(roots.clone())
            .with_client_auth_cert(
                load_certs(&dir.join("client.pem"))?,
                load_key(&dir.join("client.key"))?,
            )
            .context("build tunnel tls client config")?;

        let verifier = WebPkiClientVerifier::builder(roots)
            .build()
            .context("build tunnel client cert verifier")?;
        let server_cfg = ServerConfig::builder()
            .with_client_cert_verifier(verifier)
            .with_single_cert(
                load_certs(&dir.join("server.pem"))?,
                load_key(&dir.join("server.key"))?,
            )
            .context("build tunnel tls server config")?;

        let sni = format!(
            "tunnel-{}-hop-{}.emorelay.internal",
            ctx.tunnel_id,
            ctx.self_ordinal + 1
        );
        Ok(Self {
            connector: TlsConnector::from(Arc::new(client_cfg)),
            acceptor: TlsAcceptor::from(Arc::new(server_cfg)),
            dial_sni: ServerName::try_from(sni).context("invalid tunnel sni")?,
        })
    }
}

fn load_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>> {
    let pem = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    rustls_pemfile::certs(&mut pem.as_slice())
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("parse certs in {}", path.display()))
}

fn load_key(path: &Path) -> Result<PrivateKeyDer<'static>> {
    let pem = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    rustls_pemfile::private_key(&mut pem.as_slice())
        .with_context(|| format!("parse key in {}", path.display()))?
        .with_context(|| format!("no private key in {}", path.display()))
}

#[tonic::async_trait]
impl TunnelTransport for TlsTransport {
    async fn dial(&self, addr: &str) -> Result<TunnelConn> {
        let tcp = TcpStream::connect(addr)
            .await
            .with_context(|| format!("tunnel tls tcp connect {addr}"))?;
        let tls = self
            .connector
            .connect(self.dial_sni.clone(), tcp)
            .await
            .context("tunnel tls client handshake")?;
        Ok(Box::new(tls))
    }

    async fn bind(&self, addr: &str) -> Result<Box<dyn TunnelListener>> {
        let l = TcpListener::bind(addr)
            .await
            .with_context(|| format!("tunnel tls bind {addr}"))?;
        Ok(Box::new(TlsTunnelListener {
            inner: l,
            acceptor: self.acceptor.clone(),
        }))
    }
}

struct TlsTunnelListener {
    inner: TcpListener,
    acceptor: TlsAcceptor,
}

#[tonic::async_trait]
impl TunnelListener for TlsTunnelListener {
    async fn accept(&mut self) -> Result<TunnelConn> {
        let (tcp, _) = self.inner.accept().await.context("tunnel tls tcp accept")?;
        let tls = self
            .acceptor
            .accept(tcp)
            .await
            .context("tunnel tls server handshake")?;
        Ok(Box::new(tls))
    }

    fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.inner.local_addr()?)
    }
}
```

- [ ] **Step 8: 接线（mod.rs / main.rs）**

`tunnel/mod.rs`：

```rust
pub mod creds;
pub mod frame;
pub mod task;
pub mod tcp_transport;
#[cfg(test)]
pub mod testutil;
pub mod tls_transport;
pub mod transport;
```

`make_transport` 的 `"tls"` 分支改为：

```rust
        "tls" => Ok(Arc::new(tls_transport::TlsTransport::load(data_dir, ctx)?)),
```

`crates/node-agent/src/main.rs`：

`main()` 开头（tracing init 之后）安装 rustls provider（rustls 0.23 多 provider 并存时必须显式选择）：

```rust
    // 隧道 TLS/WSS 用 ring provider(与 tonic tls 栈对齐)。重复安装无害,忽略结果。
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
```

`handle_command` 签名加 `data_dir: &str`，调用处改 `handle_command(&manager, &store, cmd, &config.data_dir)`。函数体里把 `let Some(body) = cmd.body else ...` 之后改为先拦凭据命令（不进 manager 锁）：

```rust
    // 凭据命令只动磁盘,不动规则状态,不进 manager 锁。
    let body = match body {
        Body::TunnelCredentials(c) => {
            info!(tunnel_id = c.tunnel_id, ordinal = c.ordinal, "tunnel credentials received");
            if let Err(e) = crate::tunnel::creds::store(data_dir, &c).await {
                warn!(error = ?e, "store tunnel credentials failed");
            }
            return Ok(());
        }
        Body::RevokeTunnelCredentials(c) => {
            info!(tunnel_id = c.tunnel_id, "tunnel credentials revoked");
            if let Err(e) = crate::tunnel::creds::remove_tunnel(data_dir, c.tunnel_id).await {
                warn!(error = ?e, "remove tunnel credentials failed");
            }
            return Ok(());
        }
        other => other,
    };
```

（原 match 里的 `Body::TunnelCredentials`/`Body::RevokeTunnelCredentials` 占位分支删除——所有变体已被上面拦截，余下 match 只剩 5 个规则分支。）

- [ ] **Step 9: 跑测试验证通过**

Run: `cargo test -p panel-server --test tls_ca && cargo test -p node-agent && cargo test --workspace`
Expected: 全 PASS（含 TLS roundtrip / 拒非 TLS 连入 / creds 落盘清理）。

- [ ] **Step 10: Commit**

```bash
git add crates/panel-server/src/tls/issue.rs crates/panel-server/tests/tls_ca.rs crates/node-agent/src/tunnel crates/node-agent/src/main.rs crates/node-agent/Cargo.toml
git commit -m "feat(tls): tunnel hop cert issuance; agent stores credentials and speaks mutual TLS transport"
```

---

## Task 7: WSS transport（WebSocket over TLS）

**Files:**
- Create: `crates/node-agent/src/tunnel/wss_transport.rs`
- Modify: `crates/node-agent/src/tunnel/mod.rs`、`crates/node-agent/Cargo.toml`

> **实现前置**：context7 查 `tokio-tungstenite 0.24`（`client_async`/`accept_async` 签名、`Message::Binary` 载荷类型——0.24 是 `Vec<u8>`，更新版本可能是 `Bytes`，按实际调整 `.into()`）。TLS 层完全复用 TlsTransport 的 connector/acceptor/SNI，tungstenite 只做 ws 协议层（不开它的 tls feature）。

- [ ] **Step 1: Cargo.toml 加依赖**

`crates/node-agent/Cargo.toml` `[dependencies]` 加：

```toml
# P3b WSS transport:ws 协议层 over 自管 TLS 流(不用 tungstenite 的 tls feature)。
tokio-tungstenite = { version = "0.24", default-features = false, features = ["handshake"] }
futures-util = { version = "0.3", default-features = false, features = ["sink", "std"] }
```

- [ ] **Step 2: 写失败测试（wss_transport.rs 尾部，随实现创建——红灯由编译失败承担）**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::testutil::write_hop_creds_pair;
    use crate::tunnel::transport::TunnelTransport;
    use emorelay_common::control::v1::TunnelContext;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn ctx(ordinal: u32) -> TunnelContext {
        TunnelContext {
            tunnel_id: 9,
            role: 0,
            next_hop_addr: String::new(),
            next_hop_inter_port: 0,
            self_inter_port: 0,
            transport: "wss".into(),
            self_ordinal: ordinal,
        }
    }

    #[tokio::test]
    async fn wss_transport_roundtrip() {
        let dir = tempfile::TempDir::new().unwrap();
        let data_dir = dir.path().display().to_string();
        write_hop_creds_pair(&data_dir, 9, 0, 1).await;

        let server_t = WssTransport::load(&data_dir, &ctx(1)).expect("server load");
        let client_t = WssTransport::load(&data_dir, &ctx(0)).expect("client load");

        let mut listener = server_t.bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut conn = listener.accept().await.expect("wss accept");
            let mut buf = [0u8; 5];
            conn.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"hello");
            conn.write_all(b"world").await.unwrap();
            // 显式 flush:WsByteStream 写入是消息缓冲语义。
            conn.flush().await.unwrap();
        });

        let mut conn = client_t.dial(&addr.to_string()).await.expect("wss dial");
        conn.write_all(b"hello").await.unwrap();
        conn.flush().await.unwrap();
        let mut buf = [0u8; 5];
        conn.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"world");
        server.await.unwrap();
    }

    /// 大 payload(单次 write_all → 单条大 Binary 消息)完整往返;
    /// tungstenite 默认 max_message_size(64MB)远大于 256KB。
    #[tokio::test]
    async fn wss_transport_large_payload() {
        let dir = tempfile::TempDir::new().unwrap();
        let data_dir = dir.path().display().to_string();
        write_hop_creds_pair(&data_dir, 9, 0, 1).await;
        let server_t = WssTransport::load(&data_dir, &ctx(1)).unwrap();
        let client_t = WssTransport::load(&data_dir, &ctx(0)).unwrap();

        let mut listener = server_t.bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let payload = vec![0xCD_u8; 256 * 1024];
        let expect = payload.clone();
        let server = tokio::spawn(async move {
            let mut conn = listener.accept().await.unwrap();
            let mut buf = vec![0u8; expect.len()];
            conn.read_exact(&mut buf).await.unwrap();
            assert_eq!(buf, expect);
        });

        let mut conn = client_t.dial(&addr.to_string()).await.unwrap();
        conn.write_all(&payload).await.unwrap();
        conn.flush().await.unwrap();
        // 半关写端让对端 read_exact 后不会悬挂在 EOF 判定上。
        conn.shutdown().await.unwrap();
        server.await.unwrap();
    }
}
```

- [ ] **Step 3: 跑测试验证失败**

Run: `cargo test -p node-agent tunnel::wss`
Expected: 编译 FAIL（`wss_transport` 不存在）。

- [ ] **Step 4: 实现 wss_transport.rs**

```rust
//! WSS transport(P3b):WebSocket over TLS。TLS 配置/SNI 复用 TlsTransport,
//! tungstenite 只做 ws 协议层。WsByteStream 把 Binary message 流适配成
//! AsyncRead/AsyncWrite:write → 一条 Binary;read → 按序消费 Binary 载荷;
//! Ping/Pong 由 tungstenite 自动应答,Text 忽略,Close/流终止 → EOF。
//!
//! **限制:WebSocket 没有 TCP 半关(half-close)**。poll_shutdown 发 Close frame
//! 终结整条连接(保证资源释放),一端 shutdown 写半后对端尚未发出的反向数据可能
//! 被截断。依赖半关终止的业务流(HTTP/1.0 靠 FIN 界定 body 等)请选 tcp/tls。
use anyhow::{Context, Result};
use emorelay_common::control::v1::TunnelContext;
use futures_util::{Sink, Stream};
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{accept_async, client_async, WebSocketStream};

use crate::tunnel::tls_transport::TlsTransport;
use crate::tunnel::transport::{TunnelConn, TunnelListener, TunnelTransport};

pub struct WssTransport {
    tls: TlsTransport,
    /// client_async 只用 URL 写 Host/路径,TLS 已在下层完成,故 scheme 用 ws://。
    dial_url: String,
}

impl WssTransport {
    pub fn load(data_dir: &str, ctx: &TunnelContext) -> Result<Self> {
        let tls = TlsTransport::load(data_dir, ctx)?;
        let sni = format!(
            "tunnel-{}-hop-{}.emorelay.internal",
            ctx.tunnel_id,
            ctx.self_ordinal + 1
        );
        Ok(Self { tls, dial_url: format!("ws://{sni}/tunnel") })
    }
}

#[tonic::async_trait]
impl TunnelTransport for WssTransport {
    async fn dial(&self, addr: &str) -> Result<TunnelConn> {
        let tcp = TcpStream::connect(addr)
            .await
            .with_context(|| format!("tunnel wss tcp connect {addr}"))?;
        let tls = self
            .tls
            .connector
            .connect(self.tls.dial_sni.clone(), tcp)
            .await
            .context("tunnel wss tls handshake")?;
        let (ws, _resp) = client_async(self.dial_url.as_str(), tls)
            .await
            .context("tunnel ws client handshake")?;
        Ok(Box::new(WsByteStream::new(ws)))
    }

    async fn bind(&self, addr: &str) -> Result<Box<dyn TunnelListener>> {
        let l = TcpListener::bind(addr)
            .await
            .with_context(|| format!("tunnel wss bind {addr}"))?;
        Ok(Box::new(WssTunnelListener {
            inner: l,
            acceptor: self.tls.acceptor.clone(),
        }))
    }
}

struct WssTunnelListener {
    inner: TcpListener,
    acceptor: tokio_rustls::TlsAcceptor,
}

#[tonic::async_trait]
impl TunnelListener for WssTunnelListener {
    async fn accept(&mut self) -> Result<TunnelConn> {
        let (tcp, _) = self.inner.accept().await.context("tunnel wss tcp accept")?;
        let tls = self
            .acceptor
            .accept(tcp)
            .await
            .context("tunnel wss tls handshake")?;
        let ws = accept_async(tls).await.context("tunnel ws server handshake")?;
        Ok(Box::new(WsByteStream::new(ws)))
    }

    fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.inner.local_addr()?)
    }
}

// ============= WsByteStream =============

fn to_io(e: tokio_tungstenite::tungstenite::Error) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e)
}

struct WsByteStream<S> {
    inner: WebSocketStream<S>,
    read_buf: Vec<u8>,
    read_pos: usize,
}

impl<S> WsByteStream<S> {
    fn new(inner: WebSocketStream<S>) -> Self {
        Self { inner, read_buf: Vec::new(), read_pos: 0 }
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send> AsyncRead for WsByteStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            if self.read_pos < self.read_buf.len() {
                let n = (self.read_buf.len() - self.read_pos).min(buf.remaining());
                let pos = self.read_pos;
                buf.put_slice(&self.read_buf[pos..pos + n]);
                self.read_pos += n;
                return Poll::Ready(Ok(()));
            }
            match Pin::new(&mut self.inner).poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                // 流终止 / 对端 Close → EOF(空读)。
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Ready(Some(Ok(Message::Binary(data)))) => {
                    self.read_buf = data.into();
                    self.read_pos = 0;
                }
                Poll::Ready(Some(Ok(Message::Close(_)))) => return Poll::Ready(Ok(())),
                // Ping/Pong 由 tungstenite 自动应答;Text/Frame 对字节流无意义,跳过。
                Poll::Ready(Some(Ok(_))) => continue,
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Err(to_io(e))),
            }
        }
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send> AsyncWrite for WsByteStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        match Pin::new(&mut self.inner).poll_ready(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(e)) => Poll::Ready(Err(to_io(e))),
            Poll::Ready(Ok(())) => {
                Pin::new(&mut self.inner)
                    .start_send(Message::Binary(data.to_vec().into()))
                    .map_err(to_io)?;
                Poll::Ready(Ok(data.len()))
            }
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx).map_err(to_io)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_close(cx).map_err(to_io)
    }
}
```

（若 tokio-tungstenite 0.24 的 `Message::Binary` 载荷是 `Vec<u8>`，`data.into()` 与 `data.to_vec().into()` 直接成立；若是 `Bytes`，同样 `into()` 可达——按编译结果微调，不动测试。）

- [ ] **Step 5: 接线 mod.rs**

`tunnel/mod.rs` 加 `pub mod wss_transport;`（字母序末尾）；`make_transport` 的 `"wss"` 分支改为：

```rust
        "wss" => Ok(Arc::new(wss_transport::WssTransport::load(data_dir, ctx)?)),
```

- [ ] **Step 6: 跑测试验证通过**

Run: `cargo test -p node-agent tunnel && cargo test --workspace`
Expected: 全 PASS。

- [ ] **Step 7: Commit**

```bash
git add crates/node-agent/src/tunnel crates/node-agent/Cargo.toml
git commit -m "feat(agent): WSS tunnel transport with WsByteStream adapter over mutual TLS"
```

---

## Task 8: server 真实下发（split dispatch + 凭据下发 + reconcile）

**Files:**
- Create: `crates/panel-server/src/grpc/tunnel_dispatch.rs`、`crates/panel-server/tests/api_tunnel_dispatch.rs`
- Modify: `crates/panel-server/src/grpc/mod.rs`、`crates/panel-server/src/grpc/service.rs`
- Modify: `crates/panel-server/src/models/rule.rs`、`crates/panel-server/src/models/tunnel.rs`
- Modify: `crates/panel-server/src/routes/rules.rs`、`rules_io.rs`、`bandwidth_profiles.rs`、`crates/panel-server/src/sweeper/user_quota.rs`、`crates/panel-server/src/routes/tunnels.rs`

- [ ] **Step 1: 写失败测试 `crates/panel-server/tests/api_tunnel_dispatch.rs`**

```rust
mod common;

use axum::http::Method;
use emorelay_common::control::v1::{command::Body, TunnelRole};
use serde_json::json;

/// 建 N 个 online 节点(带 public_ip + port_pool),返回 ids。
async fn seed_online_nodes(app: &common::TestApp, n: usize) -> Vec<i64> {
    let mut ids = Vec::new();
    for i in 0..n {
        let id = sqlx::query(
            "INSERT INTO nodes (name, agent_token_hash, status, public_ip, port_pool_min, port_pool_max) \
             VALUES (?, 'x', 'online', ?, 30000, 30010)",
        )
        .bind(format!("dn{i}"))
        .bind(format!("10.1.0.{i}"))
        .execute(&app.state.pool)
        .await
        .unwrap()
        .last_insert_rowid();
        ids.push(id);
    }
    ids
}

async fn create_tunnel(app: &common::TestApp, transport: &str, nodes: &[i64]) -> i64 {
    let req = common::auth_req(Method::POST, "/api/tunnels", &app.admin_token,
        Some(json!({ "name": format!("t-{transport}-{}", nodes.len()), "transport": transport, "node_ids": nodes }))).unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    body["id"].as_i64().unwrap()
}

#[tokio::test]
async fn create_rule_on_tunnel_dispatches_per_hop_split_rules() {
    let app = common::make_app().await.unwrap();
    let nodes = seed_online_nodes(&app, 3).await;
    // 模拟三个 Agent 在线。
    let mut rxs: Vec<_> = nodes.iter().map(|n| app.state.dispatcher.subscribe(*n).0).collect();
    let tid = create_tunnel(&app, "tcp", &nodes).await;

    let req = common::auth_req(Method::POST, "/api/rules", &app.admin_token,
        Some(json!({ "node_id": nodes[0], "name": "r", "protocol": "tcp", "listen_port": 20000,
                     "target_host": "9.9.9.9", "target_port": 443, "tunnel_id": tid }))).unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    let rule_id = body["id"].as_i64().unwrap();

    // 每个 hop 节点都收到一条带 tunnel 上下文的 ApplyRule。
    let expected_roles = [TunnelRole::Entry, TunnelRole::Mid, TunnelRole::Exit];
    for (i, rx) in rxs.iter_mut().enumerate() {
        let cmd = rx.try_recv().expect("hop should receive ApplyRule");
        let Some(Body::ApplyRule(apply)) = cmd.body else { panic!("expected ApplyRule") };
        let rule = apply.rule.expect("rule");
        assert_eq!(rule.id, rule_id);
        let t = rule.tunnel.expect("tunnel context");
        assert_eq!(t.role, expected_roles[i] as i32);
        assert_eq!(t.self_ordinal, i as u32);
        if i == 0 {
            assert_eq!(rule.listen_port, 20000);
            assert_eq!(t.next_hop_addr, "10.1.0.1");
        }
        if i > 0 {
            assert!(t.self_inter_port >= 30000, "mid/exit 监听 inter_port");
        }
    }
}

#[tokio::test]
async fn tls_tunnel_create_dispatches_credentials_to_each_hop() {
    let app = common::make_app().await.unwrap();
    let nodes = seed_online_nodes(&app, 2).await;
    let mut rxs: Vec<_> = nodes.iter().map(|n| app.state.dispatcher.subscribe(*n).0).collect();
    let _tid = create_tunnel(&app, "tls", &nodes).await;

    for (i, rx) in rxs.iter_mut().enumerate() {
        let cmd = rx.try_recv().expect("hop should receive TunnelCredentials");
        let Some(Body::TunnelCredentials(c)) = cmd.body else { panic!("expected TunnelCredentials") };
        assert_eq!(c.ordinal, i as i32);
        assert!(c.server_cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(c.client_cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(c.ca_pem.contains("BEGIN CERTIFICATE"), "凭据必须自包含 CA");
    }
}

#[tokio::test]
async fn delete_rule_and_tunnel_dispatch_remove_and_revoke() {
    let app = common::make_app().await.unwrap();
    let nodes = seed_online_nodes(&app, 2).await;
    let tid = create_tunnel(&app, "tls", &nodes).await;
    let req = common::auth_req(Method::POST, "/api/rules", &app.admin_token,
        Some(json!({ "node_id": nodes[0], "name": "r", "protocol": "tcp", "listen_port": 20000,
                     "target_host": "9.9.9.9", "target_port": 443, "tunnel_id": tid }))).unwrap();
    let (_, body) = common::send(app.app.clone(), req).await.unwrap();
    let rule_id = body["id"].as_i64().unwrap();

    // 规则/隧道建好后再上线订阅(只关心 delete 阶段的命令)。
    let mut rxs: Vec<_> = nodes.iter().map(|n| app.state.dispatcher.subscribe(*n).0).collect();

    let req = common::auth_req(Method::DELETE, &format!("/api/rules/{rule_id}"), &app.admin_token, None).unwrap();
    let (status, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, axum::http::StatusCode::OK);
    for rx in rxs.iter_mut() {
        let cmd = rx.try_recv().expect("hop should receive RemoveRule");
        let Some(Body::RemoveRule(r)) = cmd.body else { panic!("expected RemoveRule") };
        assert_eq!(r.rule_id, rule_id);
    }

    let req = common::auth_req(Method::DELETE, &format!("/api/tunnels/{tid}"), &app.admin_token, None).unwrap();
    let (status, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, axum::http::StatusCode::OK);
    for rx in rxs.iter_mut() {
        let cmd = rx.try_recv().expect("hop should receive RevokeTunnelCredentials");
        let Some(Body::RevokeTunnelCredentials(r)) = cmd.body else { panic!("expected Revoke") };
        assert_eq!(r.tunnel_id, tid);
    }
}

#[tokio::test]
async fn reconcile_replays_tunnel_hop_rules_with_credentials_first() {
    let app = common::make_app().await.unwrap();
    let nodes = seed_online_nodes(&app, 2).await;
    let tid = create_tunnel(&app, "tls", &nodes).await;
    let req = common::auth_req(Method::POST, "/api/rules", &app.admin_token,
        Some(json!({ "node_id": nodes[0], "name": "r", "protocol": "tcp", "listen_port": 20000,
                     "target_host": "9.9.9.9", "target_port": 443, "tunnel_id": tid }))).unwrap();
    let (_, _body) = common::send(app.app.clone(), req).await.unwrap();
    // 非隧道规则也挂在 exit 节点上,确认一并 reconcile。
    sqlx::query(
        "INSERT INTO forward_rules (user_id, node_id, name, protocol, listen_ip, listen_port, target_host, target_port) \
         VALUES (?, ?, 'plain', 'tcp', '0.0.0.0', 30005, '1.1.1.1', 80)",
    ).bind(app.admin_user_id).bind(nodes[1])
    .execute(&app.state.pool).await.unwrap();

    // exit 节点(mid 链路上无规则行)reconcile:凭据先行,再是本 hop 拆分 Rule + 非隧道规则。
    let cmds = panel_server::grpc::tunnel_dispatch::reconcile_commands_for_node(&app.state, nodes[1])
        .await
        .expect("reconcile");
    let mut saw_creds_at = None;
    let mut saw_hop_rule_at = None;
    let mut saw_plain_at = None;
    for (i, cmd) in cmds.iter().enumerate() {
        match &cmd.body {
            Some(Body::TunnelCredentials(c)) if c.tunnel_id == tid => saw_creds_at = Some(i),
            Some(Body::ApplyRule(a)) => {
                let r = a.rule.as_ref().unwrap();
                if let Some(t) = &r.tunnel {
                    assert_eq!(t.role, TunnelRole::Exit as i32, "exit 节点只该拿 exit 份");
                    saw_hop_rule_at = Some(i);
                } else {
                    saw_plain_at = Some(i);
                }
            }
            _ => {}
        }
    }
    let creds = saw_creds_at.expect("reconcile 必须含隧道凭据");
    let hop = saw_hop_rule_at.expect("reconcile 必须含本 hop 拆分 Rule");
    assert!(creds < hop, "凭据必须先于隧道规则下发");
    saw_plain_at.expect("非隧道规则也要 reconcile");
}

#[tokio::test]
async fn restart_tunnel_redispatches_credentials_and_restarts_rules() {
    let app = common::make_app().await.unwrap();
    let nodes = seed_online_nodes(&app, 2).await;
    let tid = create_tunnel(&app, "tls", &nodes).await;
    let req = common::auth_req(Method::POST, "/api/rules", &app.admin_token,
        Some(json!({ "node_id": nodes[0], "name": "r", "protocol": "tcp", "listen_port": 20000,
                     "target_host": "9.9.9.9", "target_port": 443, "tunnel_id": tid }))).unwrap();
    let (_, body) = common::send(app.app.clone(), req).await.unwrap();
    let rule_id = body["id"].as_i64().unwrap();

    let mut rxs: Vec<_> = nodes.iter().map(|n| app.state.dispatcher.subscribe(*n).0).collect();
    let req = common::auth_req(Method::POST, &format!("/api/tunnels/{tid}/restart"), &app.admin_token, None).unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(body["dispatched"], true);

    for rx in rxs.iter_mut() {
        // 凭据 + 该规则的 restart,顺序:凭据先。
        let c1 = rx.try_recv().expect("credentials");
        assert!(matches!(c1.body, Some(Body::TunnelCredentials(_))));
        let c2 = rx.try_recv().expect("restart");
        let Some(Body::RestartRule(r)) = c2.body else { panic!("expected RestartRule") };
        assert_eq!(r.rule_id, rule_id);
    }
}

#[tokio::test]
async fn create_tunnel_rejects_dialed_hop_without_public_ip() {
    let app = common::make_app().await.unwrap();
    let n1 = seed_online_nodes(&app, 1).await[0];
    let bare = sqlx::query(
        "INSERT INTO nodes (name, agent_token_hash, status, public_ip, port_pool_min, port_pool_max) \
         VALUES ('noip', 'x', 'online', '', 30000, 30010)",
    ).execute(&app.state.pool).await.unwrap().last_insert_rowid();
    let req = common::auth_req(Method::POST, "/api/tunnels", &app.admin_token,
        Some(json!({ "name": "noip-t", "transport": "tcp", "node_ids": [n1, bare] }))).unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST, "{body}");
    assert!(body["message"].as_str().unwrap().contains("public_ip"));
}
```

（`common::TestApp` 的字段/HELPER 以 `tests/common/mod.rs` 现状为准——`admin_user_id`、`auth_req`、`send` 均已存在于既有测试。`rx.try_recv()` 是 `mpsc::UnboundedReceiver::try_recv`。）

- [ ] **Step 2: 跑测试验证失败**

Run: `cargo test -p panel-server --test api_tunnel_dispatch`
Expected: 编译 FAIL（`grpc::tunnel_dispatch` 不存在）。

- [ ] **Step 3: model 新查询**

`crates/panel-server/src/models/rule.rs` 在 `list_active_for_node` 之后加：

```rust
    /// 关联某隧道的全部活跃规则(数据面 split 下发/reconcile 用)。
    pub async fn list_active_for_tunnel(
        pool: &SqlitePool,
        tunnel_id: i64,
    ) -> sqlx::Result<Vec<Self>> {
        let sql = format!(
            "SELECT {RULE_COLUMNS} FROM forward_rules WHERE tunnel_id = ? AND deleted_at IS NULL ORDER BY id"
        );
        sqlx::query_as::<_, Rule>(&sql).bind(tunnel_id).fetch_all(pool).await
    }
```

`crates/panel-server/src/models/tunnel.rs` 的 `impl TunnelHop` 加：

```rust
    /// 该节点参与的活跃隧道 id 列表(reconcile 用)。
    pub async fn list_tunnel_ids_for_node(pool: &SqlitePool, node_id: i64) -> sqlx::Result<Vec<i64>> {
        sqlx::query_scalar(
            "SELECT DISTINCT t.id FROM tunnel_hops th \
             JOIN tunnels t ON t.id = th.tunnel_id \
             WHERE th.node_id = ? AND t.deleted_at IS NULL ORDER BY t.id",
        )
        .bind(node_id)
        .fetch_all(pool)
        .await
    }

    /// 该节点在指定隧道里的 hop 行(凭据 ordinal 用)。
    pub async fn find_for_node(
        pool: &SqlitePool,
        tunnel_id: i64,
        node_id: i64,
    ) -> sqlx::Result<Option<Self>> {
        let sql = format!(
            "SELECT {HOP_COLS} FROM tunnel_hops WHERE tunnel_id = ? AND node_id = ? LIMIT 1"
        );
        sqlx::query_as(&sql).bind(tunnel_id).bind(node_id).fetch_optional(pool).await
    }
```

- [ ] **Step 4: 实现 grpc/tunnel_dispatch.rs**

```rust
//! 隧道规则真实下发(P3b 数据面)。关联隧道的规则用 split_tunnel_rule 拆成
//! per-hop Rule 分发到链上每个节点;非隧道规则保持原单节点路径。tls/wss 隧道
//! 的 hop 凭据由内置 CA 即时签发(不入 DB,重签幂等),创建/restart/reconcile 下发。
//! Agent 离线时 dispatch 返回 false 仅 warn——reconcile 在下次 subscribe 时兜底。
use emorelay_common::control::v1::{
    command::Body, ApplyRule, Command, RevokeTunnelCredentials, Rule as ProtoRule,
    TunnelCredentials,
};
use tracing::warn;

use crate::grpc::commands::{apply_command, remove_command, restart_command};
use crate::grpc::tunnel_split::{split_tunnel_rule, HopInput, SplitInput};
use crate::models::rule::Rule as DbRule;
use crate::models::tunnel::{Tunnel, TunnelHop};
use crate::state::AppState;

/// 把关联隧道的 DB 规则拆成 (node_id, proto Rule) 列表。
/// 隧道已删/无 hop → Ok(None),调用方按非隧道规则 fallback(防御:正常流程不会出现,
/// 删除保护拦截了「规则还引用、隧道先删」)。
async fn split_for(
    state: &AppState,
    rule: &DbRule,
    tunnel_id: i64,
) -> sqlx::Result<Option<Vec<(i64, ProtoRule)>>> {
    let Some(tunnel) = Tunnel::find_by_id(&state.pool, tunnel_id).await? else {
        return Ok(None);
    };
    let hops = TunnelHop::list_for_tunnel(&state.pool, tunnel_id).await?;
    if hops.is_empty() {
        return Ok(None);
    }
    let mut hop_inputs = Vec::with_capacity(hops.len());
    for h in &hops {
        let addr: Option<(String,)> =
            sqlx::query_as("SELECT public_ip FROM nodes WHERE id = ? AND deleted_at IS NULL")
                .bind(h.node_id)
                .fetch_optional(&state.pool)
                .await?;
        hop_inputs.push(HopInput {
            node_id: h.node_id,
            inter_port: h.inter_port,
            addr: addr.map(|a| a.0).unwrap_or_default(),
        });
    }
    let input = SplitInput {
        rule_id: rule.id,
        protocol: rule.protocol.clone(),
        listen_ip: rule.listen_ip.clone(),
        listen_port: rule.listen_port as u32,
        target_host: rule.target_host.clone(),
        target_port: rule.target_port as u32,
        enabled: rule.enabled != 0,
        bandwidth_mbps: rule.bandwidth_mbps.unwrap_or(0),
        tunnel_id,
        transport: tunnel.transport.clone(),
    };
    Ok(Some(split_tunnel_rule(&input, &hop_inputs)))
}

fn warn_offline(node_id: i64, rule_id: i64, what: &str) {
    warn!(node_id, rule_id, "agent offline; {what} will sync at next register");
}

/// apply(create/update/enable/disable/限速变更统一入口)。
pub async fn dispatch_rule_apply(state: &AppState, rule: &DbRule) -> sqlx::Result<()> {
    match rule.tunnel_id {
        Some(tid) => {
            if let Some(parts) = split_for(state, rule, tid).await? {
                for (node_id, proto) in parts {
                    let cmd = Command {
                        body: Some(Body::ApplyRule(ApplyRule { rule: Some(proto) })),
                    };
                    if !state.dispatcher.dispatch(node_id, cmd) {
                        warn_offline(node_id, rule.id, "tunnel hop rule");
                    }
                }
                return Ok(());
            }
            // fail-closed:隧道不可见(理论不可达,删除保护拦截)时**不下发**——
            // 绝不让本应走加密隧道的规则退化成 entry 节点明文直连。
            warn!(rule_id = rule.id, tunnel_id = tid, "tunnel missing for rule; apply NOT dispatched");
            Ok(())
        }
        None => {
            if !state.dispatcher.dispatch(rule.node_id, apply_command(rule)) {
                warn_offline(rule.node_id, rule.id, "rule");
            }
            Ok(())
        }
    }
}

async fn tunnel_node_ids(state: &AppState, tunnel_id: i64) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar("SELECT node_id FROM tunnel_hops WHERE tunnel_id = ? ORDER BY ordinal")
        .bind(tunnel_id)
        .fetch_all(&state.pool)
        .await
}

/// remove:隧道规则对链上每个节点发 RemoveRule;非隧道单节点。
pub async fn dispatch_rule_remove(state: &AppState, rule: &DbRule) -> sqlx::Result<()> {
    let nodes = match rule.tunnel_id {
        Some(tid) => tunnel_node_ids(state, tid).await?,
        None => vec![rule.node_id],
    };
    for node_id in nodes {
        if !state.dispatcher.dispatch(node_id, remove_command(rule.id)) {
            warn_offline(node_id, rule.id, "rule removal");
        }
    }
    Ok(())
}

/// restart。返回是否至少送达一个节点(rules.rs restart 响应里回显)。
pub async fn dispatch_rule_restart(state: &AppState, rule: &DbRule) -> sqlx::Result<bool> {
    let nodes = match rule.tunnel_id {
        Some(tid) => tunnel_node_ids(state, tid).await?,
        None => vec![rule.node_id],
    };
    let mut any = false;
    for node_id in nodes {
        any |= state.dispatcher.dispatch(node_id, restart_command(rule.id));
    }
    Ok(any)
}

fn credentials_command(state: &AppState, tunnel_id: i64, ordinal: i64) -> Option<Command> {
    match crate::tls::issue::issue_tunnel_hop_certs(&state.ca, tunnel_id, ordinal) {
        Ok(c) => Some(Command {
            body: Some(Body::TunnelCredentials(TunnelCredentials {
                tunnel_id,
                ordinal: ordinal as i32,
                server_cert_pem: c.server_cert_pem,
                server_key_pem: c.server_key_pem,
                client_cert_pem: c.client_cert_pem,
                client_key_pem: c.client_key_pem,
                ca_pem: state.ca.ca_pem.clone(),
            })),
        }),
        Err(e) => {
            warn!(error = ?e, tunnel_id, ordinal, "issue tunnel hop certs failed");
            None
        }
    }
}

/// tls/wss 隧道:为每个 hop 即时签发凭据并下发。tcp 隧道 no-op。
pub async fn dispatch_tunnel_credentials(state: &AppState, tunnel: &Tunnel) -> sqlx::Result<()> {
    if tunnel.transport == "tcp" {
        return Ok(());
    }
    for h in TunnelHop::list_for_tunnel(&state.pool, tunnel.id).await? {
        if let Some(cmd) = credentials_command(state, tunnel.id, h.ordinal) {
            if !state.dispatcher.dispatch(h.node_id, cmd) {
                warn!(node_id = h.node_id, tunnel_id = tunnel.id, "agent offline; credentials will resend at next register");
            }
        }
    }
    Ok(())
}

/// 删隧道后通知各 hop 清理凭据目录。
pub async fn dispatch_revoke_tunnel_credentials(
    state: &AppState,
    tunnel_id: i64,
    hop_node_ids: &[i64],
) {
    for node_id in hop_node_ids {
        let cmd = Command {
            body: Some(Body::RevokeTunnelCredentials(RevokeTunnelCredentials { tunnel_id })),
        };
        let _ = state.dispatcher.dispatch(*node_id, cmd);
    }
}

/// reconcile:Agent 重连后重放该节点应有的全部命令(顺序敏感:凭据先于隧道规则)。
/// 1) 本节点的非隧道规则;2) 本节点参与的每个活跃隧道:凭据(tls/wss) → 该隧道
/// 全部活跃规则 split 后取本节点份额(entry/mid/exit 均覆盖——隧道规则行的 node_id
/// 是 entry,mid/exit 节点上没有 forward_rules 行,只能从 tunnel_hops 反查)。
pub async fn reconcile_commands_for_node(
    state: &AppState,
    node_id: i64,
) -> sqlx::Result<Vec<Command>> {
    let mut out = Vec::new();
    for rule in DbRule::list_active_for_node(&state.pool, node_id).await? {
        if rule.tunnel_id.is_none() {
            out.push(apply_command(&rule));
        }
    }
    for tid in TunnelHop::list_tunnel_ids_for_node(&state.pool, node_id).await? {
        let Some(tunnel) = Tunnel::find_by_id(&state.pool, tid).await? else {
            continue;
        };
        if tunnel.transport != "tcp" {
            if let Some(hop) = TunnelHop::find_for_node(&state.pool, tid, node_id).await? {
                if let Some(cmd) = credentials_command(state, tid, hop.ordinal) {
                    out.push(cmd);
                }
            }
        }
        for rule in DbRule::list_active_for_tunnel(&state.pool, tid).await? {
            if let Some(parts) = split_for(state, &rule, tid).await? {
                for (nid, proto) in parts {
                    if nid == node_id {
                        out.push(Command {
                            body: Some(Body::ApplyRule(ApplyRule { rule: Some(proto) })),
                        });
                    }
                }
            }
        }
    }
    Ok(out)
}
```

`crates/panel-server/src/grpc/mod.rs` 加 `pub mod tunnel_dispatch;`（字母序）。

- [ ] **Step 5: 替换 9 处 dispatch 调用**

逐处「外科手术式」替换（行号以当前文件为准，grep `dispatcher.dispatch` 核对）：

1. `routes/rules.rs` create（`if !state.dispatcher.dispatch(rule.node_id, apply_command(&rule)) { ... }`）→
   ```rust
       crate::grpc::tunnel_dispatch::dispatch_rule_apply(&state, &rule).await?;
   ```
2. `routes/rules.rs` update 同款替换。
3. `routes/rules.rs` `set_enabled_handler`（`if let Ok(Some(rule)) = ... dispatch` 块）→
   ```rust
       if let Ok(Some(rule)) = Rule::find_by_id(&state.pool, id).await {
           let _ = crate::grpc::tunnel_dispatch::dispatch_rule_apply(&state, &rule).await;
       }
   ```
4. `routes/rules.rs` delete（`dispatch(node_id, remove_command(id))`）→ 软删前已取 `existing`：
   ```rust
       crate::grpc::tunnel_dispatch::dispatch_rule_remove(&state, &existing).await?;
   ```
   （`let node_id = existing.node_id;` 一行随之失去引用则删除。）
5. `routes/rules.rs` restart →
   ```rust
       let dispatched = crate::grpc::tunnel_dispatch::dispatch_rule_restart(&state, &rule).await?;
   ```
6. `routes/rules_io.rs` 两处 `dispatch(rule.node_id, apply_command(&rule))` → `let _ = crate::grpc::tunnel_dispatch::dispatch_rule_apply(&state, &rule).await;`（导入规则 `tunnel_id` 恒 `None`，行为等价；统一入口防漂移）。
7. `routes/bandwidth_profiles.rs` 一处同款替换（限速变更重推受影响规则——规则可能关联隧道，必须走 split）。
8. `sweeper/user_quota.rs` 一处同款替换（到期/超额 disable 后重推）。
9. `grpc/service.rs` `subscribe_commands` 的 reconcile 块整体替换为：
   ```rust
           let reconciled = match crate::grpc::tunnel_dispatch::reconcile_commands_for_node(
               &self.state,
               inner.node_id,
           )
           .await
           {
               Ok(cmds) => {
                   let n = cmds.len();
                   for cmd in cmds {
                       self.state.dispatcher.dispatch(inner.node_id, cmd);
                   }
                   n
               }
               Err(e) => {
                   warn!(error = ?e, "reconcile query failed; agent will run with last-known rules");
                   0
               }
           };
   ```
   随之清理 service.rs 失去引用的 `apply_command` / `DbRule` import；rules.rs 失去引用的 `apply_command/remove_command/restart_command` import 同理（只删因本改动孤儿化的）。

- [ ] **Step 6: tunnels.rs 接入（create 校验 + 凭据下发 / delete 吊销 / restart 真实下发）**

`routes/tunnels.rs`：

create 的 `NodeRow` 查询补 `public_ip`（`SELECT id, status, public_ip, port_pool_min, port_pool_max ...`，struct 加 `public_ip: String`），并在 online 校验后加：

```rust
        // ordinal ≥ 1 的 hop 会被上一跳 dial,split 时 next_hop_addr 取它的 public_ip。
        if ordinal >= 1 && row.public_ip.trim().is_empty() {
            return Err(ApiError::BadRequest(format!(
                "node {nid} needs a public_ip to be a dialed hop (ordinal >= 1)"
            )));
        }
```

（实现提示：现状 create 先收集 pools 再分配——把 public_ip 校验并入第一段节点遍历，遍历时已有 ordinal 序号 = `node_ids` 下标。）

create 在 `Tunnel::create_with_hops` 成功、audit 之前/之后加凭据下发，并替换原「控制面不 dispatch」注释：

```rust
    // 数据面:tls/wss 隧道即时签发 hop 凭据下发(Agent 离线 → reconcile 兜底)。
    if let Some(t) = Tunnel::find_by_id(&state.pool, tid).await? {
        crate::grpc::tunnel_dispatch::dispatch_tunnel_credentials(&state, &t).await?;
    }
```

delete 在 `soft_delete` 成功后、audit 之前加（hop 节点列表须在软删前取出）：

```rust
    let hop_nodes: Vec<i64> =
        sqlx::query_scalar("SELECT node_id FROM tunnel_hops WHERE tunnel_id = ? ORDER BY ordinal")
            .bind(id)
            .fetch_all(&state.pool)
            .await?;
```

（放在 `Tunnel::soft_delete` 调用之前），软删成功后：

```rust
    crate::grpc::tunnel_dispatch::dispatch_revoke_tunnel_credentials(&state, id, &hop_nodes).await;
```

restart 整个 handler 体替换为真实下发：

```rust
pub async fn restart(
    State(state): State<AppState>, auth: AuthUser, actor_ip: ActorIp, Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    auth.require_admin()?;
    let t = Tunnel::find_by_id(&state.pool, id).await?.ok_or(ApiError::NotFound)?;
    // 凭据先行(重签轮换),再对该隧道全部活跃规则 per-hop restart。
    crate::grpc::tunnel_dispatch::dispatch_tunnel_credentials(&state, &t).await?;
    let mut dispatched = false;
    for rule in crate::models::rule::Rule::list_active_for_tunnel(&state.pool, id).await? {
        dispatched |= crate::grpc::tunnel_dispatch::dispatch_rule_restart(&state, &rule).await?;
    }
    audit::record_with_ip(&state.pool, Some(auth.0.sub), actor_ip.as_option(),
        "tunnel.restart", Some("tunnel"), Some(id), None, true, None).await;
    Ok(Json(json!({ "ok": true, "dispatched": dispatched })))
}
```

- [ ] **Step 7: 跑测试验证通过**

Run: `cargo test -p panel-server --test api_tunnel_dispatch && cargo test -p panel-server --test api_tunnels && cargo test -p panel-server --test api_rules && cargo test --workspace`
Expected: 全 PASS（注意 api_tunnels.rs 既有 `delete_tunnel_blocked_by_rule_reference` 等不受影响；`restart` 既有断言若有 `dispatched: false` 需同步——grep 确认,api_tunnels.rs 现无 restart 断言则不动）。

- [ ] **Step 8: Commit**

```bash
git add crates/panel-server/src/grpc crates/panel-server/src/models crates/panel-server/src/routes crates/panel-server/src/sweeper crates/panel-server/tests/api_tunnel_dispatch.rs
git commit -m "feat(server): real tunnel dispatch — per-hop split rules, credentials issuance, reconcile replay"
```

---

## Task 9: 隧道 status 心跳聚合 + install.sh/.env + 文档收尾

**Files:**
- Modify: `crates/panel-server/src/models/tunnel.rs`、`crates/panel-server/src/routes/tunnels.rs`、`crates/panel-server/tests/api_tunnels.rs`
- Modify: `crates/panel-server/src/routes/install.rs`、`crates/panel-server/tests/api_install.rs`
- Modify: `.env.example`、`docs/api.md`、`README.md`、`plan.md`、`CLAUDE.md`、`docs/superpowers/plans/2026-06-10-mvp-followups-phase-3.md`

- [ ] **Step 1: 写失败测试（追加 api_tunnels.rs）**

```rust
#[tokio::test]
async fn tunnel_status_aggregates_hop_heartbeats() {
    let app = common::make_app().await.unwrap();
    let nodes = seed_online_nodes(&app, 2).await;
    let req = common::auth_req(Method::POST, "/api/tunnels", &app.admin_token,
        Some(json!({ "name": "hb", "transport": "tcp", "node_ids": nodes }))).unwrap();
    let (_, body) = common::send(app.app.clone(), req).await.unwrap();
    let tid = body["id"].as_i64().unwrap();

    let set_seen = |node_id: i64, expr: &'static str| {
        let pool = app.state.pool.clone();
        async move {
            sqlx::query(&format!("UPDATE nodes SET last_seen_at = {expr} WHERE id = ?"))
                .bind(node_id).execute(&pool).await.unwrap();
        }
    };
    let get_status = || async {
        let req = common::auth_req(Method::GET, &format!("/api/tunnels/{tid}/status"),
            &app.admin_token, None).unwrap();
        let (s, b) = common::send(app.app.clone(), req).await.unwrap();
        assert_eq!(s, StatusCode::OK);
        b["status"].as_str().unwrap().to_string()
    };

    // 全部 hop 30s 内有心跳 → up。
    set_seen(nodes[0], "datetime('now')").await;
    set_seen(nodes[1], "datetime('now')").await;
    assert_eq!(get_status().await, "up");
    // 一个超窗 → degraded。
    set_seen(nodes[1], "datetime('now', '-120 seconds')").await;
    assert_eq!(get_status().await, "degraded");
    // 全部超窗 → down。
    set_seen(nodes[0], "datetime('now', '-120 seconds')").await;
    assert_eq!(get_status().await, "down");

    // 聚合值回写 tunnels.status,GET :id 同步反映。
    let req = common::auth_req(Method::GET, &format!("/api/tunnels/{tid}"), &app.admin_token, None).unwrap();
    let (_, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(body["status"], "down");
}
```

`tests/api_install.rs` 追加断言（在既有 `install_script_parses_tls_pem_args` 测试里或新测试）：

```rust
#[tokio::test]
async fn install_script_sets_agent_data_dir() {
    let app = common::make_app().await.unwrap();
    // install.sh 路由挂 IP 维度 rate limiter:必须带 x-forwarded-for(沿用本文件既有测试写法)。
    let req = Request::get("/install.sh?node=7")
        .header("x-forwarded-for", "203.0.113.77")
        .body(Body::empty())
        .unwrap();
    let (status, body) = common::send_text(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("AGENT_DATA_DIR=/var/lib/emorelay"));
}
```

（`send_text`、header 写法均以 `api_install.rs` 既有测试为准对齐，缺则沿用既有 body 读取方式;每个测试用独立 IP 避免触发限流。）

- [ ] **Step 2: 跑测试验证失败**

Run: `cargo test -p panel-server --test api_tunnels && cargo test -p panel-server --test api_install`
Expected: status 测试 FAIL（status 恒 `unknown`）+ install 测试 FAIL。

- [ ] **Step 3: models/tunnel.rs 加聚合**

`impl Tunnel` 追加：

```rust
    /// hop 心跳聚合(spec §5.1:最近 30s 有心跳 = 该 hop 存活)。
    /// 全部存活 → up;全部超窗 → down;部分 → degraded;无 hop → unknown(防御)。
    pub async fn compute_status(pool: &SqlitePool, id: i64) -> sqlx::Result<String> {
        let (total, alive): (i64, i64) = sqlx::query_as(
            "SELECT COUNT(*), \
                COALESCE(SUM(CASE WHEN n.last_seen_at IS NOT NULL \
                    AND n.last_seen_at >= datetime('now', '-30 seconds') THEN 1 ELSE 0 END), 0) \
             FROM tunnel_hops th JOIN nodes n ON n.id = th.node_id \
             WHERE th.tunnel_id = ?",
        )
        .bind(id)
        .fetch_one(pool)
        .await?;
        Ok(match (total, alive) {
            (0, _) => "unknown",
            (t, a) if a == t => "up",
            (_, 0) => "down",
            _ => "degraded",
        }
        .to_string())
    }

    pub async fn set_status(pool: &SqlitePool, id: i64, status: &str) -> sqlx::Result<u64> {
        let res = sqlx::query(
            "UPDATE tunnels SET status = ?, updated_at = datetime('now') \
             WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(status)
        .bind(id)
        .execute(pool)
        .await?;
        Ok(res.rows_affected())
    }
```

- [ ] **Step 4: tunnels.rs status/get 接入**

`status` handler 改为实时计算 + 回写：

```rust
pub async fn status(
    State(state): State<AppState>, auth: AuthUser, Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    auth.require_admin()?;
    let _t = Tunnel::find_by_id(&state.pool, id).await?.ok_or(ApiError::NotFound)?;
    let status = Tunnel::compute_status(&state.pool, id).await?;
    let _ = Tunnel::set_status(&state.pool, id, &status).await;
    Ok(Json(json!({ "id": id, "status": status })))
}
```

`get` handler 在取 hops 之前同样刷新（list 保持存储值，避免分页 N 次聚合）：

```rust
    let status = Tunnel::compute_status(&state.pool, id).await?;
    let _ = Tunnel::set_status(&state.pool, id, &status).await;
```

（`TunnelDetail` 的 `status` 字段改用计算值 `status`，不再用 `t.status`。）

- [ ] **Step 5: install.rs + .env.example**

`routes/install.rs` 的 agent.env heredoc 加一行（`AGENT_STATE_PATH` 之后）：

```bash
AGENT_DATA_DIR=/var/lib/emorelay
```

`.env.example` Agent 段（`AGENT_STATE_PATH` 之后）加：

```bash
# Agent 本地数据目录:隧道 TLS 凭据落 ${AGENT_DATA_DIR}/tunnels/<id>/hop-<ordinal>/。
AGENT_DATA_DIR=./agent-data
```

- [ ] **Step 6: 文档**

- `docs/api.md`：
  - Tunnels 一节更新：`status` 语义（hop 心跳聚合 30s 窗口,up/degraded/down;GET :id 与 /status 实时刷新并**回写存储值**——GET 有写副作用,list 返回上次刷新值）；`restart` 改为「重签下发 hop 凭据 + 对隧道全部活跃规则 per-hop 重启,返回 dispatched」；create 校验补「ordinal ≥ 1 的节点必须有 public_ip」；transport 选型注明「wss 不保证 TCP 半关语义,依赖半关的业务流选 tcp/tls」。
  - rules 一节补「关联隧道的规则会拆成 entry/mid/exit 实例分发到链上各节点;流量统计与限速只在 entry 计」。
  - 新增「隧道凭据下发」小节：`Command.tunnel_credentials`（含 ca_pem 自包含）、即时签发不入 DB、reconcile 重发、删除隧道 → revoke 清理 Agent 目录。
- `README.md` 功能列表加「多跳隧道数据面（TCP/TLS/WSS,UDP-over-tunnel）」。
- `plan.md` 附录「Phase 3b 控制面」之后加「Phase 3b 数据面（2026-06-11 启动）」：本计划文件路径、Agent tunnel 模块六文件、proto self_ordinal/ca_pem 字段、stats/限速 entry-only 决策、preamble 协议、reconcile 扩展、status 聚合;注明 P3c（前端 + e2e）待展开。
- `CLAUDE.md`「仓库现状」：当前阶段改为「P3c（隧道前端 + e2e）」,已交付列表补 P3b 数据面一行,待推进改 P3c。
- `docs/superpowers/plans/2026-06-10-mvp-followups-phase-3.md` 的「P3b-数据面 概要」节首加一行指针：「**已展开为独立 TDD 计划:`2026-06-11-p3b-data-plane.md`(9 Task);以下概要保留作历史背景。**」

- [ ] **Step 7: 全量回归 + Commit**

Run: `cargo test --workspace && cd web && npx vitest run && npm run build`
Expected: 全 PASS（前端未改,跑一次确认无意外耦合）。

```bash
git add crates/panel-server/src/models/tunnel.rs crates/panel-server/src/routes/tunnels.rs crates/panel-server/src/routes/install.rs crates/panel-server/tests/api_tunnels.rs crates/panel-server/tests/api_install.rs .env.example docs/api.md README.md plan.md CLAUDE.md docs/superpowers/plans/2026-06-10-mvp-followups-phase-3.md
git commit -m "feat(server): tunnel status from hop heartbeats; AGENT_DATA_DIR in install env; p3b data-plane docs"
```

---

## P3b 数据面 验收清单

- [ ] proto `TunnelContext.self_ordinal` / `TunnelCredentials.ca_pem` 编译可构造;split 填 ordinal → Task 1
- [ ] Agent 持久化 tunnel 上下文,旧 state 文件兼容加载 → Task 1
- [ ] `TunnelTransport` trait + TCP transport dial/bind/accept 回环 → Task 2
- [ ] 双跳/三跳 TCP 业务流经 entry→(mid)→exit 往返;stats 只在 entry → Task 3
- [ ] UDP-over-tunnel:2 字节帧 + per-client session 复用 + 闲置回收;tcp_udp 双栈 preamble 区分 → Task 4
- [ ] RuleManager 对带 tunnel 的 Rule 起 TunnelTask;remove 释放端口;重启恢复隧道角色 → Task 5
- [ ] `issue_tunnel_hop_certs` 签 server/client 对;Agent 落盘/Revoke 清理;TLS transport 双向认证通、拒非 TLS 连入 → Task 6
- [ ] WSS transport 字节往返(含大 payload 分块) → Task 7
- [ ] 创建关联隧道规则 → 链上各节点收到正确角色拆分 Rule;tls 隧道创建 → 各 hop 收凭据(含 ca_pem);删规则/删隧道 → RemoveRule/Revoke;reconcile 凭据先行重放本 hop 份额;restart 重签重启;无 public_ip 的被 dial hop 拒建 → Task 8
- [ ] 隧道 status = hop 心跳 30s 聚合(up/degraded/down)并回写;install.sh 写 AGENT_DATA_DIR → Task 9
- [ ] 每 Task `cargo test --workspace` 全绿 + 子代理 `superpowers:code-reviewer` 通过

> 真机双跳/三跳 e2e（起真 panel-server + 多 agent 进程,curl 入口达目标 + mTLS 吊销断链 + UDP 帧实测）属 **P3c**（spec §4.8/§4.9 单元 8）,与隧道前端一并展开。

## 执行注意（给实施会话）

1. 严格按 Task 1→9 顺序:T3 依赖 T2(transport)与 T1(self_ordinal);T5 依赖 T3/T4(task);T6 依赖 T5(make_transport 签名)与 T1(ca_pem/creds 目录);T7 依赖 T6(TlsTransport 复用);T8 依赖 T1(split self_ordinal)+T6(issue);T9 依赖 T8(tunnels.rs 已改)。
2. **rustls/tungstenite API 不确定性**:T6/T7 动手前 context7 校准(`tokio-rustls 0.26` feature/provider、`Message::Binary` 载荷类型)。测试断言是契约,实现体按真实 API 调整,不改断言语义。provider 统一 **ring**,main.rs 启动 `install_default` 一次。
3. T8 的 9 处 dispatch 替换是本计划唯一的「面状」改动:每处只换 dispatch 调用行,不动周边 audit/校验逻辑;随手清理**因替换而孤儿化**的 import,不动其他。错误传播取舍:create/update/delete/restart 用 `?`(split 的 DB 查询失败是真异常,500 合理),set_enabled/rules_io/sweeper 用 `let _ =`(原行为 dispatch 不影响主流程)——两者都不让「Agent 离线」影响响应,离线由 reconcile 兜底。
4. 不顺手改无关代码。发现本计划与代码现状冲突(行号漂移、helper 命名差异)时按最小改动对齐,以「让本计划的测试通过」为准。
5. 每个 Task 收尾:跑该 Task Run 命令 + `cargo test --workspace`,全绿 → commit → spawn `general-purpose` 子代理走 `superpowers:code-reviewer`(prompt 含本文件与 CLAUDE.md/plan.md 路径、该 Task 文件清单、强制红线),阻塞性问题修完才进下一 Task。
6. P3b 数据面完成后**停下来向用户报告**,确认验收(尤其 TLS 隧道在真实双节点环境的手工冒烟)后再展开 P3c。
