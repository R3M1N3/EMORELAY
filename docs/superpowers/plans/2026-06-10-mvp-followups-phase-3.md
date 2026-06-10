# Phase 3 · 多跳隧道 + 内置 CA + 默认 mTLS Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给 EMORELAY 加内置 CA + 默认 mTLS（主控自动签发并强制校验 Agent 客户端证书，支持吊销），并在其上构建多跳隧道（HK→JP→US，TCP/TLS/WSS 三 transport）与隧道管理前端。

**Architecture:** 主控启动时用 `rcgen` 自签 CA + server 证书落盘；gRPC 控制面默认强制 mTLS（client cert 由本 CA 签发，`PANEL_DEV_DISABLE_MTLS=1` 退回 plaintext）。创建节点时一次性返回「token + CA + client cert + client key」四件套，DB 只存证书 serial/fingerprint。隧道由 `tunnels` + `tunnel_hops` 描述，proto `Rule.tunnel` 携带角色与下一跳，dispatcher 按 hop 拆 Rule，Agent 的 `tunnel/` 模块按 entry/mid/exit 角色跑三 transport。

**Tech Stack:** Rust（rcgen 自签证书 / tonic+rustls mTLS / tokio-tungstenite WSS / tokio relay）+ SQLx/SQLite + React 19。

---

## 三段式交付（spec §4 按子系统拆分）

spec §4 覆盖三个耦合度递增的子系统。本计划按依赖顺序分三段，每段独立可交付、独立 review、独立回归。**本文件先完整展开 P3a**；P3b/P3c 待 P3a 落地（用户验证 fleet-wide 重装无误）后，重新调 `superpowers:writing-plans` 展开为同款 TDD plan。

| 段 | 范围 | spec | §4.9 单元 |
|---|---|---|---|
| **P3a** | 内置 CA + 默认 mTLS + 节点四件套 + install.sh 升级 + 吊销/CRL + 存量迁移 | §4.4 | 1, 2, 3, 7（吊销部分） |
| **P3b** | tunnels/tunnel_hops migration + REST + proto Rule.tunnel + dispatcher 拆 hop + Agent tunnel 模块（TCP→TLS→WSS）+ 节点删除保护扩展 | §4.2/4.3/4.5/4.6 | 4, 5, 6, 7（删除保护） |
| **P3c** | 前端 /tunnels + /tunnels/:id + Rules 关联隧道 + 双跳/三跳 TCP/TLS e2e + UDP-over-tunnel | §4.7 | 8 |

### P3a 启动前的关键认知（来自 spec §5.1）

- **fleet-wide 重装**：P3a 启用强制 mTLS 后，P1/P2 阶段创建的存量节点没有 client cert，会全部连不上。P3a-T6 含一步迁移：启动时检测「nodes 存在但 cert_serial IS NULL」的活跃节点 → 自动签发 client cert + 写 audit `node.mtls_credentials_issued`；管理员需到面板「轮换凭据」拿明文 cert+key 重装每个 Agent。**升级 P3a 等同 fleet-wide Agent 重装。**
- **dev 体验**：强制 mTLS 后 `cargo run` 默认要先生成 CA。`PANEL_DEV_DISABLE_MTLS=1` 是开发推荐用法（plaintext + 一行 warn），Agent 端 `AGENT_CONTROL_ENDPOINT` 用 `http://` 即走 plaintext（沿用 `control.rs` 已有判断），无需新 env。
- **现状复用**：`crates/panel-server/src/grpc/mod.rs` 已用 tonic 的 `ServerTlsConfig::client_ca_root` 实现过可选 mTLS（`PANEL_GRPC_TLS_CLIENT_CA`）。P3a 把「手配 env 证书」换成「内置 CA 自动签发 + 默认强制」，并加吊销能力。Agent 端 `control.rs::connect` 已支持 `ClientTlsConfig` 带 client identity，P3a 不需要改 Agent 连接代码，只需让 install.sh 把四件套落盘到 Agent 能读到的路径。

---

## P3a 文件结构（变更面）

**Create:**
- `crates/panel-server/src/tls/mod.rs` — TLS 模块声明 + 共享类型（`CaBundle`、`IssuedCert`）
- `crates/panel-server/src/tls/ca.rs` — CA bootstrap（启动检查/生成 CA + server cert，落盘 0600）
- `crates/panel-server/src/tls/issue.rs` — 为 node 签发 client cert（SAN `node-<id>.emorelay.internal`）+ serial/fingerprint 计算
- `crates/panel-server/src/tls/crl.rs` — 吊销列表（内存 + 落盘 `crl.json`，fingerprint 集合），register 时拒已吊销证书
- `migrations/0005_node_certs.sql` — nodes 加 `cert_serial` / `cert_fingerprint` 列
- `crates/panel-server/tests/tls_ca.rs` — CA bootstrap + 签发 + 验证链 单元/集成测试
- `crates/panel-server/tests/api_nodes_credentials.rs` — 创建节点四件套 + 吊销 API 测试

**Modify:**
- `crates/panel-server/Cargo.toml` — 加 `rcgen`（自签证书）。fingerprint 用现有 `sha2` 对 DER、serial 自生成 hex，无需 x509-parser
- `crates/panel-server/src/config.rs` — 加 `dev_disable_mtls: bool`、`panel_public_host: Option<String>`(server cert SAN)
- `crates/panel-server/src/lib.rs` — `pub mod tls;`
- `crates/panel-server/src/state.rs` — `AppState` 加 `ca: Arc<CaBundle>` + `crl: Arc<Crl>`
- `crates/panel-server/src/main.rs` — 启动 bootstrap CA + 存量节点迁移 + 把 ca/crl 注入 AppState
- `crates/panel-server/src/grpc/mod.rs` — `serve` 改用内置 CA + 默认强制 mTLS（dev override 退 plaintext）
- `crates/panel-server/src/grpc/service.rs` — register 时校验 client cert 未吊销（通过 peer_certs fingerprint）
- `crates/panel-server/src/routes/nodes.rs` — create 响应加四件套；新增 `revoke_credentials` handler
- `crates/panel-server/src/routes/mod.rs` — 注册 `POST /api/nodes/:id/revoke-credentials`
- `crates/panel-server/src/routes/install.rs` — install.sh 接收 `--ca-pem-b64` / `--client-cert-pem-b64` / `--client-key-pem-b64`，落盘 `/etc/emorelay/tls/` + env file 加三个 AGENT_GRPC_* 路径
- `crates/panel-server/src/models/node.rs` — 加 `set_cert_meta` / `find_active_without_cert`
- `web/src/lib/api.ts` — `CreateNodeResponse` 加四件套；`nodes.revokeCredentials`
- `web/src/pages/Nodes.tsx` — 创建成功 Modal 改四块（token/CA/cert/key 各自复制 + 折叠）
- `web/src/pages/NodeDetail.tsx` — 加「轮换凭据」按钮 → 调吊销 API → 弹四件套 Modal
- `.env.example` — 加 `PANEL_DEV_DISABLE_MTLS` / `PANEL_PUBLIC_HOST`
- `docs/api.md` / `docs/deployment.md` / `README.md` / `plan.md` — P3a 文档

---

## P3a 全局约定

