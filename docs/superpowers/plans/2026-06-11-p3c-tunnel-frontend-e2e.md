# P3c · 隧道前端 + 双跳/三跳 e2e Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 交付隧道管理前端（/tunnels 列表 + 详情 + Rules 关联隧道）与真 server+真 agent 的双跳/三跳 e2e（TCP/TLS 矩阵 + UDP-over-tunnel + mTLS 真链路/吊销），并收紧 P3b 终审记录的两条安全限制（隧道 client SAN 校验、Agent apply 失败重试）。

**Architecture:** 前端按 Nodes/NodeDetail 既有模式新增两页 + RuleForm 下拉；后端只在 `routes/tunnels.rs` 扩展 `rules_count`/`rules` 字段（不动 /api/rules 筛选）；node-agent 抽出 `lib.rs` + `run_agent(Config)` 可编程入口，panel-server 以 dev-dependency 引入，在 `tests/tunnel_e2e.rs` 内 in-process 起真 gRPC server + 多个真 agent 跑真流量。

**Tech Stack:** React 19 + vitest/@testing-library、Axum/SQLx、tonic 0.12、rustls 0.23（WebPkiClientVerifier）、x509-parser（SAN 解析）、rcgen 0.13。

---

## 全局约定

- **每个 Task 收尾**：跑该 Task 的 Run 命令 + `cargo test --workspace`（前端 Task 用 `cd web && npx vitest run && npm run build`），全绿 → commit → spawn `general-purpose` 子代理走 `superpowers:code-reviewer`（只读，三段式回报，prompt 附本文件与 CLAUDE.md 路径 + 待审文件绝对路径），阻塞性问题修完才进下一 Task。
- **不顺手改无关代码**；计划与代码现状冲突时按最小改动同步。
- **crate API 不确定性**：Task 1 的 x509-parser、Task 11 的 tonic ClientTlsConfig，动手前先 context7 校准当前 API；本计划骨架是行为契约，测试断言是验收标准，实现体按真实 API 调整，**不改测试断言语义**。
- **e2e 端口约定**：每个 e2e 测试用独立端口段防并行互撞——双跳 TCP 21xxx、三跳 TCP 22xxx、双跳 TLS 23xxx、三跳 TLS 24xxx、UDP 25xxx。listen_ip 一律 `127.0.0.1`。

## 范围外（明确不做，避免执行时擅自扩张）

- **隧道侧 CRL**：隧道 hop 凭据即时签发、不入库、无 fingerprint 记录，Agent 侧也没有 CRL 分发通道。本阶段以「client SAN = 上一跳」校验阻断同 CA 凭据横向连入；被吊销节点若保留凭据文件副本、在隧道仍存续期间可连入的残余风险，留待后续「隧道凭据短有效期 + 定期轮换」方案（不在 P3c）。
- **ApplyRule proto ack**：重试用 Agent 侧队列实现（Task 3），不改 proto。
- **节点链拖拽排序**：用「上移/下移」按钮满足排序能力，不引入拖拽库。
- **Rules 列表页按 tunnel 筛选**：详情页关联规则直接由 `GET /api/tunnels/:id` 返回，不给 `/api/rules` 加 `tunnel_id` 查询参数。
- **WSS e2e**：transport 矩阵 e2e 覆盖 TCP/TLS（spec §4.8 验收范围）；WSS 数据面已有单测（roundtrip/partial-read/large-payload），不重复入 e2e。

---

## Task 1: 隧道 TLS/WSS server 端校验 client SAN = 上一跳

P3b 终审已知限制①：hop server 端只验 client cert 链到 CA，不校验 SAN——同一内置 CA 的任意凭据（任何节点、任何隧道、任何 hop）都能连入任意 hop。本 Task 在 TLS/WSS accept 握手后校验 client cert SAN 必须等于 `tunnel-<id>-hop-<self_ordinal-1>.emorelay.internal`。

**Files:**
- Modify: `crates/node-agent/Cargo.toml`（加 `x509-parser`）
- Modify: `crates/node-agent/src/tunnel/tls_transport.rs`
- Modify: `crates/node-agent/src/tunnel/wss_transport.rs`
- Modify: `crates/node-agent/src/tunnel/testutil.rs`
- Test: `tls_transport.rs` 内嵌 tests

- [ ] **Step 1: testutil 扩展——允许 client SAN 与目录 ordinal 不一致**

把 `write_hop_creds_pair` 重构为基于新的 `write_hop_creds_matrix`（同一 CA；server SAN 始终 = 目录 ordinal，client SAN 可指向别的 hop，用于构造负路径）：

```rust
/// 为同一 CA 下的多个 hop 目录写凭据。spec = (dir_ordinal, client_san_ordinal):
/// server SAN 始终 = tunnel-<id>-hop-<dir_ordinal>;client SAN = tunnel-<id>-hop-<client_san_ordinal>
/// (两者不一致用于构造「链合法但 SAN 不属于上一跳」的负路径)。
pub async fn write_hop_creds_matrix(data_dir: &str, tunnel_id: i64, specs: &[(u32, u32)]) {
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    let (ca_key, ca_cert) = make_ca();
    let ca_pem = ca_cert.pem();
    for (dir_ordinal, client_san_ordinal) in specs {
        let server_san = format!("tunnel-{tunnel_id}-hop-{dir_ordinal}.emorelay.internal");
        let client_san = format!("tunnel-{tunnel_id}-hop-{client_san_ordinal}.emorelay.internal");
        let (server_cert_pem, server_key_pem) =
            issue_leaf(&server_san, ExtendedKeyUsagePurpose::ServerAuth, &ca_cert, &ca_key);
        let (client_cert_pem, client_key_pem) =
            issue_leaf(&client_san, ExtendedKeyUsagePurpose::ClientAuth, &ca_cert, &ca_key);
        crate::tunnel::creds::store(
            data_dir,
            &TunnelCredentials {
                tunnel_id,
                ordinal: *dir_ordinal as i32,
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

/// 为相邻两 hop 各写一套凭据(同一 CA,client SAN = 自身 hop,正路径)。
pub async fn write_hop_creds_pair(data_dir: &str, tunnel_id: i64, ordinal_a: u32, ordinal_b: u32) {
    write_hop_creds_matrix(data_dir, tunnel_id, &[(ordinal_a, ordinal_a), (ordinal_b, ordinal_b)]).await
}
```

（原 `write_hop_creds_pair` 函数体删除，循环逻辑并入 matrix；`make_ca`/`issue_leaf` 保持私有不变。）

- [ ] **Step 2: 写失败测试（追加 tls_transport.rs tests）**

```rust
    /// 同 CA、链合法、但 client SAN 指向 hop-7 而非上一跳 hop-0:
    /// hop-1 server 必须在握手后拒绝(SAN 校验)。防同 CA 凭据横向连入。
    #[tokio::test]
    async fn tls_server_rejects_client_cert_with_wrong_hop_san() {
        let dir = tempfile::TempDir::new().unwrap();
        let data_dir = dir.path().display().to_string();
        // hop-0 目录:client SAN 伪造为 hop-7;hop-1 目录正常。
        crate::tunnel::testutil::write_hop_creds_matrix(&data_dir, 9, &[(0, 7), (1, 1)]).await;

        let server_t = TlsTransport::load(&data_dir, &ctx(9, 1)).unwrap();
        let client_t = TlsTransport::load(&data_dir, &ctx(9, 0)).unwrap();
        let mut listener = server_t.bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let client = tokio::spawn(async move {
            // 链验证在握手层通过,dial 可能成功;server 在 accept 内做 SAN 校验后拒绝。
            let _ = client_t.dial(&addr.to_string()).await;
        });
        assert!(
            listener.accept().await.is_err(),
            "client SAN 不是上一跳(hop-0)必须被拒"
        );
        let _ = client.await;
    }

    /// entry(self_ordinal=0)没有上一跳,不允许 bind 隧道 listener(防御)。
    #[tokio::test]
    async fn tls_entry_hop_must_not_bind() {
        let dir = tempfile::TempDir::new().unwrap();
        let data_dir = dir.path().display().to_string();
        write_hop_creds_pair(&data_dir, 9, 0, 1).await;
        let t = TlsTransport::load(&data_dir, &ctx(9, 0)).unwrap();
        assert!(t.bind("127.0.0.1:0").await.is_err());
    }
```

- [ ] **Step 3: 跑测试验证失败**

Run: `cargo test -p node-agent tls_`
Expected: 新增两个测试 FAIL（`write_hop_creds_matrix` 不存在则先补 Step 1；SAN 校验未实现 → `rejects_client_cert_with_wrong_hop_san` 中 accept 返回 Ok）。

- [ ] **Step 4: 实现 SAN 校验**

`crates/node-agent/Cargo.toml` `[dependencies]` 加（版本以 context7 校准，0.16/0.17 均含所需 API）：

```toml
# P3c:隧道 server 端校验 client cert SAN = 上一跳(防同 CA 凭据横向连入)。
x509-parser = "0.16"
```

`tls_transport.rs`：

```rust
pub struct TlsTransport {
    pub(crate) connector: TlsConnector,
    pub(crate) acceptor: TlsAcceptor,
    pub(crate) dial_sni: ServerName<'static>,
    /// server 端期望的 client cert SAN(= 上一跳)。entry(ordinal 0)无上一跳 → None,不允许 bind。
    pub(crate) expect_client_san: Option<String>,
}
```

`load()` 末尾构造处加：