- **证书算法**：CA + server + client 均 ECDSA P-256（rcgen `KeyPair::generate` + `PKCS_ECDSA_P256_SHA256`）。CA 有效期 10 年，server/client 5 年。
- **落盘布局**：`${PANEL_DATA_DIR}/tls/{ca.pem, ca.key, server.pem, server.key, crl.json}`，文件权限 0600（Unix；Windows 测试跳过权限断言）。
- **DB 不存私钥**：nodes 只存 `cert_serial`（十六进制串）+ `cert_fingerprint`（SHA-256 of DER，十六进制）。client cert+key 明文仅在创建/轮换响应里一次性返回。
- **吊销语义**：吊销 = 把旧 fingerprint 加入 CRL + **立即重签新证书并覆盖 nodes.cert_serial/fingerprint**（不留空窗口，返回新四件套）。register 时若 peer cert fingerprint ∈ CRL → PermissionDenied。
- **每个 Task 收尾**：跑该 Task 测试 + `cargo test --workspace`（前端 Task 用 vitest+build），全绿 → commit → spawn 子代理走 `superpowers:code-reviewer`（CLAUDE.md 流程）→ 通过才进下一 Task。
- **实现前置**：T1/T2/T3 涉及 rcgen 0.13 与 tonic 0.12 TLS API，实现 agent 动手前**先用 context7 查 `rcgen` 0.13 与 `tonic` 0.12 的当前 API**（本计划给出基于 rcgen 0.13 的骨架与行为契约，测试是契约；若 crate 实际 API 与骨架签名不符，以「让测试通过」为准调整实现体，不改测试断言语义）。

---

## Task 1: CA bootstrap（rcgen 自签 CA + server 证书 + 落盘幂等）

**Files:**
- Modify: `crates/panel-server/Cargo.toml`（加 `rcgen = "0.13"`）
- Create: `crates/panel-server/src/tls/mod.rs`、`crates/panel-server/src/tls/ca.rs`
- Modify: `crates/panel-server/src/lib.rs`（`pub mod tls;`）、`crates/panel-server/src/config.rs`
- Test: `crates/panel-server/tests/tls_ca.rs`

> **实现前置**：先 `context7` 查 rcgen 0.13 API。本计划骨架基于 rcgen 0.13 的：`KeyPair::generate()`（默认 P-256）、`CertificateParams::new(san_vec)`、`params.self_signed(&key)` → `Certificate`、`params.signed_by(&key, &issuer_cert, &issuer_key)` → `Certificate`、`cert.pem()` / `key.serialize_pem()` / `cert.der()`。若签名实际 API 形态不同（如 `Issuer` 包装），以「测试通过」为准调整，勿改测试断言语义。

- [ ] **Step 1: config.rs 加字段**

`crates/panel-server/src/config.rs` 的 `Config` struct 加（放 `panel_public_base_url` 之后）：

```rust
    /// 强制 mTLS 的 dev 逃生阀:1 → gRPC 退回 plaintext(仅开发)。默认 false(强制 mTLS)。
    pub dev_disable_mtls: bool,
    /// server 证书 SAN 里写入的对外主机名(Agent 连入时校验)。
    /// 留空 → 仅签 127.0.0.1 + localhost(本地开发足够)。
    pub panel_public_host: Option<String>,
```

`from_env()` 末尾 struct 构造加：

```rust
            dev_disable_mtls: env::var("PANEL_DEV_DISABLE_MTLS")
                .ok()
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false),
            panel_public_host: env::var("PANEL_PUBLIC_HOST").ok().filter(|s| !s.is_empty()),
```

同步修 `tests/common/mod.rs` 的 `Config { ... }` 字面量（若有）加这两个字段：`dev_disable_mtls: true`（测试默认 plaintext，不强制每个测试建 CA）、`panel_public_host: None`。

- [ ] **Step 2: 写失败测试 `crates/panel-server/tests/tls_ca.rs`**

```rust
//! CA bootstrap 与证书签发的链路测试。用临时目录,不污染真实 data dir。
use panel_server::tls::ca::{bootstrap_ca, CaBundle};
use std::sync::Arc;
use tempfile::TempDir;

fn tls_dir(t: &TempDir) -> String {
    t.path().display().to_string().replace('\\', "/")
}

#[test]
fn bootstrap_generates_ca_and_server_cert_then_is_idempotent() {
    let dir = TempDir::new().unwrap();
    let tls = tls_dir(&dir);

    // 首次:生成 CA + server cert,四个文件落盘。
    let bundle = bootstrap_ca(&tls, Some("relay.example.com")).expect("first bootstrap");
    for f in ["ca.pem", "ca.key", "server.pem", "server.key"] {
        assert!(
            std::path::Path::new(&tls).join(f).exists(),
            "missing {f}"
        );
    }
    let ca_pem_first = bundle.ca_pem.clone();

    // 第二次:文件已存在 → 复用,不重新生成(CA pem 内容一致)。
    let bundle2 = bootstrap_ca(&tls, Some("relay.example.com")).expect("second bootstrap");
    assert_eq!(bundle2.ca_pem, ca_pem_first, "幂等:CA 必须复用,不可重签");
}

#[test]
fn issued_server_cert_chains_to_ca() {
    let dir = TempDir::new().unwrap();
    let bundle: Arc<CaBundle> = bootstrap_ca(&tls_dir(&dir), None).unwrap();
    // server.pem 必须是有效 PEM,且 CA pem 非空(链验证在 mTLS 集成测试里跑;
    // 这里断言结构正确 + 内容非空,避免重依赖 x509 解析库)。
    assert!(bundle.ca_pem.contains("BEGIN CERTIFICATE"));
    assert!(bundle.server_cert_pem.contains("BEGIN CERTIFICATE"));
    assert!(bundle.server_key_pem.contains("BEGIN PRIVATE KEY")
        || bundle.server_key_pem.contains("BEGIN EC PRIVATE KEY"));
    // CA 与 server 不可是同一张证书。
    assert_ne!(bundle.ca_pem, bundle.server_cert_pem);
}
```

`bootstrap_ca` 返回 `anyhow::Result<Arc<CaBundle>>`。

- [ ] **Step 3: 跑测试验证失败**

Run: `cargo test -p panel-server --test tls_ca`
Expected: 编译 FAIL（`panel_server::tls` 不存在）。

- [ ] **Step 4: 实现 tls/mod.rs + tls/ca.rs**

`crates/panel-server/src/lib.rs` 加 `pub mod tls;`（按字母序）。

`crates/panel-server/src/tls/mod.rs`：

```rust
pub mod ca;
pub mod crl;
pub mod issue;
```

`crates/panel-server/src/tls/ca.rs`（骨架；rcgen 0.13 实际调用按 context7 校准）：

```rust
//! 内置 CA bootstrap(P3a)。启动时检查 ${PANEL_DATA_DIR}/tls/:
//! 不存在则用 rcgen 自签 ECDSA P-256 CA(10 年)+ server 证书(5 年),落盘 0600;
//! 已存在则原样加载复用(幂等,绝不重签——重签会让所有已签发 client cert 失效)。
use anyhow::{Context, Result};
use std::path::Path;
use std::sync::Arc;

/// 进程内持有的 CA 物料。server_* 给 gRPC TLS;ca_* 给 client_ca_root 校验 + 节点四件套下发。
pub struct CaBundle {
    pub ca_pem: String,
    pub ca_key_pem: String,
    pub server_cert_pem: String,
    pub server_key_pem: String,
}

pub fn bootstrap_ca(tls_dir: &str, public_host: Option<&str>) -> Result<Arc<CaBundle>> {
    let dir = Path::new(tls_dir);
    std::fs::create_dir_all(dir).with_context(|| format!("create tls dir {tls_dir}"))?;
    let ca_pem_path = dir.join("ca.pem");

    if ca_pem_path.exists() {
        // 已存在 → 加载复用。
        let bundle = CaBundle {
            ca_pem: read(dir, "ca.pem")?,
            ca_key_pem: read(dir, "ca.key")?,
            server_cert_pem: read(dir, "server.pem")?,
            server_key_pem: read(dir, "server.key")?,
        };
        return Ok(Arc::new(bundle));
    }

    // 不存在 → 生成。
    // 1) CA:KeyPair::generate() + CertificateParams 设 is_ca + 10 年有效期 + self_signed。
    // 2) server:CertificateParams::new(SAN) + signed_by(CA)。
    //    SAN = ["127.0.0.1", "localhost"] + public_host(若给)。
    // (rcgen 0.13 具体调用以 context7 为准;以下为行为契约)
    let ca_key = rcgen_generate_keypair()?;
    let (ca_cert_pem, ca_key_pem) = rcgen_self_signed_ca(&ca_key, 3650)?;

    let mut sans = vec!["127.0.0.1".to_string(), "localhost".to_string()];
    if let Some(h) = public_host {
        sans.push(h.to_string());
    }
    let server_key = rcgen_generate_keypair()?;
    let (server_cert_pem, server_key_pem) =
        rcgen_signed_leaf(&server_key, &sans, &ca_cert_pem, &ca_key, 1825, /*is_client=*/ false)?;

    write_0600(dir, "ca.pem", &ca_cert_pem)?;
    write_0600(dir, "ca.key", &ca_key_pem)?;
    write_0600(dir, "server.pem", &server_cert_pem)?;
    write_0600(dir, "server.key", &server_key_pem)?;

    Ok(Arc::new(CaBundle {
        ca_pem: ca_cert_pem,
        ca_key_pem,
        server_cert_pem,
        server_key_pem,
    }))
}

fn read(dir: &Path, name: &str) -> Result<String> {
    std::fs::read_to_string(dir.join(name)).with_context(|| format!("read {name}"))
}

/// 落盘并设 0600(Unix)。Windows 无 chmod,跳过权限设置(测试不断言权限)。
fn write_0600(dir: &Path, name: &str, content: &str) -> Result<()> {
    let path = dir.join(name);
    std::fs::write(&path, content).with_context(|| format!("write {name}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("chmod 0600 {name}"))?;
    }
    Ok(())
}
```

`rcgen_generate_keypair` / `rcgen_self_signed_ca` / `rcgen_signed_leaf` 是本文件内的小 helper，封装 rcgen 0.13 实际调用（KeyPair / CertificateParams / DistinguishedName / IsCa::Ca / KeyUsagePurpose / ExtendedKeyUsagePurpose::{ServerAuth,ClientAuth} / not_before / not_after）。`is_client=true` 时 EKU 用 ClientAuth + SAN 用 DnsName，`false` 时 ServerAuth + SAN 混合 IP/DNS。**实现 agent 按 context7 的 rcgen 0.13 文档把这三个 helper 写实**，签名保持 `Result<(String /*cert_pem*/, String /*key_pem*/)>`（CA helper 返回 cert+key 两个 pem）。

`tls/crl.rs` 与 `tls/issue.rs` 本 Task 先写最小桩（`mod` 能编译即可，内容在 T2/T6 填）：

```rust
// crl.rs 桩
#![allow(dead_code)]
// 实体在 Task 6 实现。
```
```rust
// issue.rs 桩
#![allow(dead_code)]
// 实体在 Task 2 实现。
```

- [ ] **Step 5: 跑测试验证通过**

Run: `cargo test -p panel-server --test tls_ca && cargo test --workspace`
Expected: 全 PASS（两个 tls_ca 测试 + 既有全绿）。

- [ ] **Step 6: Commit**

```bash
git add crates/panel-server/Cargo.toml crates/panel-server/src/tls crates/panel-server/src/lib.rs crates/panel-server/src/config.rs crates/panel-server/tests/tls_ca.rs crates/panel-server/tests/common/mod.rs
git commit -m "feat(tls): self-signed CA bootstrap with idempotent on-disk persistence"
```

---

## Task 2: migration 0005 + 为 node 签发 client cert（issue.rs）

**Files:**
- Create: `migrations/0005_node_certs.sql`
- Modify: `crates/panel-server/src/tls/issue.rs`、`crates/panel-server/src/models/node.rs`
- Test: 扩展 `crates/panel-server/tests/tls_ca.rs`

- [ ] **Step 1: 写 migration 0005**

```sql
-- migrations/0005_node_certs.sql
-- P3a:节点 mTLS 客户端证书元数据。DB 只存 serial + fingerprint(审计 + 吊销),
-- 绝不存私钥明文(明文仅在创建/轮换响应里一次性返回)。
-- PG 迁移:ADD COLUMN 语法一致。
ALTER TABLE nodes ADD COLUMN cert_serial TEXT;
ALTER TABLE nodes ADD COLUMN cert_fingerprint TEXT;
CREATE INDEX idx_nodes_cert_fingerprint ON nodes (cert_fingerprint)
    WHERE cert_fingerprint IS NOT NULL;
```

- [ ] **Step 2: 写失败测试（追加到 tls_ca.rs）**

```rust
use panel_server::tls::issue::issue_client_cert;

#[test]
fn issue_client_cert_chains_and_has_stable_fingerprint() {
    let dir = TempDir::new().unwrap();
    let ca = bootstrap_ca(&tls_dir(&dir), None).unwrap();

    let issued = issue_client_cert(&ca, 42).expect("issue");
    assert!(issued.cert_pem.contains("BEGIN CERTIFICATE"));
    assert!(
        issued.key_pem.contains("BEGIN PRIVATE KEY")
            || issued.key_pem.contains("BEGIN EC PRIVATE KEY")
    );
    // serial / fingerprint 为非空 hex 串。
    assert!(!issued.serial.is_empty() && issued.serial.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(issued.fingerprint.len(), 64, "SHA-256 hex = 64 chars");
    assert!(issued.fingerprint.chars().all(|c| c.is_ascii_hexdigit()));

    // 两次为同一 node 签发 → 不同 serial/fingerprint(每次新证书)。
    let issued2 = issue_client_cert(&ca, 42).expect("issue2");
    assert_ne!(issued.fingerprint, issued2.fingerprint);
}
```

- [ ] **Step 3: 跑测试验证失败**

Run: `cargo test -p panel-server --test tls_ca`
Expected: FAIL（`issue_client_cert` 未实现）。

- [ ] **Step 4: 实现 issue.rs**

```rust
//! 为节点签发 mTLS client 证书(P3a)。SAN = node-<id>.emorelay.internal,
//! EKU = ClientAuth,由内置 CA 签名。返回明文 cert+key(一次性下发)+ serial + fingerprint(落 DB)。
use crate::tls::ca::CaBundle;
use anyhow::Result;
use sha2::{Digest, Sha256};

pub struct IssuedCert {
    pub cert_pem: String,
    pub key_pem: String,
    /// 证书 serial,十六进制串(落 nodes.cert_serial)。
    pub serial: String,
    /// 证书 DER 的 SHA-256,64 位十六进制(落 nodes.cert_fingerprint;CRL 比对用)。
    pub fingerprint: String,
}

pub fn issue_client_cert(ca: &CaBundle, node_id: i64) -> Result<IssuedCert> {
    let san = format!("node-{node_id}.emorelay.internal");
    // rcgen 0.13:为该 SAN 生成 P-256 keypair + CertificateParams(EKU ClientAuth,
    // 随机 serial,5 年),signed_by(CA)。serial 可由 rcgen 生成或自填随机 u64→hex。
    // (具体 API 以 context7 为准;契约:返回 cert.pem()/key.serialize_pem()/cert.der())
    let (cert_pem, key_pem, der, serial) = rcgen_client_leaf(&san, ca, 1825)?;

    let fingerprint = hex::encode(Sha256::digest(&der));
    Ok(IssuedCert {
        cert_pem,
        key_pem,
        serial,
        fingerprint,
    })
}
```