```rust
        let expect_client_san = ctx.self_ordinal.checked_sub(1).map(|prev| {
            format!("tunnel-{}-hop-{}.emorelay.internal", ctx.tunnel_id, prev)
        });
        Ok(Self {
            connector: TlsConnector::from(Arc::new(client_cfg)),
            acceptor: TlsAcceptor::from(Arc::new(server_cfg)),
            dial_sni: ServerName::try_from(sni).context("invalid tunnel sni")?,
            expect_client_san,
        })
```

`bind()` 改为：

```rust
    async fn bind(&self, addr: &str) -> Result<Box<dyn TunnelListener>> {
        let expect = self
            .expect_client_san
            .clone()
            .context("entry hop (ordinal 0) must not bind a tunnel listener")?;
        let l = TcpListener::bind(addr)
            .await
            .with_context(|| format!("tunnel tls bind {addr}"))?;
        Ok(Box::new(TlsTunnelListener {
            inner: l,
            acceptor: self.acceptor.clone(),
            expect_client_san: expect,
        }))
    }
```

模块级 helper（TLS/WSS 共用）：

```rust
/// 握手后校验 client cert 的 SAN 含 expected(= 上一跳)。
/// WebPkiClientVerifier 只验链;这里补「持证者必须是上一跳」的身份绑定。
pub(crate) fn verify_client_san(
    conn: &tokio_rustls::rustls::ServerConnection,
    expected: &str,
) -> Result<()> {
    let certs = conn
        .peer_certificates()
        .context("tunnel peer presented no client certificate")?;
    let leaf = certs.first().context("empty client certificate chain")?;
    let (_, cert) = x509_parser::parse_x509_certificate(leaf.as_ref())
        .map_err(|e| anyhow::anyhow!("parse tunnel client cert: {e}"))?;
    let ok = cert
        .subject_alternative_name()
        .ok()
        .flatten()
        .map(|ext| {
            ext.value.general_names.iter().any(|n| {
                matches!(n, x509_parser::extensions::GeneralName::DNSName(d) if *d == expected)
            })
        })
        .unwrap_or(false);
    if ok {
        Ok(())
    } else {
        anyhow::bail!("tunnel client cert SAN does not match previous hop {expected}")
    }
}
```

`TlsTunnelListener` 加字段 `expect_client_san: String`，`accept()` 在握手成功后校验：

```rust
    async fn accept(&mut self) -> Result<TunnelConn> {
        let (tcp, _) = self.inner.accept().await.context("tunnel tls tcp accept")?;
        let tls = self
            .acceptor
            .accept(tcp)
            .await
            .context("tunnel tls server handshake")?;
        verify_client_san(tls.get_ref().1, &self.expect_client_san)?;
        Ok(Box::new(tls))
    }
```

`wss_transport.rs` 同步接线：`WssTunnelListener` 加字段 `expect_client_san: String`；`WssTransport::bind` 改为：

```rust
    async fn bind(&self, addr: &str) -> Result<Box<dyn TunnelListener>> {
        let expect = self
            .tls
            .expect_client_san
            .clone()
            .context("entry hop (ordinal 0) must not bind a tunnel listener")?;
        let l = TcpListener::bind(addr)
            .await
            .with_context(|| format!("tunnel wss bind {addr}"))?;
        Ok(Box::new(WssTunnelListener {
            inner: l,
            acceptor: self.tls.acceptor.clone(),
            expect_client_san: expect,
        }))
    }
```

`WssTunnelListener::accept()` 在 TLS 握手后、ws 握手前加：

```rust
        crate::tunnel::tls_transport::verify_client_san(tls.get_ref().1, &self.expect_client_san)?;
```

- [ ] **Step 5: 跑测试验证通过**

Run: `cargo test -p node-agent && cargo test --workspace`
Expected: 全 PASS——既有正路径（roundtrip：hop-0 client SAN=hop-0 连 hop-1，expect=hop-0 ✓）与 WSS 三个测试不受影响；新增两个测试 PASS。

- [ ] **Step 6: Commit**

```bash
git add crates/node-agent/Cargo.toml crates/node-agent/src/tunnel/tls_transport.rs crates/node-agent/src/tunnel/wss_transport.rs crates/node-agent/src/tunnel/testutil.rs Cargo.lock
git commit -m "feat(agent): tunnel TLS/WSS server verifies client cert SAN equals previous hop"
```

---

## Task 2: node-agent lib 化（run_agent 可编程入口）

e2e 需要在 panel-server 测试进程内起真 agent。把 main.rs 的会话循环抽到 lib，main 变薄壳。**纯重构，无行为变化**，验收 = 全量回归。

**Files:**
- Create: `crates/node-agent/src/lib.rs`、`crates/node-agent/src/agent.rs`
- Modify: `crates/node-agent/src/main.rs`

- [ ] **Step 1: 创建 lib.rs**

```rust
//! node-agent 库入口(P3c)。把会话循环暴露为可编程 API,
//! 供 panel-server e2e 测试 in-process 起真 agent;二进制 main 也走这里。
pub mod agent;
pub mod config;
pub mod control;
pub mod limit;
pub mod manager;
pub mod relay;
pub mod stats;
pub mod store;
pub mod system;
pub mod tunnel;

pub use agent::run_agent;
```

- [ ] **Step 2: 创建 agent.rs（从 main.rs 平移）**

把 main.rs 中的 `HEARTBEAT_INTERVAL`、`RETRY_BACKOFF`、`stats_interval()`、`run_session()`、`report_node_stats()`、`report_stats()`、`handle_command()` **原样平移**到 `src/agent.rs`（不改逻辑），并加入口：

```rust
/// Agent 主循环:加载本地持久化规则 → 连接主控 → 处理命令/心跳/统计,
/// 会话断开后退避重连,永不返回(调用方 spawn + abort 控制生命周期)。
pub async fn run_agent(config: Config) -> Result<()> {
    // 隧道 TLS/WSS 用 ring provider(与 tonic tls 栈对齐)。重复安装无害,忽略结果。
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();

    let stats = Arc::new(StatsCollector::new());
    let manager = Arc::new(Mutex::new(RuleManager::new(
        stats.clone(),
        config.data_dir.clone(),
    )));
    let store = Arc::new(ConfigStore::new(config.state_path.clone()));
    let sampler = Arc::new(SystemSampler::new());

    let persisted = match store.load().await {
        Ok(rs) => rs,
        Err(e) => {
            warn!(error = ?e, "load persisted state failed; starting fresh");
            Vec::new()
        }
    };
    {
        let mut mgr = manager.lock().await;
        for rule in persisted {
            let rule_id = rule.id;
            if let Err(e) = mgr.apply(rule).await {
                warn!(rule_id, error = ?e, "apply persisted rule failed");
            }
        }
    }

    loop {
        match run_session(&config, manager.clone(), store.clone(), stats.clone(), sampler.clone())
            .await
        {
            Ok(()) => warn!("session ended cleanly; reconnecting after backoff"),
            Err(e) => error!(error = ?e, "session error; reconnecting after backoff"),
        }
        tokio::time::sleep(RETRY_BACKOFF).await;
    }
}
```

use 声明按编译器提示补齐（`crate::config::Config` 等路径不变，模块都在 lib 内）。

- [ ] **Step 3: main.rs 改薄壳**

```rust
use anyhow::Result;
use tracing::info;
use tracing_subscriber::EnvFilter;

use node_agent::config::Config;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let config = Config::from_env()?;
    info!(
        node_id = config.node_id,
        endpoint = %config.control_endpoint,
        state_path = %config.state_path,
        "node-agent starting"
    );
    node_agent::run_agent(config).await
}
```

（原 `mod xxx;` 声明全部删除——已移入 lib.rs。Cargo 对同名 lib+bin crate 默认 `node_agent` 下划线命名。）

- [ ] **Step 4: 回归验证**

Run: `cargo build -p node-agent && cargo test --workspace`
Expected: 编译通过 + 全绿（单测都在 lib 内,自动随 lib 跑）。

- [ ] **Step 5: Commit**

```bash
git add crates/node-agent/src/lib.rs crates/node-agent/src/agent.rs crates/node-agent/src/main.rs
git commit -m "refactor(agent): extract library entry run_agent for in-process e2e"
```

---

## Task 3: Agent 命令失败重试队列

P3b 终审已知限制②：apply 失败只记日志，靠下次 reconcile 兜底。本 Task 加会话内重试队列：失败命令 30s 后重试，最多 5 次；收到同 rule_id 新命令时旧重试作废（防过期 Apply 复活已删规则）；会话断开队列随之丢弃（重连后 server reconcile 全量重放，语义不重叠）。

**Files:**
- Create: `crates/node-agent/src/retry.rs`
- Modify: `crates/node-agent/src/lib.rs`（`pub mod retry;`）、`crates/node-agent/src/agent.rs`（接线）