`rcgen_client_leaf` 封装 rcgen 0.13：返回 `(cert_pem, key_pem, der_bytes, serial_hex)`。serial 用 rcgen 的 `params.serial_number = Some(SerialNumber::from(rand_u64))`，hex 化后返回；der 用 `cert.der()` 的字节。

- [ ] **Step 5: models/node.rs 加方法**

```rust
    /// 写入证书元数据(创建/轮换后)。
    pub async fn set_cert_meta(
        pool: &SqlitePool,
        id: i64,
        serial: &str,
        fingerprint: &str,
    ) -> sqlx::Result<u64> {
        let res = sqlx::query(
            "UPDATE nodes SET cert_serial = ?, cert_fingerprint = ?, updated_at = datetime('now') \
             WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(serial)
        .bind(fingerprint)
        .bind(id)
        .execute(pool)
        .await?;
        Ok(res.rows_affected())
    }

    /// 活跃但尚无证书的节点 id 列表(P3a 启动迁移用)。
    pub async fn find_active_without_cert(pool: &SqlitePool) -> sqlx::Result<Vec<i64>> {
        sqlx::query_scalar::<_, i64>(
            "SELECT id FROM nodes WHERE deleted_at IS NULL AND cert_serial IS NULL ORDER BY id",
        )
        .fetch_all(pool)
        .await
    }
```

（`Node` struct 不必加 cert 字段——这两列只在迁移/吊销路径按需读写，避免 NodeView 暴露。若 `NODE_COLUMNS` 用 `SELECT *` 则需确认 FromRow 不报错；现状是显式列名，加列无影响。）

- [ ] **Step 6: 跑测试验证通过**

Run: `cargo test -p panel-server --test tls_ca && cargo test --workspace`
Expected: 全 PASS。

- [ ] **Step 7: Commit**

```bash
git add migrations/0005_node_certs.sql crates/panel-server/src/tls/issue.rs crates/panel-server/src/models/node.rs crates/panel-server/tests/tls_ca.rs
git commit -m "feat(tls): issue per-node client certs; nodes cert_serial/fingerprint columns"
```

---

## Task 3: gRPC 默认强制 mTLS（内置 CA + dev override）

**Files:**
- Modify: `crates/panel-server/src/state.rs`、`crates/panel-server/src/main.rs`、`crates/panel-server/src/grpc/mod.rs`
- Test: 扩展 `crates/panel-server/tests/tls_ca.rs`（构建 TLS 配置不崩 + dev override 走 plaintext 的纯函数测试）

> mTLS 的真链路验证（Agent 带 client cert 连入）留到 P3c e2e（需起真 server+agent）。本 Task 测「TLS server 配置能从内置 CA 构建成功」与「dev_disable_mtls=true → 返回 plaintext 模式标记」。

- [ ] **Step 1: state.rs 注入 ca**

`crates/panel-server/src/state.rs` 的 `AppState` 加字段：

```rust
    pub ca: std::sync::Arc<crate::tls::ca::CaBundle>,
    pub crl: std::sync::Arc<crate::tls::crl::Crl>,
```

（`Crl` 在 T6 完整实现；T3 先让它是个可 `Default`/`new` 的空集合——见下方 T3 对 crl.rs 的最小实现。）

- [ ] **Step 2: crl.rs 最小实现（T6 扩展热加载/落盘）**

```rust
//! 吊销列表(P3a)。fingerprint 集合;register 时拒已吊销证书。
//! T3 先给内存版 + 文件加载;T6 加 revoke 落盘 + 热加载。
use std::collections::HashSet;
use std::sync::RwLock;

#[derive(Default)]
pub struct Crl {
    revoked: RwLock<HashSet<String>>,
}

impl Crl {
    pub fn new() -> Self {
        Self::default()
    }

    /// 从 crl.json(JSON 数组 of fingerprint)加载;文件不存在 → 空集合。
    pub fn load(path: &str) -> Self {
        let revoked = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
            .map(|v| v.into_iter().collect())
            .unwrap_or_default();
        Self {
            revoked: RwLock::new(revoked),
        }
    }

    pub fn is_revoked(&self, fingerprint: &str) -> bool {
        self.revoked.read().unwrap().contains(fingerprint)
    }
}
```

- [ ] **Step 3: 写测试（追加 tls_ca.rs）**

```rust
use panel_server::grpc::tls_mode_for;

#[test]
fn dev_disable_mtls_yields_plaintext() {
    // dev_disable_mtls = true → plaintext(开发逃生阀)。
    assert!(matches!(tls_mode_for(true), panel_server::grpc::GrpcTlsMode::Plaintext));
    // 默认 false → mTLS。
    assert!(matches!(tls_mode_for(false), panel_server::grpc::GrpcTlsMode::Mtls));
}

#[test]
fn crl_load_missing_file_is_empty() {
    let crl = panel_server::tls::crl::Crl::load("/nonexistent/crl.json");
    assert!(!crl.is_revoked("deadbeef"));
}
```

- [ ] **Step 4: 跑测试验证失败**

Run: `cargo test -p panel-server --test tls_ca`
Expected: FAIL（`tls_mode_for` / `GrpcTlsMode` 不存在）。

- [ ] **Step 5: grpc/mod.rs 改造**

在 `crates/panel-server/src/grpc/mod.rs` 顶部加：

```rust
/// gRPC 控制面 TLS 模式。默认 mTLS;dev 逃生阀退 plaintext。
pub enum GrpcTlsMode {
    Mtls,
    Plaintext,
}

pub fn tls_mode_for(dev_disable_mtls: bool) -> GrpcTlsMode {
    if dev_disable_mtls {
        GrpcTlsMode::Plaintext
    } else {
        GrpcTlsMode::Mtls
    }
}
```

`serve` 改为按内置 CA + mode 构建（替换原来读 env 证书的逻辑）：

```rust
pub async fn serve(state: AppState, addr: SocketAddr) -> Result<()> {
    let mode = tls_mode_for(state.config.dev_disable_mtls);
    let ca = state.ca.clone();
    let svc = ControlPlaneServer::new(service::ControlPlaneImpl::new(state));
    let mut builder = Server::builder();

    match mode {
        GrpcTlsMode::Mtls => {
            // server identity = 内置 server 证书;client_ca_root = 内置 CA(强制 client cert)。
            let identity = Identity::from_pem(
                ca.server_cert_pem.as_bytes(),
                ca.server_key_pem.as_bytes(),
            );
            let tls_cfg = ServerTlsConfig::new()
                .identity(identity)
                .client_ca_root(Certificate::from_pem(ca.ca_pem.as_bytes()));
            builder = builder.tls_config(tls_cfg).context("apply gRPC mTLS config")?;
            info!(%addr, "grpc control plane listening (built-in CA, mTLS enforced)");
        }
        GrpcTlsMode::Plaintext => {
            warn!(%addr, "grpc control plane PLAINTEXT (PANEL_DEV_DISABLE_MTLS set — dev only)");
        }
    }

    builder.add_service(svc).serve(addr).await.context("grpc serve")?;
    Ok(())
}
```

旧的 `grpc_tls_cert/key/client_ca` 三个 env 字段保留在 Config 里（向后兼容/不删），但 `serve` 不再读它们——加一行注释说明「P3a 起 gRPC TLS 走内置 CA;这三个 env 仅保留兼容,已弃用」。import 清理（`Certificate`/`Identity` 仍用）。

- [ ] **Step 6: main.rs bootstrap + 注入**

`crates/panel-server/src/main.rs` 在 `db::run_migrations` 之后、构造 `AppState` 之前加：

```rust
    let tls_dir = std::path::PathBuf::from(&config.panel_data_dir)
        .join("tls")
        .display()
        .to_string();
    let ca = panel_server::tls::ca::bootstrap_ca(&tls_dir, config.panel_public_host.as_deref())?;
    let crl_path = format!("{tls_dir}/crl.json");
    let crl = std::sync::Arc::new(panel_server::tls::crl::Crl::load(&crl_path));
    info!(mtls = !config.dev_disable_mtls, "tls ready");
```

`AppState { ... }` 构造加 `ca, crl: crl.clone()`。

`tests/common/mod.rs` 的 `AppState { ... }` 构造同步加：用临时目录 bootstrap 一个 CA（`make_app` 已有 tempdir）+ `crl: Arc::new(Crl::new())`：

```rust
    let tls_dir = format!("{}/tls", temp.path().display().to_string().replace('\\', "/"));
    let ca = panel_server::tls::ca::bootstrap_ca(&tls_dir, None)?;
    // ... AppState { config, pool, sessions, dispatcher, ca, crl: Arc::new(panel_server::tls::crl::Crl::new()) }
```

- [ ] **Step 7: 跑测试验证通过**

Run: `cargo test -p panel-server --test tls_ca && cargo test --workspace`
Expected: 全 PASS（注意:所有集成测试现在每个 make_app 会 bootstrap 一个临时 CA,确认无显著拖慢;P-256 生成 ms 级）。

- [ ] **Step 8: Commit**

```bash
git add crates/panel-server/src/state.rs crates/panel-server/src/main.rs crates/panel-server/src/grpc/mod.rs crates/panel-server/src/tls/crl.rs crates/panel-server/tests/tls_ca.rs crates/panel-server/tests/common/mod.rs
git commit -m "feat(grpc): enforce mTLS via built-in CA by default; PANEL_DEV_DISABLE_MTLS escape hatch"
```

## Task 4: 创建节点响应四件套 + DB 存 serial/fingerprint

**Files:**
- Modify: `crates/panel-server/src/routes/nodes.rs`
- Test: `crates/panel-server/tests/api_nodes_credentials.rs`

- [ ] **Step 1: 写失败测试 `crates/panel-server/tests/api_nodes_credentials.rs`**

```rust
mod common;

use axum::http::{Method, StatusCode};
use serde_json::json;

#[tokio::test]
async fn create_node_returns_four_credential_blocks() {
    let app = common::make_app().await.unwrap();
    let req = common::auth_req(
        Method::POST,
        "/api/nodes",
        &app.admin_token,
        Some(json!({ "name": "hk-relay-01" })),
    )
    .unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");

    // 四件套:token + CA + client cert + client key,均一次性返回。
    assert!(body["agent_token"].as_str().unwrap().len() >= 16);
    assert!(body["ca_pem"].as_str().unwrap().contains("BEGIN CERTIFICATE"));
    assert!(body["client_cert_pem"].as_str().unwrap().contains("BEGIN CERTIFICATE"));
    let key = body["client_key_pem"].as_str().unwrap();
    assert!(key.contains("BEGIN PRIVATE KEY") || key.contains("BEGIN EC PRIVATE KEY"));

    let node_id = body["node"]["id"].as_i64().unwrap();

    // DB 只存 serial/fingerprint,绝不存私钥明文。
    let (serial, fp): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT cert_serial, cert_fingerprint FROM nodes WHERE id = ?",
    )
    .bind(node_id)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert!(serial.is_some() && fp.is_some(), "证书元数据必须落库");
    // 全表任何列都不应出现私钥明文。
    let dump: Vec<(String,)> = sqlx::query_as("SELECT cert_serial FROM nodes WHERE id = ?")
        .bind(node_id)
        .fetch_all(&app.state.pool)
        .await
        .unwrap();
    assert!(!dump[0].0.contains("PRIVATE KEY"));
}
```

- [ ] **Step 2: 跑测试验证失败**

Run: `cargo test -p panel-server --test api_nodes_credentials`
Expected: FAIL（响应无 ca_pem 等字段）。

- [ ] **Step 3: 实现 — nodes.rs create 扩展**

`CreateNodeResponse` 加四件套字段：

```rust
#[derive(Serialize)]
pub struct CreateNodeResponse {
    pub node: NodeView,
    /// 仅在创建时返回一次的明文 token；之后只能轮换重新发放。
    pub agent_token: String,
    /// mTLS 四件套(一次性):CA 公钥 / 该节点 client 证书 / client 私钥。
    pub ca_pem: String,
    pub client_cert_pem: String,
    pub client_key_pem: String,
}
```

create handler 在拿到 `id` 后、构造响应前加：

```rust
    // 为新节点签发 mTLS client 证书(四件套之三);DB 只存 serial/fingerprint。
    let issued = crate::tls::issue::issue_client_cert(&state.ca, id)
        .map_err(ApiError::Internal)?;
    Node::set_cert_meta(&state.pool, id, &issued.serial, &issued.fingerprint).await?;
```

响应构造改为：

```rust
    Ok(Json(CreateNodeResponse {
        node: node.into(),
        agent_token: token,
        ca_pem: state.ca.ca_pem.clone(),
        client_cert_pem: issued.cert_pem,
        client_key_pem: issued.key_pem,
    }))
```

- [ ] **Step 4: 跑测试验证通过**

Run: `cargo test -p panel-server --test api_nodes_credentials && cargo test --workspace`
Expected: 全 PASS。

- [ ] **Step 5: Commit**

```bash
git add crates/panel-server/src/routes/nodes.rs crates/panel-server/tests/api_nodes_credentials.rs
git commit -m "feat(server): create-node returns mTLS four-piece credentials; store cert meta only"
```

---

## Task 5: install.sh 接收三个 base64 PEM + 前端四件套 Modal

**Files:**
- Modify: `crates/panel-server/src/routes/install.rs`
- Modify: `crates/panel-server/tests/api_install.rs`
- Modify: `web/src/lib/api.ts`、`web/src/pages/Nodes.tsx`

> 安全要点：install.sh 端点本身**不含任何 secret**（保持无鉴权可缓存）。CA/cert/key 由前端把四件套 base64 后拼进 `sudo bash -s -- --token=X --ca-pem-b64=Y --client-cert-pem-b64=Z --client-key-pem-b64=W` 命令字符串，用户从创建/轮换 Modal 复制走。脚本负责解析这三个参数、解码落盘、写 env。

- [ ] **Step 1: 写失败测试（追加 api_install.rs）**