- [ ] **Step 1: 写失败测试（retry.rs 内嵌 tests，先写文件骨架 + 测试）**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use emorelay_common::control::v1::{command::Body, ApplyRule, Command, RemoveRule, Rule};
    use std::time::{Duration, Instant};

    fn apply_cmd(rule_id: i64) -> Command {
        Command {
            body: Some(Body::ApplyRule(ApplyRule {
                rule: Some(Rule { id: rule_id, ..Default::default() }),
            })),
        }
    }

    #[test]
    fn due_only_after_delay_and_requeue_respects_max_attempts() {
        let mut q = RetryQueue::default();
        let t0 = Instant::now();
        assert!(q.push_failed(apply_cmd(1), 0, t0));
        assert!(q.take_due(t0).is_empty(), "未到期不重试");

        let t1 = t0 + RETRY_DELAY + Duration::from_secs(1);
        let due = q.take_due(t1);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].attempts, 1);
        assert!(q.take_due(t1).is_empty(), "取走后队列为空");

        // 连续失败到 MAX_ATTEMPTS 后丢弃。
        let mut attempts = due[0].attempts;
        let mut now = t1;
        loop {
            let accepted = q.push_failed(apply_cmd(1), attempts, now);
            if attempts + 1 >= MAX_ATTEMPTS {
                assert!(!accepted, "超过最大重试次数必须丢弃");
                break;
            }
            assert!(accepted);
            now = now + RETRY_DELAY + Duration::from_secs(1);
            let d = q.take_due(now);
            assert_eq!(d.len(), 1);
            attempts = d[0].attempts;
        }
    }

    #[test]
    fn new_command_supersedes_pending_retry_of_same_rule() {
        let mut q = RetryQueue::default();
        let t0 = Instant::now();
        assert!(q.push_failed(apply_cmd(7), 0, t0));
        // 收到同 rule 的 RemoveRule → 挂起的 Apply 重试作废。
        let remove = Command { body: Some(Body::RemoveRule(RemoveRule { rule_id: 7 })) };
        q.supersede(&remove);
        assert!(q.take_due(t0 + RETRY_DELAY * 2).is_empty());
    }

    #[test]
    fn same_rule_repush_replaces_old_entry() {
        let mut q = RetryQueue::default();
        let t0 = Instant::now();
        assert!(q.push_failed(apply_cmd(3), 0, t0));
        assert!(q.push_failed(apply_cmd(3), 1, t0));
        let due = q.take_due(t0 + RETRY_DELAY + Duration::from_secs(1));
        assert_eq!(due.len(), 1, "同 rule 只保留最新一条");
        assert_eq!(due[0].attempts, 2);
    }

    #[test]
    fn commands_without_rule_id_are_not_queued() {
        let mut q = RetryQueue::default();
        let cmd = Command { body: None };
        assert!(!q.push_failed(cmd, 0, Instant::now()));
    }
}
```

- [ ] **Step 2: 跑测试验证失败**

Run: `cargo test -p node-agent retry`
Expected: 编译 FAIL（`RetryQueue` 未实现）。

- [ ] **Step 3: 实现 retry.rs**

```rust
//! 命令失败重试队列(P3c)。apply/remove/restart 失败后 30s 重试,最多 5 次;
//! 同 rule 的新命令到来时旧重试作废(防过期 Apply 复活已删规则)。
//! 队列只活在单个会话内:断线重连后 server reconcile 全量重放,无需跨会话保留。
use emorelay_common::control::v1::{command::Body, Command};
use std::time::{Duration, Instant};

pub const MAX_ATTEMPTS: u32 = 5;
pub const RETRY_DELAY: Duration = Duration::from_secs(30);

pub struct PendingCommand {
    pub cmd: Command,
    /// 已失败次数(含本次入队前那次)。
    pub attempts: u32,
}

struct Entry {
    cmd: Command,
    due: Instant,
    attempts: u32,
}

#[derive(Default)]
pub struct RetryQueue {
    items: Vec<Entry>,
}

/// 规则类命令的 rule_id;凭据类/空命令返回 None(不参与重试)。
fn rule_id_of(cmd: &Command) -> Option<i64> {
    match cmd.body.as_ref()? {
        Body::ApplyRule(a) => a.rule.as_ref().map(|r| r.id),
        Body::RemoveRule(r) => Some(r.rule_id),
        Body::RestartRule(r) => Some(r.rule_id),
        Body::EnableRule(r) => Some(r.rule_id),
        Body::DisableRule(r) => Some(r.rule_id),
        _ => None,
    }
}

impl RetryQueue {
    /// 失败命令入队。prev_attempts = 此前已失败次数;本次失败后 attempts = prev+1,
    /// 达到 MAX_ATTEMPTS 则放弃(返回 false)。同 rule 旧条目被替换。
    pub fn push_failed(&mut self, cmd: Command, prev_attempts: u32, now: Instant) -> bool {
        let Some(rid) = rule_id_of(&cmd) else { return false };
        let attempts = prev_attempts + 1;
        self.items.retain(|e| rule_id_of(&e.cmd) != Some(rid));
        if attempts >= MAX_ATTEMPTS {
            tracing::warn!(rule_id = rid, attempts, "command retry exhausted; giving up");
            return false;
        }
        self.items.push(Entry { cmd, due: now + RETRY_DELAY, attempts });
        true
    }

    /// 收到新命令时调用:同 rule 的挂起重试作废(新命令失败会重新入队)。
    pub fn supersede(&mut self, cmd: &Command) {
        if let Some(rid) = rule_id_of(cmd) {
            self.items.retain(|e| rule_id_of(&e.cmd) != Some(rid));
        }
    }

    /// 取出已到期的命令(从队列移除;调用方重试失败需再 push_failed)。
    pub fn take_due(&mut self, now: Instant) -> Vec<PendingCommand> {
        let (due, rest): (Vec<_>, Vec<_>) = self.items.drain(..).partition(|e| e.due <= now);
        self.items = rest;
        due.into_iter()
            .map(|e| PendingCommand { cmd: e.cmd, attempts: e.attempts })
            .collect()
    }
}
```

`lib.rs` 加 `pub mod retry;`（按字母序）。

- [ ] **Step 4: agent.rs 接线**

`run_session` 中：

```rust
    let mut retry = crate::retry::RetryQueue::default();
    let mut retry_tick = interval(Duration::from_secs(5));
    retry_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    retry_tick.tick().await;
```

命令分支改为（替换原 `Ok(Some(cmd))` 处理）：

```rust
                    Ok(Some(cmd)) => {
                        retry.supersede(&cmd);
                        if let Err(e) =
                            handle_command(&manager, &store, cmd.clone(), &config.data_dir).await
                        {
                            error!(error = ?e, "command apply failed; queued for retry");
                            retry.push_failed(cmd, 0, std::time::Instant::now());
                        }
                    }
```

`select!` 加分支：

```rust
            _ = retry_tick.tick() => {
                let now = std::time::Instant::now();
                for p in retry.take_due(now) {
                    if let Err(e) =
                        handle_command(&manager, &store, p.cmd.clone(), &config.data_dir).await
                    {
                        error!(error = ?e, attempts = p.attempts, "command retry failed");
                        retry.push_failed(p.cmd, p.attempts, now);
                    } else {
                        info!(attempts = p.attempts, "command retry succeeded");
                    }
                }
            }