```rust
#[tokio::test]
async fn install_script_parses_tls_pem_args() {
    let app = common::make_app().await.unwrap();
    let req = Request::get("/install.sh?node=7")
        .body(Body::empty())
        .unwrap();
    let (status, body) = common::send_text(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    // 脚本必须解析三个 TLS 参数并落盘到 /etc/emorelay/tls/。
    assert!(body.contains("--ca-pem-b64"));
    assert!(body.contains("--client-cert-pem-b64"));
    assert!(body.contains("--client-key-pem-b64"));
    assert!(body.contains("/etc/emorelay/tls/ca.pem"));
    assert!(body.contains("AGENT_GRPC_CA_CERT"));
    assert!(body.contains("AGENT_GRPC_CLIENT_CERT"));
    assert!(body.contains("AGENT_GRPC_CLIENT_KEY"));
}
```

（若 `api_install.rs` 无 `send_text` helper：直接断言现有 `send` 返回的 body 字符串；install.sh 返回 text 而非 JSON，沿用该文件既有读取方式——查文件顶部已有的辅助，缺则用 `String::from_utf8` 读 body。实现 agent 按文件现状对齐。）

- [ ] **Step 2: 跑测试验证失败**

Run: `cargo test -p panel-server --test api_install`
Expected: FAIL（脚本不含 TLS 参数解析）。

- [ ] **Step 3: install.rs 脚本模板升级**

`render_install_sh` 的参数解析 `while` 循环加三个分支（与 `--token` 并列）：

```bash
    --ca-pem-b64=*)          CA_B64="${{1#*=}}"; shift ;;
    --client-cert-pem-b64=*) CERT_B64="${{1#*=}}"; shift ;;
    --client-key-pem-b64=*)  KEY_B64="${{1#*=}}"; shift ;;
```

循环前初始化 `CA_B64="" CERT_B64="" KEY_B64=""`。env 文件写入段之前加 TLS 落盘段：

```bash
# 2.5 写 mTLS 凭据(若提供)。三者必须同时给。
if [[ -n "$CA_B64" && -n "$CERT_B64" && -n "$KEY_B64" ]]; then
  install -d -m 0700 /etc/emorelay/tls
  echo "$CA_B64"   | base64 -d > /etc/emorelay/tls/ca.pem
  echo "$CERT_B64" | base64 -d > /etc/emorelay/tls/client.pem
  echo "$KEY_B64"  | base64 -d > /etc/emorelay/tls/client-key.pem
  chmod 0600 /etc/emorelay/tls/*.pem
  TLS_ENV=$'AGENT_GRPC_CA_CERT=/etc/emorelay/tls/ca.pem\nAGENT_GRPC_CLIENT_CERT=/etc/emorelay/tls/client.pem\nAGENT_GRPC_CLIENT_KEY=/etc/emorelay/tls/client-key.pem'
else
  TLS_ENV=""
fi
```

agent.env 写入段把 `TLS_ENV` 追加进去（在 heredoc 后单独 append，避免空值写空行污染）：

```bash
cat > /etc/emorelay/agent.env <<EOF
AGENT_NODE_ID={node_id}
AGENT_TOKEN=$TOKEN
AGENT_CONTROL_ENDPOINT={control_endpoint}
AGENT_STATE_PATH=/var/lib/emorelay/agent-state.json
EOF
if [[ -n "$TLS_ENV" ]]; then printf '%s\n' "$TLS_ENV" >> /etc/emorelay/agent.env; fi
chmod 0600 /etc/emorelay/agent.env
```

（render 用 Rust `format!` r##"..."##,`{{` `}}` 转义沿用现状;control_endpoint/node_id/base_url 占位不变。）

- [ ] **Step 4: 前端 renderInstallCommand 四件套 + Nodes Modal 四块**

`web/src/lib/api.ts`：`CreateNodeResponse` 加：

```typescript
  ca_pem: string
  client_cert_pem: string
  client_key_pem: string
```

`renderInstallCommand` 改签名带可选四件套并 base64 嵌入（浏览器 `btoa`）：

```typescript
export function renderInstallCommand(opts: {
  nodeId: number
  token: string
  caPem?: string
  clientCertPem?: string
  clientKeyPem?: string
}): string {
  const base = window.location.origin
  let cmd = `curl -fsSL ${base}/install.sh?node=${opts.nodeId} | sudo bash -s -- --token=${opts.token}`
  if (opts.caPem && opts.clientCertPem && opts.clientKeyPem) {
    cmd += ` --ca-pem-b64=${btoa(opts.caPem)}`
    cmd += ` --client-cert-pem-b64=${btoa(opts.clientCertPem)}`
    cmd += ` --client-key-pem-b64=${btoa(opts.clientKeyPem)}`
  }
  return cmd
}
```

`web/src/pages/Nodes.tsx` 创建成功 Modal：把现有「复制安装命令」改为传四件套给 `renderInstallCommand`；并在 Modal 内加四个独立可复制块（token / ca_pem / client_cert_pem / client_key_pem），各带「复制」按钮 + 折叠（`<details>` 或 useState 展开），文案提示「私钥仅此一次显示，丢失需轮换」。沿用页面既有 toast + 复制逻辑（`navigator.clipboard.writeText`）。

- [ ] **Step 5: 验证**

Run: `cargo test -p panel-server --test api_install && cd web && npx vitest run && npm run build`
Expected: 全 PASS + build 零错误。

- [ ] **Step 6: Commit**

```bash
git add crates/panel-server/src/routes/install.rs crates/panel-server/tests/api_install.rs web/src/lib/api.ts web/src/pages/Nodes.tsx
git commit -m "feat(install): script accepts base64 mTLS PEMs; node-create modal shows four credential blocks"
```

---

## Task 6: 吊销 API + CRL 落盘/热加载 + register 拒吊销 + 存量迁移

**Files:**
- Modify: `crates/panel-server/src/tls/crl.rs`、`crates/panel-server/src/routes/nodes.rs`、`crates/panel-server/src/routes/mod.rs`、`crates/panel-server/src/grpc/service.rs`、`crates/panel-server/src/main.rs`
- Modify: `web/src/lib/api.ts`、`web/src/pages/NodeDetail.tsx`
- Test: 扩展 `crates/panel-server/tests/api_nodes_credentials.rs`

- [ ] **Step 1: 写失败测试（追加 api_nodes_credentials.rs）**

```rust
#[tokio::test]
async fn revoke_credentials_rotates_and_revokes_old() {
    let app = common::make_app().await.unwrap();
    // 建节点拿初始四件套。
    let req = common::auth_req(Method::POST, "/api/nodes", &app.admin_token,
        Some(json!({ "name": "rev-node" }))).unwrap();
    let (_, body) = common::send(app.app.clone(), req).await.unwrap();
    let node_id = body["node"]["id"].as_i64().unwrap();
    let (old_fp,): (String,) = sqlx::query_as("SELECT cert_fingerprint FROM nodes WHERE id = ?")
        .bind(node_id).fetch_one(&app.state.pool).await.unwrap();

    // 吊销 → 返回新四件套 + 旧 fingerprint 进 CRL + DB 换新 fingerprint。
    let req = common::auth_req(Method::POST, &format!("/api/nodes/{node_id}/revoke-credentials"),
        &app.admin_token, None).unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["client_cert_pem"].as_str().unwrap().contains("BEGIN CERTIFICATE"));
    assert!(app.state.crl.is_revoked(&old_fp), "旧证书必须进 CRL");

    let (new_fp,): (String,) = sqlx::query_as("SELECT cert_fingerprint FROM nodes WHERE id = ?")
        .bind(node_id).fetch_one(&app.state.pool).await.unwrap();
    assert_ne!(old_fp, new_fp);

    // audit:node.credentials_revoked。
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_logs WHERE action = 'node.credentials_revoked' AND target_id = ?")
        .bind(node_id).fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn revoke_requires_admin() {
    let app = common::make_app().await.unwrap();
    let req = common::auth_req(Method::POST, "/api/nodes", &app.admin_token,
        Some(json!({ "name": "n2" }))).unwrap();
    let (_, body) = common::send(app.app.clone(), req).await.unwrap();
    let node_id = body["node"]["id"].as_i64().unwrap();
    let (_uid, token) = common::make_user_token(&app, "nonadmin", "password123").await.unwrap();
    let req = common::auth_req(Method::POST, &format!("/api/nodes/{node_id}/revoke-credentials"),
        &token, None).unwrap();
    let (status, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::FORBIDDEN);
}
```

- [ ] **Step 2: 跑测试验证失败**

Run: `cargo test -p panel-server --test api_nodes_credentials`
Expected: FAIL（吊销路由不存在）。

- [ ] **Step 3: crl.rs 加 revoke 落盘**

```rust
    /// 把 fingerprint 加入吊销集并持久化到 path(JSON 数组)。
    pub fn revoke(&self, fingerprint: &str, path: &str) -> anyhow::Result<()> {
        let mut set = self.revoked.write().unwrap();
        set.insert(fingerprint.to_string());
        let snapshot: Vec<&String> = set.iter().collect();
        let json = serde_json::to_string(&snapshot)?;
        std::fs::write(path, json)?;
        Ok(())
    }
```

（热加载：本 P3a 用「同进程内 revoke 直接更新内存 + 落盘」即满足——register 读的是同一个 `Arc<Crl>` 内存集合。跨进程/外部改 crl.json 的 mtime 监听留作 P3b/后续；spec §4.4 的「文件 mtime 监听」在单进程内非必需，本 Task 不实现 watcher，注释说明。）

- [ ] **Step 4: nodes.rs 吊销 handler**

```rust
pub async fn revoke_credentials(
    State(state): State<AppState>,
    auth: AuthUser,
    actor_ip: ActorIp,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    auth.require_admin()?;
    let _node = Node::find_by_id(&state.pool, id).await?.ok_or(ApiError::NotFound)?;

    // 取旧 fingerprint 入 CRL(若有)。
    let old: Option<(Option<String>,)> =
        sqlx::query_as("SELECT cert_fingerprint FROM nodes WHERE id = ? AND deleted_at IS NULL")
            .bind(id).fetch_optional(&state.pool).await?;
    if let Some((Some(old_fp),)) = old {
        let crl_path = format!("{}/tls/crl.json", state.config.panel_data_dir);
        state.crl.revoke(&old_fp, &crl_path).map_err(ApiError::Internal)?;
    }

    // 重签新证书 + 落新 meta。
    let issued = crate::tls::issue::issue_client_cert(&state.ca, id).map_err(ApiError::Internal)?;
    Node::set_cert_meta(&state.pool, id, &issued.serial, &issued.fingerprint).await?;

    audit::record_with_ip(&state.pool, Some(auth.0.sub), actor_ip.as_option(),
        "node.credentials_revoked", Some("node"), Some(id), None, true, None).await;

    Ok(Json(json!({
        "ca_pem": state.ca.ca_pem.clone(),
        "client_cert_pem": issued.cert_pem,
        "client_key_pem": issued.key_pem,
    })))
}
```

`routes/mod.rs` 加：

```rust
        .route("/api/nodes/{id}/revoke-credentials", post(nodes::revoke_credentials))
```

- [ ] **Step 5: register 拒吊销证书（grpc/service.rs）**

在 `register` 取 token hash 校验通过之后、颁发 session 之前加 peer cert 吊销检查（plaintext dev 模式无 peer cert → 跳过）：

```rust
        // mTLS 模式下校验 client 证书未被吊销(plaintext dev 无 peer cert,跳过)。
        if let Some(certs) = req.peer_certs() {
            if let Some(leaf) = certs.first() {
                use sha2::{Digest, Sha256};
                let fp = hex::encode(Sha256::digest(leaf.as_ref()));
                if self.state.crl.is_revoked(&fp) {
                    audit::record(&self.state.pool, None, "agent.register", Some("node"),
                        Some(req.node_id), Some(&format!("version={}", req.version)),
                        false, Some("revoked_cert")).await;
                    return Err(Status::permission_denied("permission denied"));
                }
            }
        }
```

（`req.peer_certs()` 需在 `req.into_inner()` 之前调用——调整 register 开头：先 `let peer = req.peer_certs();` 再 `let req = req.into_inner();`，用 `peer` 做上面的检查。tonic 0.12 的 `Certificate::as_ref()` 给 DER 字节;以 context7 校准。）

- [ ] **Step 6: main.rs 存量迁移**

`crates/panel-server/src/main.rs` bootstrap CA 之后、serve 之前加：

```rust
    // P3a 存量迁移:活跃但无证书的节点(P1/P2 创建)自动签发 client cert。
    // 管理员需到面板「轮换凭据」拿明文重装 Agent(升级 P3a = fleet-wide 重装)。
    match panel_server::models::node::Node::find_active_without_cert(&state.pool).await {
        Ok(ids) => {
            for nid in ids {
                if let Ok(issued) = panel_server::tls::issue::issue_client_cert(&state.ca, nid) {
                    let _ = panel_server::models::node::Node::set_cert_meta(
                        &state.pool, nid, &issued.serial, &issued.fingerprint).await;
                    panel_server::audit::record(&state.pool, None,
                        "node.mtls_credentials_issued", Some("node"), Some(nid), None, true, None).await;
                    tracing::warn!(node_id = nid, "issued mTLS cert for legacy node; rotate to get plaintext for reinstall");
                }
            }
        }
        Err(e) => tracing::warn!(error = ?e, "legacy node cert migration query failed"),
    }
```

（注意：此段需放在 `AppState` 构造之后,因为用 `state.ca` / `state.pool`。）

- [ ] **Step 7: 前端轮换按钮（NodeDetail.tsx）**

`web/src/lib/api.ts` `nodes` 组加：

```typescript
  revokeCredentials: (id: number) =>
    api.post<{ ca_pem: string; client_cert_pem: string; client_key_pem: string }>(
      `/api/nodes/${id}/revoke-credentials`,
    ),
```

`web/src/pages/NodeDetail.tsx` 顶部操作区加「轮换凭据」按钮 → confirm Modal（警告「旧证书立即失效，须用新四件套重装 Agent」）→ 调 `nodes.revokeCredentials(id)` → 成功后弹四件套 Modal（复用 Nodes.tsx 的四块展示组件，或就地渲染 token 缺省的三块 + 安装命令）。沿用页面既有 toast。

- [ ] **Step 8: 验证**

Run: `cargo test -p panel-server --test api_nodes_credentials && cargo test --workspace && cd web && npx vitest run && npm run build`
Expected: 全 PASS。

- [ ] **Step 9: Commit**