```

- [ ] **Step 5: 跑测试验证通过**

Run: `cargo test -p node-agent retry && cargo test --workspace`
Expected: 全 PASS。

- [ ] **Step 6: Commit**

```bash
git add crates/node-agent/src/retry.rs crates/node-agent/src/lib.rs crates/node-agent/src/agent.rs
git commit -m "feat(agent): in-session retry queue for failed rule commands"
```

---

## Task 4: 后端 TunnelView.rules_count + TunnelDetail.rules

前端列表页要显示「关联规则数」，详情页要显示关联规则列表。在 `tunnels.rs` 响应内补齐，不动 `/api/rules`。

**Files:**
- Modify: `crates/panel-server/src/routes/tunnels.rs`
- Test: 追加 `crates/panel-server/tests/api_tunnels.rs`

- [ ] **Step 1: 写失败测试（追加 api_tunnels.rs）**

参考同文件既有 `delete_tunnel_blocked_by_rule_reference` 的建隧道+建规则方式（`seed_online_nodes` + REST）：

```rust
#[tokio::test]
async fn tunnel_views_expose_rules_count_and_detail_rules() {
    let app = common::make_app().await.unwrap();
    let nodes = seed_online_nodes(&app, 2).await;

    let req = common::auth_req(Method::POST, "/api/tunnels", &app.admin_token,
        Some(json!({ "name": "t-views", "transport": "tcp", "node_ids": nodes }))).unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    let tid = body["id"].as_i64().unwrap();

    // 挂一条入口规则。
    let req = common::auth_req(Method::POST, "/api/rules", &app.admin_token,
        Some(json!({
            "node_id": nodes[0], "name": "r-on-tunnel", "protocol": "tcp",
            "listen_port": 30005, "target_host": "10.0.0.9", "target_port": 80,
            "tunnel_id": tid,
        }))).unwrap();
    let (status, rule_body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{rule_body}");
    let rule_id = rule_body["id"].as_i64().unwrap();

    // list:rules_count = 1。
    let req = common::auth_req(Method::GET, "/api/tunnels", &app.admin_token, None).unwrap();
    let (_, body) = common::send(app.app.clone(), req).await.unwrap();
    let item = body["items"].as_array().unwrap().iter()
        .find(|t| t["id"].as_i64() == Some(tid)).expect("tunnel in list");
    assert_eq!(item["rules_count"].as_i64(), Some(1));

    // detail:rules 数组含该规则的 id/name/protocol/listen_port/enabled。
    let req = common::auth_req(Method::GET, &format!("/api/tunnels/{tid}"), &app.admin_token, None).unwrap();
    let (_, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(body["rules_count"].as_i64(), Some(1));
    let rules = body["rules"].as_array().unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0]["id"].as_i64(), Some(rule_id));
    assert_eq!(rules[0]["name"].as_str(), Some("r-on-tunnel"));
    assert_eq!(rules[0]["listen_port"].as_i64(), Some(30005));
    assert_eq!(rules[0]["enabled"].as_bool(), Some(true));
}
```

（若该测试文件中规则创建响应不是 `{"id": ...}` 形态，按 `api_tunnel_dispatch.rs` 既有规则创建断言对齐取 id 的方式，不改断言语义。）

- [ ] **Step 2: 跑测试验证失败**

Run: `cargo test -p panel-server --test api_tunnels`
Expected: FAIL（响应无 `rules_count`/`rules` 字段）。

- [ ] **Step 3: 实现**

`tunnels.rs` 结构体扩展：

```rust
#[derive(Serialize)]
pub struct TunnelView {
    pub id: i64,
    pub name: String,
    pub transport: String,
    pub status: String,
    pub hops_count: i64,
    pub rules_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// 隧道详情里的关联规则摘要(前端详情页列表用,不暴露完整 RuleView)。
#[derive(Serialize)]
pub struct TunnelRuleRef {
    pub id: i64,
    pub name: String,
    pub protocol: String,
    pub listen_port: i64,
    pub enabled: bool,
}
```

`TunnelDetail` 加 `pub rules_count: i64,` 与 `pub rules: Vec<TunnelRuleRef>,`。

`list` 循环内（已有 hops 查询旁）加：

```rust
        let rules_count = Tunnel::active_rule_refs(&state.pool, t.id).await?;
```

并把 `rules_count` 填入 `TunnelView`。`update` 处的 `TunnelView` 构造同样补 `rules_count: Tunnel::active_rule_refs(&state.pool, id).await?,`。

`get` 内加：

```rust
    let rules: Vec<TunnelRuleRef> = crate::models::rule::Rule::list_active_for_tunnel(&state.pool, id)
        .await?
        .into_iter()
        .map(|r| TunnelRuleRef {
            id: r.id,
            name: r.name,
            protocol: r.protocol,
            listen_port: r.listen_port,
            enabled: r.enabled != 0,
        })
        .collect();
```

`TunnelDetail` 构造补 `rules_count: rules.len() as i64, rules,`。

- [ ] **Step 4: 跑测试验证通过**

Run: `cargo test -p panel-server --test api_tunnels && cargo test --workspace`
Expected: 全 PASS。

- [ ] **Step 5: Commit**

```bash
git add crates/panel-server/src/routes/tunnels.rs crates/panel-server/tests/api_tunnels.rs
git commit -m "feat(server): tunnel views expose rules_count and detail rules list"
```

---

## Task 5: 前端 api.ts tunnels + Tunnels 列表页 + 路由/侧边栏

**Files:**
- Modify: `web/src/lib/api.ts`、`web/src/App.tsx`
- Create: `web/src/pages/Tunnels.tsx`
- Test: `web/src/pages/Tunnels.test.tsx`

- [ ] **Step 1: 写失败测试 `web/src/pages/Tunnels.test.tsx`**

```tsx
import { describe, expect, it, vi, beforeEach } from 'vitest'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import Tunnels from './Tunnels'
import { ToastProvider } from '../lib/toast'

vi.mock('../lib/api', async (importOriginal) => {
  const mod = await importOriginal<typeof import('../lib/api')>()
  return {
    ...mod,
    tunnels: {
      list: vi.fn().mockResolvedValue({
        items: [
          { id: 1, name: 'hk-jp', transport: 'tls', status: 'up', hops_count: 2, rules_count: 3, created_at: '2026-06-11 00:00:00', updated_at: '2026-06-11 00:00:00' },
          { id: 2, name: 'hk-jp-us', transport: 'tcp', status: 'degraded', hops_count: 3, rules_count: 0, created_at: '2026-06-11 00:00:00', updated_at: '2026-06-11 00:00:00' },
        ],
        total: 2, page: 1, page_size: 20,
      }),
      create: vi.fn().mockResolvedValue({ id: 9 }),
      del: vi.fn().mockResolvedValue({ ok: true }),
      restart: vi.fn().mockResolvedValue({ ok: true, dispatched: true }),
      get: vi.fn(), update: vi.fn(), status: vi.fn(),
    },
    nodes: {
      ...mod.nodes,
      list: vi.fn().mockResolvedValue({
        items: [
          { id: 11, name: 'hk-1', status: 'online', public_ip: '1.1.1.1' },
          { id: 12, name: 'jp-1', status: 'online', public_ip: '2.2.2.2' },
          { id: 13, name: 'us-1', status: 'online', public_ip: '3.3.3.3' },
        ],
        total: 3, page: 1, page_size: 100,
      }),
    },
  }
})

import { tunnels } from '../lib/api'

function renderPage() {
  return render(
    <ToastProvider>
      <MemoryRouter>
        <Tunnels />
      </MemoryRouter>
    </ToastProvider>,
  )
}

beforeEach(() => vi.clearAllMocks())

describe('Tunnels page', () => {
  it('renders tunnel list with transport, hops and rules count', async () => {
    renderPage()
    expect(await screen.findByText('hk-jp')).toBeInTheDocument()
    expect(screen.getByText('hk-jp-us')).toBeInTheDocument()
  })

  it('creates a tunnel from the chain builder', async () => {
    renderPage()
    await screen.findByText('hk-jp')
    fireEvent.click(screen.getByRole('button', { name: '创建隧道' }))
    // 默认两行节点下拉。
    const selects = await screen.findAllByLabelText(/节点 #/)
    expect(selects).toHaveLength(2)

    fireEvent.change(screen.getByLabelText('隧道名 *'), { target: { value: 't-new' } })
    fireEvent.change(selects[0], { target: { value: '11' } })
    fireEvent.change(selects[1], { target: { value: '12' } })
    fireEvent.click(screen.getByRole('button', { name: '创建' }))

    await waitFor(() =>
      expect(tunnels.create).toHaveBeenCalledWith({
        name: 't-new',
        transport: 'tls',
        node_ids: [11, 12],
      }),
    )
  })

  it('rejects duplicate nodes in the chain before submitting', async () => {
    renderPage()
    await screen.findByText('hk-jp')
    fireEvent.click(screen.getByRole('button', { name: '创建隧道' }))
    const selects = await screen.findAllByLabelText(/节点 #/)
    fireEvent.change(screen.getByLabelText('隧道名 *'), { target: { value: 't-dup' } })
    fireEvent.change(selects[0], { target: { value: '11' } })
    fireEvent.change(selects[1], { target: { value: '11' } })
    fireEvent.click(screen.getByRole('button', { name: '创建' }))
    expect(await screen.findByText(/节点不可重复/)).toBeInTheDocument()
    expect(tunnels.create).not.toHaveBeenCalled()
  })
})
```

- [ ] **Step 2: 跑测试验证失败**

Run: `cd web && npx vitest run src/pages/Tunnels.test.tsx`
Expected: FAIL（`./Tunnels` 模块不存在 / `tunnels` 未导出）。

- [ ] **Step 3: api.ts 加 tunnels 类型与端点**

`web/src/lib/api.ts`「数据类型」区追加：

```ts
export interface TunnelView {
  id: number
  name: string
  transport: 'tcp' | 'tls' | 'wss'
  status: 'up' | 'degraded' | 'down' | 'unknown'
  hops_count: number
  rules_count: number
  created_at: string
  updated_at: string
}

export interface TunnelHopView {
  ordinal: number
  node_id: number
  inter_port: number | null
}

export interface TunnelRuleRef {
  id: number
  name: string
  protocol: string
  listen_port: number
  enabled: boolean
}

export interface TunnelDetailView {
  id: number
  name: string
  transport: TunnelView['transport']
  status: TunnelView['status']
  hops: TunnelHopView[]
  rules_count: number
  rules: TunnelRuleRef[]
  created_at: string
  updated_at: string
}

export interface TunnelListResponse {
  items: TunnelView[]
  total: number
  page: number
  page_size: number
}

export interface CreateTunnelRequest {
  name: string
  transport: TunnelView['transport']
  node_ids: number[]
}
```

「端点」区追加：

```ts
export const tunnels = {
  list: (q: { page?: number; page_size?: number } = {}) => {
    const sp = new URLSearchParams()
    if (q.page) sp.set('page', String(q.page))
    if (q.page_size) sp.set('page_size', String(q.page_size))
    return api.get<TunnelListResponse>(`/api/tunnels?${sp.toString()}`)
  },
  get: (id: number) => api.get<TunnelDetailView>(`/api/tunnels/${id}`),
  create: (req: CreateTunnelRequest) => api.post<{ id: number }>('/api/tunnels', req),
  update: (id: number, req: { name: string }) => api.patch<TunnelView>(`/api/tunnels/${id}`, req),
  del: (id: number) => api.del<{ ok: boolean }>(`/api/tunnels/${id}`),
  restart: (id: number) => api.post<{ ok: boolean; dispatched: boolean }>(`/api/tunnels/${id}/restart`),
  status: (id: number) => api.get<{ id: number; status: TunnelView['status'] }>(`/api/tunnels/${id}/status`),
}
```

同时补隧道关联字段（本 Task 一并做，Task 7 直接使用）：`RuleView` 加 `tunnel_id: number | null`；`CreateRuleRequest` 加 `tunnel_id?: number | null`。

- [ ] **Step 4: 实现 Tunnels.tsx**

模仿 `Nodes.tsx` 的列表 + Modal + toast 模式。要点（完整组件按此契约实现）：

```tsx
import { useCallback, useEffect, useState, type FormEvent } from 'react'
import { Link } from 'react-router-dom'
import {
  ApiError, nodes, tunnels, shortTime,
  type CreateTunnelRequest, type NodeView, type TunnelView,
} from '../lib/api'
import { Modal, StatusDot, fieldInputCls, fieldLabelCls } from '../lib/ui'
import Pagination from '../components/Pagination'
import { useToast } from '../lib/use-toast'
```

- 列表加载：`useEffect` + `tunnels.list({ page, page_size })`，含 loading/error state 与 cancelled flag（仿 Nodes.tsx）。
- 节点加载（创建表单用）：`nodes.list({ page_size: 100 })` 一次。
- 表格列：名称（`<Link to={`/tunnels/${t.id}`}>`）、transport（大写显示）、状态（`<StatusDot>` 映射 up→on / degraded→unknown / down→off / unknown→unknown，旁边写文字）、`hops_count`、`rules_count`、`shortTime(created_at)`、操作（重启 / 删除）。
- 重启：`tunnels.restart(id)` → toast 成功/失败。
- 删除：确认 Modal → `tunnels.del(id)`；后端有活跃规则引用时返回 400，错误文案直接 toast 展示。
- 创建 Modal `TunnelForm`：
  - `name`（必填 input，label「隧道名 *」）、`transport`（select：TLS（推荐）/TCP/WSS，默认 `tls`）。
  - 节点链构造器：`const [chain, setChain] = useState<string[]>(['', ''])`；每行渲染 `<label>节点 #{i + 1}{i === 0 ? '（入口）' : ''}</label>` + select（option = 在线节点 `n.name`，value=`n.id`；非 online 节点不出现在选项中）+「↑」「↓」按钮（首/末行禁用）+「移除」按钮（`chain.length > 2` 才显示）；底部「+ 添加节点」按钮 push `''`。select 需设 `id`/`htmlFor` 或 `aria-label={`节点 #${i + 1}`}` 使测试 `getByLabelText(/节点 #/)` 可定位。
  - 提交校验：所有行非空（「请为每个 hop 选择节点」）、无重复（「节点不可重复」）、`chain.length >= 2`（UI 上移除按钮已保证）。
  - 提交：`tunnels.create({ name, transport, node_ids: chain.map(Number) })` → 成功后关 Modal + 刷新列表 + toast；失败 `setError(e instanceof ApiError ? e.message : '提交失败')` 表单内展示。
  - 表单内提示文案：「第 2 跳起的节点必须配置公网 IP；所有节点须在线」。

- [ ] **Step 5: App.tsx 接路由与侧边栏**

- import：`import Tunnels from './pages/Tunnels'`、`import TunnelDetail from './pages/TunnelDetail'`（TunnelDetail 在 Task 6 创建——**本 Task 先建占位文件** `web/src/pages/TunnelDetail.tsx`：

```tsx
export default function TunnelDetail() {
  return null // P3c Task 6 实现
}
```

- Route（`rules/:id` 之后）：

```tsx
<Route path="tunnels" element={<Tunnels />} />
<Route path="tunnels/:id" element={<TunnelDetail />} />
```

- NavItem（「规则」之后）：`<NavItem to="/tunnels" label="隧道" onClick={() => setDrawerOpen(false)} />`
- `CurrentRoute` 的 labels 加 `'/tunnels': '隧道',`。

- [ ] **Step 6: 跑测试验证通过**

Run: `cd web && npx vitest run && npm run build`
Expected: 全 PASS + build 零错误。

- [ ] **Step 7: Commit**

```bash
git add web/src/lib/api.ts web/src/pages/Tunnels.tsx web/src/pages/TunnelDetail.tsx web/src/pages/Tunnels.test.tsx web/src/App.tsx
git commit -m "feat(web): tunnels list page with chain builder; /tunnels route and sidebar entry"
```

---

## Task 6: 前端 TunnelDetail 详情页

**Files:**
- Modify: `web/src/pages/TunnelDetail.tsx`（替换 Task 5 占位）
- Test: `web/src/pages/TunnelDetail.test.tsx`

- [ ] **Step 1: 写失败测试**

```tsx
import { describe, expect, it, vi } from 'vitest'
import { render, screen } from '@testing-library/react'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import TunnelDetail from './TunnelDetail'
import { ToastProvider } from '../lib/toast'

vi.mock('../lib/api', async (importOriginal) => {
  const mod = await importOriginal<typeof import('../lib/api')>()
  return {
    ...mod,
    tunnels: {
      ...mod.tunnels,
      get: vi.fn().mockResolvedValue({
        id: 5, name: 'hk-jp-us', transport: 'tls', status: 'up',
        hops: [
          { ordinal: 0, node_id: 11, inter_port: null },
          { ordinal: 1, node_id: 12, inter_port: 30001 },
          { ordinal: 2, node_id: 13, inter_port: 30002 },
        ],
        rules_count: 1,
        rules: [{ id: 77, name: 'r-game', protocol: 'tcp', listen_port: 20000, enabled: true }],
        created_at: '2026-06-11 00:00:00', updated_at: '2026-06-11 00:00:00',
      }),
      restart: vi.fn().mockResolvedValue({ ok: true, dispatched: true }),
    },
    nodes: {
      ...mod.nodes,
      list: vi.fn().mockResolvedValue({
        items: [
          { id: 11, name: 'hk-1' }, { id: 12, name: 'jp-1' }, { id: 13, name: 'us-1' },
        ],
        total: 3, page: 1, page_size: 100,
      }),
    },
  }
})

function renderPage() {
  return render(
    <ToastProvider>
      <MemoryRouter initialEntries={['/tunnels/5']}>
        <Routes>
          <Route path="/tunnels/:id" element={<TunnelDetail />} />
        </Routes>
      </MemoryRouter>
    </ToastProvider>,
  )
}

describe('TunnelDetail page', () => {
  it('renders hop chain with roles and node names', async () => {
    renderPage()
    expect(await screen.findByText('hk-jp-us')).toBeInTheDocument()
    expect(screen.getByText('Entry')).toBeInTheDocument()
    expect(screen.getByText('Mid')).toBeInTheDocument()
    expect(screen.getByText('Exit')).toBeInTheDocument()
    expect(screen.getByText('hk-1')).toBeInTheDocument()
    expect(screen.getByText('jp-1')).toBeInTheDocument()
    expect(screen.getByText('30001')).toBeInTheDocument()
  })

  it('renders associated rules', async () => {
    renderPage()
    expect(await screen.findByText('r-game')).toBeInTheDocument()
    expect(screen.getByText('20000')).toBeInTheDocument()
  })
})
```

- [ ] **Step 2: 跑测试验证失败**

Run: `cd web && npx vitest run src/pages/TunnelDetail.test.tsx`
Expected: FAIL（占位组件返回 null）。

- [ ] **Step 3: 实现 TunnelDetail.tsx**

模仿 `NodeDetail.tsx` 模式。契约：

- `useParams()` 取 id，NaN 守卫。
- `useEffect` + `Promise.all([tunnels.get(id), nodes.list({ page_size: 100 })])`，cancelled flag；node_id → name 映射（`Map<number, string>`，缺失显示 `#<node_id>`）。
- 头部：隧道名 + transport 徽标 + 状态 `StatusDot` + 「重启隧道」按钮（`tunnels.restart` + toast）+ 返回 `/tunnels` 链接。
- hops 表：列 = 序号（ordinal）、角色（ordinal 0→`Entry`，最后一个→`Exit`，其余→`Mid`；双跳时 ordinal 1 是 `Exit`）、节点名、`inter_port`（null 显示 `-`，entry 无监听）。
- 关联规则表：来自 `detail.rules`，列 = 名称（`<Link to={`/rules/${r.id}`}>`）、协议、监听端口、状态（启用/停用）；空态文案「暂无关联规则」。
- loading / error 态与 NodeDetail 一致。

- [ ] **Step 4: 跑测试验证通过**

Run: `cd web && npx vitest run && npm run build`
Expected: 全 PASS + build 零错误。

- [ ] **Step 5: Commit**

```bash
git add web/src/pages/TunnelDetail.tsx web/src/pages/TunnelDetail.test.tsx
git commit -m "feat(web): tunnel detail page with hop chain and associated rules"
```

---

## Task 7: Rules 表单「关联隧道」下拉

选隧道后 node_id 自动填入口节点并禁用节点下拉（spec §4.7：入口规则必须落在 ordinal 0 节点，后端已校验）。

**Files:**
- Modify: `web/src/pages/Rules.tsx`
- Test: `web/src/pages/RuleForm.test.tsx`

- [ ] **Step 1: 导出 RuleForm 以便测试**

`Rules.tsx` 中 `function RuleForm(` 改为 `export function RuleForm(`。

- [ ] **Step 2: 写失败测试 `web/src/pages/RuleForm.test.tsx`**

```tsx
import { describe, expect, it, vi, beforeEach } from 'vitest'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { RuleForm } from './Rules'

vi.mock('../lib/api', async (importOriginal) => {
  const mod = await importOriginal<typeof import('../lib/api')>()
  return {
    ...mod,
    rules: { ...mod.rules, create: vi.fn().mockResolvedValue({ id: 1 }) },
    tunnels: {
      ...mod.tunnels,
      get: vi.fn().mockResolvedValue({
        id: 5, name: 'hk-jp', transport: 'tls', status: 'up',
        hops: [
          { ordinal: 0, node_id: 12, inter_port: null },
          { ordinal: 1, node_id: 11, inter_port: 30001 },
        ],
        rules_count: 0, rules: [],
        created_at: '', updated_at: '',
      }),
    },
  }
})

import { rules, tunnels } from '../lib/api'

const nodeList = [
  { id: 11, name: 'us-1', port_pool_min: 30000, port_pool_max: 31000 },
  { id: 12, name: 'hk-1', port_pool_min: 30000, port_pool_max: 31000 },
] as never[]
const tunnelList = [
  { id: 5, name: 'hk-jp', transport: 'tls', status: 'up', hops_count: 2, rules_count: 0, created_at: '', updated_at: '' },
] as never[]

beforeEach(() => vi.clearAllMocks())

describe('RuleForm tunnel association', () => {
  it('selecting a tunnel locks node select to the entry node and submits tunnel_id', async () => {
    render(
      <RuleForm
        mode="create"
        nodeList={nodeList}
        profiles={[]}
        tunnelList={tunnelList}
        onCancel={() => {}}
        onSuccess={() => {}}
      />,
    )
    fireEvent.change(screen.getByLabelText('关联隧道'), { target: { value: '5' } })
    await waitFor(() => expect(tunnels.get).toHaveBeenCalledWith(5))

    const nodeSelect = screen.getByLabelText('节点 *') as HTMLSelectElement
    await waitFor(() => expect(nodeSelect.value).toBe('12'))
    expect(nodeSelect).toBeDisabled()

    fireEvent.change(screen.getByLabelText('规则名 *'), { target: { value: 'r1' } })
    fireEvent.change(screen.getByLabelText('目标主机 *'), { target: { value: '10.0.0.1' } })
    fireEvent.change(screen.getByLabelText('目标端口 *'), { target: { value: '80' } })
    fireEvent.click(screen.getByRole('button', { name: '创建' }))

    await waitFor(() =>
      expect(rules.create).toHaveBeenCalledWith(
        expect.objectContaining({ node_id: 12, tunnel_id: 5 }),
      ),
    )
  })

  it('without tunnel the node select stays editable and tunnel_id is null', async () => {
    render(
      <RuleForm
        mode="create"
        nodeList={nodeList}
        profiles={[]}
        tunnelList={tunnelList}
        onCancel={() => {}}
        onSuccess={() => {}}
      />,
    )
    expect(screen.getByLabelText('节点 *')).not.toBeDisabled()
    fireEvent.change(screen.getByLabelText('规则名 *'), { target: { value: 'r2' } })
    fireEvent.change(screen.getByLabelText('目标主机 *'), { target: { value: '10.0.0.1' } })
    fireEvent.change(screen.getByLabelText('目标端口 *'), { target: { value: '80' } })
    fireEvent.click(screen.getByRole('button', { name: '创建' }))
    await waitFor(() =>
      expect(rules.create).toHaveBeenCalledWith(
        expect.objectContaining({ tunnel_id: null }),
      ),
    )
  })
})
```

注意：`getByLabelText` 需要 label 与控件关联——现有表单 label 没有 `htmlFor`。给本 Task 涉及的「节点 *」「规则名 *」「目标主机 *」「目标端口 *」「关联隧道」五个控件补 `id` + `htmlFor`（其余字段不动）。

- [ ] **Step 3: 跑测试验证失败**

Run: `cd web && npx vitest run src/pages/RuleForm.test.tsx`
Expected: FAIL（RuleForm 无 `tunnelList` prop / 无「关联隧道」控件）。

- [ ] **Step 4: 实现**

`Rules.tsx`：

1. Rules 页组件：`import { useAuth } from '../lib/use-auth'` + `import { tunnels, type TunnelView } from '../lib/api'`（并入既有 import）。仿 profiles 加载（admin 才有权限调 tunnels REST）：

```tsx
  const { user } = useAuth()
  const [tunnelList, setTunnelList] = useState<TunnelView[]>([])
  useEffect(() => {
    if (user?.role !== 'admin') return
    let cancelled = false
    tunnels
      .list({ page_size: 100 })
      .then((r) => { if (!cancelled) setTunnelList(r.items) })
      .catch(() => {}) // 非关键数据,失败静默(表单退化为无隧道下拉)
    return () => { cancelled = true }
  }, [user?.role])
```

创建/编辑两处 `<RuleForm ... />` 调用补 `tunnelList={tunnelList}`。

2. `RuleFormState` 加 `tunnel_id: string`；初始化 `tunnel_id: initial?.tunnel_id != null ? String(initial.tunnel_id) : ''`。RuleForm props 加 `tunnelList: TunnelView[]`。

3. 入口节点联动 state 与 effect（RuleForm 内）：

```tsx
  const [entryNodeId, setEntryNodeId] = useState<number | null>(null)
  useEffect(() => {
    if (!form.tunnel_id) {
      setEntryNodeId(null)
      return
    }
    let cancelled = false
    tunnelsApi
      .get(Number(form.tunnel_id))
      .then((d) => {
        if (cancelled) return
        const entry = d.hops.find((h) => h.ordinal === 0)
        if (entry) {
          setEntryNodeId(entry.node_id)
          setForm((f) => ({ ...f, node_id: String(entry.node_id) }))
        }
      })
      .catch(() => { if (!cancelled) setError('加载隧道入口节点失败') })
    return () => { cancelled = true }
  }, [form.tunnel_id])
```

（import 命名冲突时用 `import { tunnels as tunnelsApi } from '../lib/api'`。）

4. 「关联隧道」select（节点/协议行之后、规则名之前；编辑模式 disabled，同 node/protocol 语义）：

```tsx
      {tunnelList.length > 0 && (
        <div>
          <label htmlFor="rule-tunnel" className={fieldLabelCls}>关联隧道</label>
          <select
            id="rule-tunnel"
            value={form.tunnel_id}
            onChange={(e) => set('tunnel_id', e.target.value)}
            disabled={mode === 'edit'}
            className={fieldInputCls}
          >
            <option value="">不走隧道</option>
            {tunnelList.map((t) => (
              <option key={t.id} value={t.id}>
                {t.name}（{t.transport.toUpperCase()} · {t.hops_count} 跳）
              </option>
            ))}
          </select>
          <p className="text-[11px] text-zinc-500 mt-1">
            选择隧道后,规则将落在隧道入口节点,流量经隧道链转发至目标。
          </p>
        </div>
      )}
```

5. 节点 select 的 `disabled` 改为 `disabled={mode === 'edit' || entryNodeId != null}`。

6. create 提交 payload 加 `tunnel_id: form.tunnel_id ? Number(form.tunnel_id) : null,`。（编辑模式后端不支持改 tunnel_id，不发送。）

- [ ] **Step 5: 跑测试验证通过**

Run: `cd web && npx vitest run && npm run build`
Expected: 全 PASS + build 零错误。

- [ ] **Step 6: Commit**

```bash
git add web/src/pages/Rules.tsx web/src/pages/RuleForm.test.tsx
git commit -m "feat(web): rule form tunnel association locks node to tunnel entry"
```

---

## Task 8: e2e 基建 + 双跳 TCP 端到端

panel-server dev-dep 引入 node-agent，in-process：真 SQLite + 真 REST + 真 gRPC server（plaintext dev 模式）+ 2 个真 agent + 真 TCP 流量。

**Files:**
- Modify: `crates/panel-server/Cargo.toml`（dev-dependencies 加 node-agent）
- Create: `crates/panel-server/tests/tunnel_e2e.rs`

- [ ] **Step 1: Cargo.toml**

`[dev-dependencies]` 加：

```toml
# P3c 隧道 e2e:in-process 起真 agent。
node-agent = { path = "../node-agent" }
```

- [ ] **Step 2: 写 e2e 测试（首测即基建——helpers + 双跳 TCP）**

`crates/panel-server/tests/tunnel_e2e.rs`：

```rust
//! P3c 隧道端到端:真 panel-server(REST + gRPC plaintext dev 模式) + 真 node-agent
//! (in-process run_agent) + 真 TCP/UDP 流量。每个测试用独立端口段防并行互撞。
mod common;

use axum::http::{Method, StatusCode};
use common::{auth_req, make_app, send, TestApp};
use panel_server::grpc::service::ControlPlaneImpl;
use emorelay_common::control::v1::control_plane_server::ControlPlaneServer;
use serde_json::json;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tonic::transport::Server;

/// 起 in-process gRPC 控制面(plaintext;tests/common 的 Config dev_disable_mtls=true)。
async fn start_grpc(app: &TestApp) -> SocketAddr {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    let svc = ControlPlaneServer::new(ControlPlaneImpl::new(app.state.clone()));
    tokio::spawn(async move {
        let _ = Server::builder().add_service(svc).serve(addr).await;
    });
    // 等 server 可连。
    for _ in 0..50 {
        if TcpStream::connect(addr).await.is_ok() {
            return addr;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("grpc server not up at {addr}");
}

/// REST 创建节点(public_ip=127.0.0.1 供下游 hop dial),返回 (node_id, agent_token)。
async fn create_node(app: &TestApp, name: &str, pool: (i64, i64)) -> (i64, String) {
    let req = auth_req(Method::POST, "/api/nodes", &app.admin_token, Some(json!({
        "name": name,
        "public_ip": "127.0.0.1",
        "grpc_endpoint": "127.0.0.1:0",
        "port_pool_min": pool.0,
        "port_pool_max": pool.1,
    }))).unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    (
        body["node"]["id"].as_i64().unwrap(),
        body["agent_token"].as_str().unwrap().to_string(),
    )
}

/// in-process 起真 agent(plaintext 控制面)。返回 handle 供测试尾 abort。
fn spawn_agent(
    grpc: SocketAddr,
    node_id: i64,
    token: String,
    dir: &tempfile::TempDir,
) -> tokio::task::JoinHandle<()> {
    let base = dir.path().display().to_string().replace('\\', "/");
    let cfg = node_agent::config::Config {
        node_id,
        control_endpoint: format!("http://{grpc}"),
        token,
        state_path: format!("{base}/agent-state.json"),
        data_dir: base,
        grpc_ca_cert: None,
        grpc_client_cert: None,
        grpc_client_key: None,
    };
    tokio::spawn(async move {
        let _ = node_agent::run_agent(cfg).await;
    })
}

async fn wait_node_online(app: &TestApp, node_id: i64) {
    for _ in 0..100 {
        let (status,): (String,) = sqlx::query_as("SELECT status FROM nodes WHERE id = ?")
            .bind(node_id)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
        if status == "online" {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("node {node_id} never came online");
}

/// 简单 TCP echo 目标服务。
async fn start_tcp_echo() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else { break };
            tokio::spawn(async move {
                let (mut r, mut w) = s.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    addr
}

/// 入口端口可达前轮询重连(agent apply 是异步下发)。
async fn connect_entry(port: u16) -> TcpStream {
    for _ in 0..100 {
        if let Ok(s) = TcpStream::connect(("127.0.0.1", port)).await {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("entry port {port} never accepted");
}

/// REST 建隧道 → 返回 tunnel_id。
async fn create_tunnel(app: &TestApp, name: &str, transport: &str, node_ids: &[i64]) -> i64 {
    let req = auth_req(Method::POST, "/api/tunnels", &app.admin_token, Some(json!({
        "name": name, "transport": transport, "node_ids": node_ids,
    }))).unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    body["id"].as_i64().unwrap()
}

/// REST 建隧道入口规则(listen_ip=127.0.0.1)。
async fn create_tunnel_rule(
    app: &TestApp, entry_node: i64, tunnel_id: i64, protocol: &str,
    listen_port: u16, target: SocketAddr,
) -> i64 {
    let req = auth_req(Method::POST, "/api/rules", &app.admin_token, Some(json!({
        "node_id": entry_node,
        "name": format!("e2e-{protocol}-{listen_port}"),
        "protocol": protocol,
        "listen_ip": "127.0.0.1",
        "listen_port": listen_port,
        "target_host": "127.0.0.1",
        "target_port": target.port(),
        "tunnel_id": tunnel_id,
    }))).unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    body["id"].as_i64().unwrap()
}

/// 双跳 TCP:client → entry(n1, 21500) → exit(n2, inter_port) → echo。
#[tokio::test]
async fn two_hop_tcp_tunnel_end_to_end() {
    let app = make_app().await.unwrap();
    let grpc = start_grpc(&app).await;

    let (n1, t1) = create_node(&app, "e2e-hop-0", (21000, 21099)).await;
    let (n2, t2) = create_node(&app, "e2e-hop-1", (21100, 21199)).await;

    let d1 = tempfile::TempDir::new().unwrap();
    let d2 = tempfile::TempDir::new().unwrap();
    let a1 = spawn_agent(grpc, n1, t1, &d1);
    let a2 = spawn_agent(grpc, n2, t2, &d2);
    wait_node_online(&app, n1).await;
    wait_node_online(&app, n2).await;

    let echo = start_tcp_echo().await;
    let tid = create_tunnel(&app, "e2e-2hop-tcp", "tcp", &[n1, n2]).await;
    create_tunnel_rule(&app, n1, tid, "tcp", 21500, echo).await;

    let mut s = connect_entry(21500).await;
    s.write_all(b"hello-tunnel").await.unwrap();
    let mut buf = [0u8; 12];
    s.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"hello-tunnel", "双跳 TCP 隧道必须把字节原样送达 echo 并返回");

    a1.abort();
    a2.abort();
}
```

（`ControlPlaneImpl` 的构造与可见性按 `agent_e2e.rs` 既有用法对齐；若该文件 import 路径不同，以 agent_e2e.rs 为准，不另发明。）

- [ ] **Step 3: 跑测试验证（首次即真验证——基建无独立失败态，红→绿体现在调通过程）**

Run: `cargo test -p panel-server --test tunnel_e2e`
Expected: PASS。常见排障：agent register 失败查 token/endpoint；entry 不通查 dispatcher 是否推送了 split 后的两条 Rule（n1 entry / n2 exit）。

- [ ] **Step 4: 全量回归 + Commit**

Run: `cargo test --workspace`

```bash
git add crates/panel-server/Cargo.toml crates/panel-server/tests/tunnel_e2e.rs Cargo.lock
git commit -m "test(e2e): two-hop TCP tunnel end-to-end with real in-process agents"
```

---

## Task 9: e2e 三跳 TCP + 双跳/三跳 TLS 矩阵

**Files:**
- Modify: `crates/panel-server/tests/tunnel_e2e.rs`

- [ ] **Step 1: 抽取场景驱动函数 + 写三个失败测试**

把 Task 8 测试体抽成参数化驱动（Task 8 的测试改为调用它）：

```rust
/// 矩阵驱动:n_hops 个节点 × transport,验证 TCP 字节往返。
/// port_base:节点 i 的 pool = [port_base + i*100, port_base + i*100 + 99],
/// entry listen = port_base + 900。
async fn run_tcp_tunnel_matrix(n_hops: usize, transport: &str, port_base: u16) {
    let app = make_app().await.unwrap();
    let grpc = start_grpc(&app).await;

    let mut node_ids = Vec::new();
    let mut handles = Vec::new();
    let mut dirs = Vec::new();
    for i in 0..n_hops {
        let lo = port_base + (i as u16) * 100;
        let (nid, token) =
            create_node(&app, &format!("e2e-{transport}-{n_hops}h-{i}"), (lo as i64, (lo + 99) as i64)).await;
        let dir = tempfile::TempDir::new().unwrap();
        handles.push(spawn_agent(grpc, nid, token, &dir));
        dirs.push(dir);
        node_ids.push(nid);
    }
    for nid in &node_ids {
        wait_node_online(&app, *nid).await;
    }

    let echo = start_tcp_echo().await;
    let tid = create_tunnel(&app, &format!("e2e-{transport}-{n_hops}hop"), transport, &node_ids).await;
    let listen = port_base + 900;
    create_tunnel_rule(&app, node_ids[0], tid, "tcp", listen, echo).await;

    let mut s = connect_entry(listen).await;
    let payload = format!("ping-{transport}-{n_hops}");
    s.write_all(payload.as_bytes()).await.unwrap();
    let mut buf = vec![0u8; payload.len()];
    s.read_exact(&mut buf).await.unwrap();
    assert_eq!(buf, payload.as_bytes());

    for h in handles {
        h.abort();
    }
}

#[tokio::test]
async fn three_hop_tcp_tunnel_end_to_end() {
    run_tcp_tunnel_matrix(3, "tcp", 22000).await;
}

#[tokio::test]
async fn two_hop_tls_tunnel_end_to_end() {
    run_tcp_tunnel_matrix(2, "tls", 23000).await;
}

#[tokio::test]
async fn three_hop_tls_tunnel_end_to_end() {
    run_tcp_tunnel_matrix(3, "tls", 24000).await;
}
```

Task 8 的 `two_hop_tcp_tunnel_end_to_end` 改为 `run_tcp_tunnel_matrix(2, "tcp", 21000).await;`（保留原测试名）。

注意 TLS 链路语义即 e2e 验收点：服务端创建隧道时签发凭据并下发（`dispatch_tunnel_credentials`）→ agent `creds::store` 落盘 → apply 时 `TlsTransport::load` 加载 → dial SNI/SAN 对齐 + **Task 1 的 client SAN 校验在真链路全程生效**（mid/exit accept 时校验上一跳）。TLS 测试通过本身就证明正路径校验不误伤；负路径（伪造 SAN 被拒）已由 Task 1 单测覆盖，不重复入 e2e。

- [ ] **Step 2: 跑测试验证**

Run: `cargo test -p panel-server --test tunnel_e2e`
Expected: 4 个测试全 PASS。TLS 失败时优先查 agent 日志顺序：credentials received 必须先于 apply rule（reconcile/dispatch 顺序已有集成测试保障）。

- [ ] **Step 3: 全量回归 + Commit**

Run: `cargo test --workspace`

```bash
git add crates/panel-server/tests/tunnel_e2e.rs
git commit -m "test(e2e): three-hop TCP and two/three-hop TLS tunnel matrix"
```

---

## Task 10: e2e UDP-over-tunnel

**Files:**
- Modify: `crates/panel-server/tests/tunnel_e2e.rs`

- [ ] **Step 1: 写失败测试**

```rust
/// 简单 UDP echo 目标服务。
async fn start_udp_echo() -> SocketAddr {
    let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = sock.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65536];
        loop {
            let Ok((n, peer)) = sock.recv_from(&mut buf).await else { break };
            let _ = sock.send_to(&buf[..n], peer).await;
        }
    });
    addr
}

/// 双跳 UDP-over-tunnel:UDP 包在 entry 打 2 字节长度前缀帧,经隧道流送 exit 拆帧
/// 转发 target,回程同理(P3b frame.rs 协议的全链路实测)。
#[tokio::test]
async fn two_hop_udp_over_tunnel_end_to_end() {
    let app = make_app().await.unwrap();
    let grpc = start_grpc(&app).await;

    let (n1, t1) = create_node(&app, "e2e-udp-0", (25000, 25099)).await;
    let (n2, t2) = create_node(&app, "e2e-udp-1", (25100, 25199)).await;
    let d1 = tempfile::TempDir::new().unwrap();
    let d2 = tempfile::TempDir::new().unwrap();
    let a1 = spawn_agent(grpc, n1, t1, &d1);
    let a2 = spawn_agent(grpc, n2, t2, &d2);
    wait_node_online(&app, n1).await;
    wait_node_online(&app, n2).await;

    let echo = start_udp_echo().await;
    let tid = create_tunnel(&app, "e2e-2hop-udp", "tcp", &[n1, n2]).await;
    create_tunnel_rule(&app, n1, tid, "udp", 25900, echo).await;

    // UDP 无连接,入口就绪不可探测:发包 + 限时等回包,失败重发(本地回环丢包率≈0,
    // 重试覆盖的是 agent apply 异步延迟)。
    let client = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    client.connect(("127.0.0.1", 25900)).await.unwrap();
    let mut buf = [0u8; 16];
    let mut got = None;
    for _ in 0..50 {
        let _ = client.send(b"udp-ping").await;
        match tokio::time::timeout(Duration::from_millis(200), client.recv(&mut buf)).await {
            Ok(Ok(n)) => {
                got = Some(buf[..n].to_vec());
                break;
            }
            _ => continue,
        }
    }
    assert_eq!(got.as_deref(), Some(b"udp-ping".as_slice()), "UDP 必须经隧道帧封装往返");

    a1.abort();
    a2.abort();
}
```

- [ ] **Step 2: 跑测试验证**

Run: `cargo test -p panel-server --test tunnel_e2e two_hop_udp`
Expected: PASS。

- [ ] **Step 3: 全量回归 + Commit**

Run: `cargo test --workspace`

```bash
git add crates/panel-server/tests/tunnel_e2e.rs
git commit -m "test(e2e): UDP-over-tunnel frame round trip through two-hop tunnel"
```

---

## Task 11: e2e mTLS 真链路 + 吊销拒绝

P3a 留到 P3c 的两条真链路验收：「Agent 带 client cert 连上 mTLS gRPC + 在线」「吊销后旧 cert 重连被拒」。

**Files:**
- Modify: `crates/panel-server/tests/tunnel_e2e.rs`（或拆 `tests/mtls_e2e.rs`，复用 helper 时优先同文件）

- [ ] **Step 1: 写失败测试**

```rust
use tonic::transport::{Certificate, ClientTlsConfig, Identity, ServerTlsConfig};

/// 起 mTLS gRPC 控制面(内置 CA server cert + 强制 client cert)。
async fn start_grpc_mtls(app: &TestApp) -> SocketAddr {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    let ca = app.state.ca.clone();
    let identity = Identity::from_pem(ca.server_cert_pem.as_bytes(), ca.server_key_pem.as_bytes());
    let tls = ServerTlsConfig::new()
        .identity(identity)
        .client_ca_root(Certificate::from_pem(ca.ca_pem.as_bytes()));
    let svc = ControlPlaneServer::new(ControlPlaneImpl::new(app.state.clone()));
    tokio::spawn(async move {
        let _ = Server::builder()
            .tls_config(tls)
            .unwrap()
            .add_service(svc)
            .serve(addr)
            .await;
    });
    for _ in 0..50 {
        if TcpStream::connect(addr).await.is_ok() {
            return addr;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("mtls grpc server not up");
}

/// 真 agent 带 client cert 走 mTLS register → 在线;吊销后旧 cert 重连 register 被拒。
#[tokio::test]
async fn mtls_agent_register_and_revocation_rejects_old_cert() {
    let app = make_app().await.unwrap();
    let grpc = start_grpc_mtls(&app).await;

    // 创建节点拿四件套,落盘到 agent 目录。
    let req = auth_req(Method::POST, "/api/nodes", &app.admin_token, Some(json!({
        "name": "e2e-mtls-node", "public_ip": "127.0.0.1", "grpc_endpoint": "127.0.0.1:0",
        "port_pool_min": 26000, "port_pool_max": 26099,
    }))).unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    let node_id = body["node"]["id"].as_i64().unwrap();
    let token = body["agent_token"].as_str().unwrap().to_string();
    let ca_pem = body["ca_pem"].as_str().unwrap().to_string();
    let cert_pem = body["client_cert_pem"].as_str().unwrap().to_string();
    let key_pem = body["client_key_pem"].as_str().unwrap().to_string();

    let dir = tempfile::TempDir::new().unwrap();
    let base = dir.path().display().to_string().replace('\\', "/");
    std::fs::write(format!("{base}/ca.pem"), &ca_pem).unwrap();
    std::fs::write(format!("{base}/client.pem"), &cert_pem).unwrap();
    std::fs::write(format!("{base}/client.key"), &key_pem).unwrap();

    // 真 agent 带 client cert 连 https endpoint(server cert SAN 含 localhost)。
    let cfg = node_agent::config::Config {
        node_id,
        control_endpoint: format!("https://localhost:{}", grpc.port()),
        token: token.clone(),
        state_path: format!("{base}/agent-state.json"),
        data_dir: base.clone(),
        grpc_ca_cert: Some(format!("{base}/ca.pem")),
        grpc_client_cert: Some(format!("{base}/client.pem")),
        grpc_client_key: Some(format!("{base}/client.key")),
    };
    let agent = tokio::spawn(async move { let _ = node_agent::run_agent(cfg).await; });
    wait_node_online(&app, node_id).await; // mTLS 真链路 register 成功
    agent.abort();

    // 吊销凭据:旧 fingerprint 进 CRL。
    let req = auth_req(Method::POST, &format!("/api/nodes/{node_id}/revoke-credentials"),
        &app.admin_token, None).unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);

    // 用旧 client cert 直连 register → CRL 拒证(PermissionDenied)。
    let tls = ClientTlsConfig::new()
        .ca_certificate(Certificate::from_pem(ca_pem.as_bytes()))
        .identity(Identity::from_pem(cert_pem.as_bytes(), key_pem.as_bytes()))
        .domain_name("localhost");
    let channel = tonic::transport::Channel::from_shared(format!("https://localhost:{}", grpc.port()))
        .unwrap()
        .tls_config(tls)
        .unwrap()
        .connect()
        .await
        .expect("TLS 握手层面旧证书仍有效(链未变),拒绝发生在 register 的 CRL 检查");
    let mut client =
        emorelay_common::control::v1::control_plane_client::ControlPlaneClient::new(channel);
    // 注意:必须带原 token(吊销只轮换证书,不轮换 agent_token)。register 的
    // CRL 检查在 token 校验之后,无效 token 会提前被拒,测不到 CRL 路径。
    let err = client
        .register(emorelay_common::control::v1::RegisterRequest {
            node_id,
            agent_token: token,
            version: "e2e-revoked".into(),
        })
        .await
        .expect_err("吊销后的证书必须被拒");
    assert_eq!(err.code(), tonic::Code::PermissionDenied);
}
```

（`RegisterRequest` 字段名以 `agent_e2e.rs` 既有用法为准对齐。）

- [ ] **Step 2: 跑测试验证**

Run: `cargo test -p panel-server --test tunnel_e2e mtls_`
Expected: PASS。若 tonic `Channel` TLS 连接失败，用 context7 校准 tonic 0.12 `ClientTlsConfig` API；若 `https://localhost` SAN 不匹配，确认 `bootstrap_ca` 的 server SAN 列表（127.0.0.1 + localhost）。

- [ ] **Step 3: 全量回归 + Commit**

Run: `cargo test --workspace`

```bash
git add crates/panel-server/tests/tunnel_e2e.rs
git commit -m "test(e2e): real mTLS agent registration and CRL rejection after revocation"
```

---

## Task 12: 文档收尾

**Files:**
- Modify: `plan.md`（附录·实施状态加 Phase 3c 记录）
- Modify: `CLAUDE.md`（仓库现状：P3c 交付；「待推进」改为后续事项）
- Modify: `docs/api.md`（tunnels 响应 `rules_count`/`rules` 字段、rules `tunnel_id` 请求/响应字段补记）
- Modify: `README.md`（功能列表补隧道管理前端 + e2e）

- [ ] **Step 1: plan.md 附录加「Phase 3c（2026-06-11）」**

记录条目：隧道前端两页 + Rules 关联下拉、TunnelView/Detail rules 字段、Agent client SAN 校验（含 entry 禁 bind）、命令重试队列、node-agent lib 化（run_agent）、`tests/tunnel_e2e.rs` 五场景（双/三跳 TCP、双/三跳 TLS、UDP、mTLS+吊销）。注明范围外决策：隧道侧 CRL 留待「短有效期+轮换」方案。

- [ ] **Step 2: CLAUDE.md 仓库现状段更新**

「当前阶段」改为 P3c 已交付；已交付清单加 P3c 行；「待推进」更新（如：真机部署验证 / WSS e2e / 隧道凭据轮换）。

- [ ] **Step 3: docs/api.md 增量**

- `GET /api/tunnels`、`GET /api/tunnels/:id`：补 `rules_count`；detail 补 `rules[]`（id/name/protocol/listen_port/enabled）。
- `POST /api/rules` 请求与 RuleView 的 `tunnel_id` 若未记则补。

- [ ] **Step 4: 验证 + Commit**

Run: `cargo test --workspace && cd web && npx vitest run && npm run build`

```bash
git add plan.md CLAUDE.md docs/api.md README.md
git commit -m "docs: record P3c delivery (tunnel frontend, hardening, e2e matrix)"
```

---

## 执行注意（给实施会话）

1. 严格按 Task 1→12 顺序。T5 依赖 T4（rules_count 字段）；T7 依赖 T5（api.ts tunnels）；T8 依赖 T2（run_agent）；T9/10/11 依赖 T8（helpers）。
2. 每个 Task 收尾跑其 Run 命令 + 全量回归，全绿才 commit；commit 后 spawn `general-purpose` 子代理走 `superpowers:code-reviewer`（只读、三段式回报），阻塞性问题修完才进下一 Task。
3. **x509-parser / tonic ClientTlsConfig API 不确定性**：动手前 context7 校准;实现体按真实 API 调整，不改测试断言语义。
4. e2e 偶发失败优先怀疑时序（agent apply 异步、心跳间隔），加大轮询次数而非 sleep 常数；不要为了过测试改产品代码的超时语义。
5. Windows 本机跑 e2e：全部 bind 127.0.0.1/loopback；mid/exit inter_port 监听 0.0.0.0（task.rs 现状），若防火墙弹窗影响 CI 再议，不在本计划改 bind 地址。
6. 不顺手改无关代码；tests/common/mod.rs 等共享文件若需同步按最小改动。