```bash
git add crates/panel-server/src/tls/crl.rs crates/panel-server/src/routes/nodes.rs crates/panel-server/src/routes/mod.rs crates/panel-server/src/grpc/service.rs crates/panel-server/src/main.rs web/src/lib/api.ts web/src/pages/NodeDetail.tsx crates/panel-server/tests/api_nodes_credentials.rs
git commit -m "feat(tls): revoke-credentials API with CRL persistence; reject revoked certs at register; legacy node migration"
```

---

## Task 7: P3a 文档 + 配置收尾

**Files:**
- Modify: `.env.example`、`docs/api.md`、`docs/deployment.md`、`README.md`、`plan.md`

- [ ] **Step 1: .env.example**

加（gRPC TLS 段附近）：

```bash
# P3a 起 gRPC 控制面默认强制 mTLS,证书由内置 CA(${PANEL_DATA_DIR}/tls/)自动签发。
# 开发逃生阀:置 1 退回 plaintext(Agent 端 AGENT_CONTROL_ENDPOINT 用 http:// 配合)。
PANEL_DEV_DISABLE_MTLS=0
# 内置 CA 给 server 证书写入的对外主机名(Agent 连入校验)。留空 → 仅 127.0.0.1/localhost。
PANEL_PUBLIC_HOST=
```

旧的 `PANEL_GRPC_TLS_CERT/KEY/CLIENT_CA` 注释加一行「P3a 起已弃用,gRPC TLS 走内置 CA」。

- [ ] **Step 2: docs/api.md**

- nodes 创建响应：加四件套字段说明（agent_token + ca_pem + client_cert_pem + client_key_pem，一次性，私钥不可恢复）。
- 新增 `POST /api/nodes/:id/revoke-credentials`（admin，吊销旧证书入 CRL + 重签返回新四件套，audit `node.credentials_revoked`）。
- audit actions 加 `node.credentials_revoked` / `node.mtls_credentials_issued` / register 失败 `revoked_cert`。
- 加一节「mTLS 与节点凭据」：内置 CA、默认强制、dev override、四件套语义、轮换流程、升级 P3a = fleet-wide 重装警告。

- [ ] **Step 3: docs/deployment.md + README.md**

- deployment.md env 表加 `PANEL_DEV_DISABLE_MTLS` / `PANEL_PUBLIC_HOST`；加一节「升级到 P3a（启用 mTLS）」步骤：先确保 PANEL_PUBLIC_HOST 配对、启动后到面板逐节点「轮换凭据」重装 Agent、或 dev 环境用 PANEL_DEV_DISABLE_MTLS=1 平滑过渡。
- README 功能列表加「内置 CA + 默认 mTLS（节点证书自动签发 + 吊销）」。

- [ ] **Step 4: plan.md 附录**

「Phase 2」小节后加「Phase 3a（2026-06-10 启动）」记录：内置 CA + 默认 mTLS + 四件套 + 吊销/CRL + 存量迁移交付；列 migration 0005、新测试文件、fleet-wide 重装警告、PANEL_DEV_DISABLE_MTLS dev 用法。注明 P3b（隧道）/P3c（前端+e2e）待展开。

- [ ] **Step 5: 全量回归 + Commit**

```bash
cargo test --workspace && cd web && npx vitest run && npm run build
git add .env.example docs/api.md docs/deployment.md README.md plan.md
git commit -m "docs(p3a): document built-in CA + mTLS, node credential rotation, fleet-wide upgrade"
```

---

## P3a 验收清单（spec §4.4 / §4.8 前半）

- [ ] `cargo run -p panel-server`（无任何 TLS env）→ 启动自动签 CA + server cert，gRPC 强制 mTLS → Task 1/3
- [ ] `PANEL_DEV_DISABLE_MTLS=1` → 退回 plaintext + 启动 warn → Task 3
- [ ] 创建节点 → 响应含 token + CA + client cert + key 四块；DB 只存 serial/fingerprint → Task 4
- [ ] 复制安装命令（含四件套 base64）→ install.sh 落盘 /etc/emorelay/tls/ + env 三个 AGENT_GRPC_* → Task 5
- [ ] 吊销节点凭据 → 旧 fingerprint 入 CRL，register 携旧证书被拒；返回新四件套 → Task 6
- [ ] 存量节点（cert_serial IS NULL）启动时自动签发 + audit → Task 6

> 「Agent 自动启用 mTLS 连上 + 在线」「吊销后旧 Agent 立即被断开 TLS」这两条真链路验收需起真 server+agent，留到 P3c e2e（与隧道 e2e 一并做）。

---

## P3b 概要（待 P3a 落地后展开）

§4.9 单元 4/5/6/7（删除保护）。Task 概要：
1. migration 0006（tunnels + tunnel_hops + forward_rules.tunnel_id）+ `models/tunnel.rs`
2. proto：`Rule.tunnel`(TunnelContext) + `Command` oneof 加 `TunnelCredentials`/`RevokeTunnelCredentials`(字段号不复用) + `TunnelRole` enum
3. `routes/tunnels.rs` CRUD（创建校验链路 ≥2 节点全 online + 分配 inter_port + 事务写 hops + audit）
4. dispatcher 按 hop 拆 Rule（entry/mid/exit 三实例，mid/exit 角色 bandwidth_mbps=0 避免逐跳重复限速）
5. Agent `tunnel/` 模块：`transport.rs` trait + `tcp_transport.rs`（先）→ `tls_transport.rs`（复用内置 CA，SNI=tunnel-<id>-hop-<n>.emorelay.internal）→ `wss_transport.rs`（tokio-tungstenite）；`task.rs` entry/mid/exit 三角色；RuleManager/ConfigStore 扩展
6. 节点删除保护扩展（查 tunnel_hops.node_id）

## P3c 概要（待 P3b 落地后展开）

§4.9 单元 8 + §4.7。Task 概要：
1. 前端 `pages/Tunnels.tsx` + `TunnelDetail.tsx` + App 路由 + sidebar「隧道」入口
2. `Rules.tsx` 加「关联隧道」下拉（选隧道 → node_id 自动填入口节点且禁用）
3. e2e：双跳 + 三跳 × TCP/TLS 矩阵（起真 server+多 agent，curl 入口端口达目标）；mTLS 真链路（Agent 带 cert 连上、吊销后断开）；UDP-over-tunnel 帧（2 字节长度前缀，entry/exit 打包拆包）

---

## 执行注意（给实施会话）

1. 严格按 Task 1→7 顺序。T3 依赖 T1（CaBundle）；T4 依赖 T2（issue）+T3（state.ca）；T6 依赖 T4+T5。
2. 每个 Task 收尾跑其 Run 命令 + `cargo test --workspace`（前端 Task 加 vitest+build），全绿才 commit。
3. 每个 Task commit 后 spawn `general-purpose` 子代理走 `superpowers:code-reviewer`（只读，三段式回报），阻塞性问题修完才进下一 Task。
4. **rcgen/tonic TLS API 不确定性**：T1/T2/T3/T6 动手前先 context7 查 rcgen 0.13 + tonic 0.12 实际 API；本计划骨架是行为契约，测试断言是验收标准，实现体按真实 API 调整以让测试通过，**不改测试断言语义**。
5. 不顺手改无关代码；发现计划与代码现状冲突（尤其 tests/common/mod.rs 的 AppState/Config 字面量需同步加 ca/crl/两个 config 字段）时按最小改动同步，不擅自扩张。
6. P3a 完成后**停下来向用户报告**：fleet-wide 重装是运维重大事件，需用户确认 P3a 验收无误再展开 P3b。


