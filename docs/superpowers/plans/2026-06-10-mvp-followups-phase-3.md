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

# P3b · 多跳隧道控制面 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** 给 EMORELAY 加多跳隧道的「控制面」——DB 模型（tunnels/tunnel_hops）、protobuf 隧道上下文、隧道 REST CRUD（含链路校验/inter_port 分配/删除保护）、节点删除保护扩展、以及按 hop 把业务规则拆成 entry/mid/exit 三角色 Rule 的纯函数。**不含 Agent 转发实现**（数据面 = transport + TunnelTask，作为 P3b-数据面 在本段落地后展开）。

**Architecture:** 隧道是一串有序 hop（ordinal 0..N-1，N≥2）。`tunnels` 存 name/transport/status，`tunnel_hops` 存每跳 node_id + inter_port（被上一跳连入的监听端口）。业务规则 `forward_rules.tunnel_id` 关联隧道，其 `node_id` 必须 = 入口节点（ordinal 0）。dispatcher 把一条关联隧道的 Rule 拆成 N 个带 `TunnelContext` 的实例分发到各 hop 节点（限速只在 entry 计，mid/exit 置 0）。本段只做纯函数拆分 + 单测，**实际 dispatch 接入留数据面**（无 tunnel 能力的 Agent 收到拆分 Rule 会错误起普通 relay，故控制面阶段不接入真实下发）。

**Tech Stack:** Rust（SQLx/SQLite 事务 + tonic/prost）。

---

## P3b 控制面 关键设计决策（实现前必读）

1. **migration 编号 = `0006`**（spec §4.2 写 0003 已过时；实际已到 0005/P3a）。
2. **proto `Rule.tunnel` 字段号 = `12`,不是 spec §4.3 写的 11**——字段 11 已被 P2 的 `bandwidth_mbps` 占用。复用 11 会破坏 wire 兼容。`TunnelContext`/`TunnelRole`/`Command` oneof 6/7 按 spec。
3. **inter_port 语义（修正 spec §4.5 step3 的歧义）**:spec 写「为每个 ordinal < N-1 的节点分配」表述有歧义。按数据面 `task.rs` 的实际语义——inter_port 是「该 hop 被上一跳连入时监听的端口」,所以**ordinal ≥ 1 的 hop 才需要 inter_port**（共 N-1 个）,从该 hop **自己节点的 port_pool** 分配（排除 reserved + 已占用）；**entry(ordinal 0) 的 inter_port = NULL**(它监听业务规则的 listen_port,不被连入)。`tunnel_hops` 表每行存 inter_port,ordinal 0 行为 NULL。
4. **隧道全程统一 transport**（spec §5.3 已确认；用户选 A）。`tunnels.transport` ∈ {tcp, tls, wss},全链路一致。
5. **控制面阶段不真实 dispatch 拆分 Rule**:`split_tunnel_rule` 纯函数 + 单测就绪,但不接入 `rules.rs` create 的下发路径（接入留数据面,届时 Agent 能消费 tunnel 字段）。隧道创建本身也不 dispatch（spec §4.5 step5:隧道不带业务规则,等规则关联）。
6. 每个 Task 收尾跑测试 + `cargo test --workspace`,全绿 → commit → spawn 子代理走 `superpowers:code-reviewer`,通过才进下一 Task。

## P3b 控制面 文件结构（变更面）

**Create:**
- `migrations/0006_tunnels.sql` — tunnels + tunnel_hops + forward_rules.tunnel_id
- `crates/panel-server/src/models/tunnel.rs` — Tunnel + TunnelHop struct + CRUD/查询/删除保护方法
- `crates/panel-server/src/routes/tunnels.rs` — REST CRUD + 链路校验 + inter_port 分配
- `crates/panel-server/src/grpc/tunnel_split.rs` — `split_tunnel_rule` 纯函数(按 hop 拆 Rule)
- `crates/panel-server/tests/api_tunnels.rs` — 隧道 CRUD + 校验 + 删除保护集成测试
- `crates/panel-server/tests/tunnel_split.rs` — 拆分纯函数单测(双跳/三跳)

**Modify:**
- `crates/common/proto/control.proto` — Rule.tunnel=12 + TunnelContext + TunnelRole + Command oneof 6/7 + TunnelCredentials + RevokeTunnelCredentials
- `crates/panel-server/src/models/mod.rs` — `pub mod tunnel;`
- `crates/panel-server/src/routes/mod.rs` — 注册 7 个 /api/tunnels 路由
- `crates/panel-server/src/grpc/mod.rs` — `pub mod tunnel_split;`
- `crates/panel-server/src/routes/nodes.rs` — delete 扩展查 tunnel_hops 引用
- `crates/panel-server/src/routes/rules.rs` — create/update 加 tunnel_id 校验(node_id 必须=入口节点)
- `docs/api.md` / `plan.md` — P3b 控制面文档

---

## Task 1: migration 0006 + models/tunnel.rs

**Files:**
- Create: `migrations/0006_tunnels.sql`、`crates/panel-server/src/models/tunnel.rs`
- Modify: `crates/panel-server/src/models/mod.rs`
- Test: 扩展 `crates/panel-server/tests/api_tunnels.rs`（先建文件,放 model 级 sqlx 测试）

- [ ] **Step 1: 写 migration 0006**

```sql
-- migrations/0006_tunnels.sql
-- P3b 多跳隧道:tunnels(隧道定义) + tunnel_hops(有序跳) + forward_rules.tunnel_id(业务规则关联)。
-- PG 迁移:ADD COLUMN / CREATE TABLE / 部分唯一索引语法一致;datetime('now')→now()。
CREATE TABLE tunnels (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT    NOT NULL,
    transport   TEXT    NOT NULL CHECK (transport IN ('tcp', 'tls', 'wss')),
    status      TEXT    NOT NULL DEFAULT 'unknown'
                CHECK (status IN ('up', 'degraded', 'down', 'unknown')),
    created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    deleted_at  TEXT
);
CREATE UNIQUE INDEX idx_tunnels_name_active
    ON tunnels (name) WHERE deleted_at IS NULL;

CREATE TABLE tunnel_hops (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    tunnel_id   INTEGER NOT NULL REFERENCES tunnels(id),
    ordinal     INTEGER NOT NULL CHECK (ordinal >= 0),
    node_id     INTEGER NOT NULL REFERENCES nodes(id),
    -- 该 hop 被上一跳连入时监听的端口;ordinal 0(入口)为 NULL(它监听业务 listen_port)。
    inter_port  INTEGER CHECK (inter_port IS NULL OR (inter_port BETWEEN 1 AND 65535)),
    created_at  TEXT    NOT NULL DEFAULT (datetime('now'))
);
CREATE UNIQUE INDEX idx_tunnel_hops_tunnel_ordinal ON tunnel_hops (tunnel_id, ordinal);
CREATE INDEX idx_tunnel_hops_node_id ON tunnel_hops (node_id);

ALTER TABLE forward_rules ADD COLUMN tunnel_id INTEGER REFERENCES tunnels(id);
CREATE INDEX idx_forward_rules_tunnel_id ON forward_rules (tunnel_id);
```

- [ ] **Step 2: 写失败测试 `crates/panel-server/tests/api_tunnels.rs`(model 级)**

```rust
mod common;

use panel_server::models::tunnel::{Tunnel, TunnelHop};

#[tokio::test]
async fn create_tunnel_with_hops_and_read_back() {
    let app = common::make_app().await.unwrap();
    // 两个节点。
    let n1 = sqlx::query("INSERT INTO nodes (name, agent_token_hash, public_ip) VALUES ('hk', 'x', '1.1.1.1')")
        .execute(&app.state.pool).await.unwrap().last_insert_rowid();
    let n2 = sqlx::query("INSERT INTO nodes (name, agent_token_hash, public_ip) VALUES ('jp', 'x', '2.2.2.2')")
        .execute(&app.state.pool).await.unwrap().last_insert_rowid();

    // 事务建隧道 + 两跳:ordinal0(entry,inter_port NULL) + ordinal1(exit,inter_port 30001)。
    let tid = Tunnel::create_with_hops(
        &app.state.pool, "hk-jp", "tcp",
        &[(0, n1, None), (1, n2, Some(30001))],
    ).await.unwrap();

    let t = Tunnel::find_by_id(&app.state.pool, tid).await.unwrap().unwrap();
    assert_eq!(t.name, "hk-jp");
    assert_eq!(t.transport, "tcp");
    assert_eq!(t.status, "unknown");

    let hops = TunnelHop::list_for_tunnel(&app.state.pool, tid).await.unwrap();
    assert_eq!(hops.len(), 2);
    assert_eq!(hops[0].ordinal, 0);
    assert_eq!(hops[0].node_id, n1);
    assert!(hops[0].inter_port.is_none());
    assert_eq!(hops[1].ordinal, 1);
    assert_eq!(hops[1].inter_port, Some(30001));
}

#[tokio::test]
async fn soft_delete_hides_tunnel_and_active_refs_counts() {
    let app = common::make_app().await.unwrap();
    let n1 = sqlx::query("INSERT INTO nodes (name, agent_token_hash) VALUES ('a','x')")
        .execute(&app.state.pool).await.unwrap().last_insert_rowid();
    let n2 = sqlx::query("INSERT INTO nodes (name, agent_token_hash) VALUES ('b','x')")
        .execute(&app.state.pool).await.unwrap().last_insert_rowid();
    let tid = Tunnel::create_with_hops(&app.state.pool, "t1", "tls",
        &[(0, n1, None), (1, n2, Some(30002))]).await.unwrap();

    // 无规则引用 → active_rule_refs = 0。
    assert_eq!(Tunnel::active_rule_refs(&app.state.pool, tid).await.unwrap(), 0);
    // 软删后 find_by_id 不可见。
    assert_eq!(Tunnel::soft_delete(&app.state.pool, tid).await.unwrap(), 1);
    assert!(Tunnel::find_by_id(&app.state.pool, tid).await.unwrap().is_none());
}

#[tokio::test]
async fn hops_using_node_detects_node_membership() {
    let app = common::make_app().await.unwrap();
    let n1 = sqlx::query("INSERT INTO nodes (name, agent_token_hash) VALUES ('a','x')")
        .execute(&app.state.pool).await.unwrap().last_insert_rowid();
    let n2 = sqlx::query("INSERT INTO nodes (name, agent_token_hash) VALUES ('b','x')")
        .execute(&app.state.pool).await.unwrap().last_insert_rowid();
    Tunnel::create_with_hops(&app.state.pool, "t2", "tcp",
        &[(0, n1, None), (1, n2, Some(30003))]).await.unwrap();
    assert!(TunnelHop::node_in_active_tunnel(&app.state.pool, n2).await.unwrap());
    let n3 = sqlx::query("INSERT INTO nodes (name, agent_token_hash) VALUES ('c','x')")
        .execute(&app.state.pool).await.unwrap().last_insert_rowid();
    assert!(!TunnelHop::node_in_active_tunnel(&app.state.pool, n3).await.unwrap());
}
```

- [ ] **Step 3: 跑测试验证失败**

Run: `cargo test -p panel-server --test api_tunnels`
Expected: 编译 FAIL（`models::tunnel` 不存在）。

- [ ] **Step 4: 实现 models/tunnel.rs**

```rust
use sqlx::{prelude::FromRow, SqlitePool};

#[derive(Debug, Clone, FromRow)]
pub struct Tunnel {
    pub id: i64,
    pub name: String,
    pub transport: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct TunnelHop {
    pub id: i64,
    pub tunnel_id: i64,
    pub ordinal: i64,
    pub node_id: i64,
    pub inter_port: Option<i64>,
    pub created_at: String,
}

const TUNNEL_COLS: &str = "id, name, transport, status, created_at, updated_at";
const HOP_COLS: &str = "id, tunnel_id, ordinal, node_id, inter_port, created_at";

impl Tunnel {
    /// 事务建隧道 + N 跳。hops = &[(ordinal, node_id, inter_port)]。
    pub async fn create_with_hops(
        pool: &SqlitePool,
        name: &str,
        transport: &str,
        hops: &[(i64, i64, Option<i64>)],
    ) -> sqlx::Result<i64> {
        let mut tx = pool.begin().await?;
        let res = sqlx::query("INSERT INTO tunnels (name, transport) VALUES (?, ?)")
            .bind(name).bind(transport).execute(&mut *tx).await?;
        let tunnel_id = res.last_insert_rowid();
        for (ordinal, node_id, inter_port) in hops {
            sqlx::query(
                "INSERT INTO tunnel_hops (tunnel_id, ordinal, node_id, inter_port) VALUES (?, ?, ?, ?)",
            )
            .bind(tunnel_id).bind(ordinal).bind(node_id).bind(inter_port)
            .execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(tunnel_id)
    }

    pub async fn find_by_id(pool: &SqlitePool, id: i64) -> sqlx::Result<Option<Self>> {
        let sql = format!("SELECT {TUNNEL_COLS} FROM tunnels WHERE id = ? AND deleted_at IS NULL");
        sqlx::query_as(&sql).bind(id).fetch_optional(pool).await
    }

    pub async fn list_paged(pool: &SqlitePool, limit: i64, offset: i64) -> sqlx::Result<Vec<Self>> {
        let sql = format!(
            "SELECT {TUNNEL_COLS} FROM tunnels WHERE deleted_at IS NULL ORDER BY id DESC LIMIT ? OFFSET ?"
        );
        sqlx::query_as(&sql).bind(limit).bind(offset).fetch_all(pool).await
    }

    pub async fn count(pool: &SqlitePool) -> sqlx::Result<i64> {
        sqlx::query_scalar("SELECT COUNT(*) FROM tunnels WHERE deleted_at IS NULL")
            .fetch_one(pool).await
    }

    pub async fn update_name(pool: &SqlitePool, id: i64, name: &str) -> sqlx::Result<u64> {
        let res = sqlx::query(
            "UPDATE tunnels SET name = ?, updated_at = datetime('now') WHERE id = ? AND deleted_at IS NULL",
        ).bind(name).bind(id).execute(pool).await?;
        Ok(res.rows_affected())
    }

    pub async fn soft_delete(pool: &SqlitePool, id: i64) -> sqlx::Result<u64> {
        let res = sqlx::query(
            "UPDATE tunnels SET deleted_at = datetime('now'), updated_at = datetime('now') \
             WHERE id = ? AND deleted_at IS NULL",
        ).bind(id).execute(pool).await?;
        Ok(res.rows_affected())
    }

    /// 引用该隧道的活跃业务规则数(删除保护用)。
    pub async fn active_rule_refs(pool: &SqlitePool, id: i64) -> sqlx::Result<i64> {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM forward_rules WHERE tunnel_id = ? AND deleted_at IS NULL",
        ).bind(id).fetch_one(pool).await
    }
}

impl TunnelHop {
    pub async fn list_for_tunnel(pool: &SqlitePool, tunnel_id: i64) -> sqlx::Result<Vec<Self>> {
        let sql = format!(
            "SELECT {HOP_COLS} FROM tunnel_hops WHERE tunnel_id = ? ORDER BY ordinal"
        );
        sqlx::query_as(&sql).bind(tunnel_id).fetch_all(pool).await
    }

    /// 该节点是否参与任一活跃隧道(删节点保护用;隧道软删时其 hops 视为失效)。
    pub async fn node_in_active_tunnel(pool: &SqlitePool, node_id: i64) -> sqlx::Result<bool> {
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM tunnel_hops th \
             JOIN tunnels t ON t.id = th.tunnel_id \
             WHERE th.node_id = ? AND t.deleted_at IS NULL",
        ).bind(node_id).fetch_one(pool).await?;
        Ok(n > 0)
    }
}
```

`models/mod.rs` 加 `pub mod tunnel;`。

- [ ] **Step 5: 跑测试验证通过**

Run: `cargo test -p panel-server --test api_tunnels && cargo test --workspace`
Expected: 全 PASS。

- [ ] **Step 6: Commit**

```bash
git add migrations/0006_tunnels.sql crates/panel-server/src/models/tunnel.rs crates/panel-server/src/models/mod.rs crates/panel-server/tests/api_tunnels.rs
git commit -m "feat(db): tunnels + tunnel_hops schema; Tunnel/TunnelHop models with delete-protection queries"
```

## Task 2: proto — Rule.tunnel + TunnelContext + Command oneof 6/7

**Files:**
- Modify: `crates/common/proto/control.proto`
- Test: `crates/common/tests/proto_tunnel.rs`（新建,验证生成的类型可构造）

> proto 由 `crates/common/build.rs`（tonic-build + vendored protoc）在 `cargo build` 时生成,无需本地装 protoc。

- [ ] **Step 1: 写失败测试 `crates/common/tests/proto_tunnel.rs`**

```rust
//! 验证 P3b 隧道 proto 类型生成正确且可构造。
use emorelay_common::control::v1::{
    tunnel_role, Rule, TunnelContext, TunnelCredentials, RevokeTunnelCredentials,
    command::Body, Command, TunnelRole,
};

#[test]
fn rule_carries_tunnel_context() {
    let r = Rule {
        id: 1,
        protocol: "tcp".into(),
        listen_ip: "0.0.0.0".into(),
        listen_port: 20000,
        target_host: "1.2.3.4".into(),
        target_port: 443,
        enabled: true,
        bandwidth_mbps: 0,
        tunnel: Some(TunnelContext {
            tunnel_id: 7,
            role: TunnelRole::Entry as i32,
            next_hop_addr: "2.2.2.2".into(),
            next_hop_inter_port: 30001,
            self_inter_port: 0,
            transport: "tcp".into(),
        }),
    };
    assert_eq!(r.tunnel.as_ref().unwrap().tunnel_id, 7);
    assert_eq!(r.tunnel.as_ref().unwrap().role, tunnel_role::Entry as i32);
}

#[test]
fn command_oneof_has_tunnel_credentials() {
    let c = Command {
        body: Some(Body::TunnelCredentials(TunnelCredentials {
            tunnel_id: 7,
            ordinal: 1,
            server_cert_pem: "S".into(),
            server_key_pem: "SK".into(),
            client_cert_pem: "C".into(),
            client_key_pem: "CK".into(),
        })),
    };
    assert!(matches!(c.body, Some(Body::TunnelCredentials(_))));

    let r = Command {
        body: Some(Body::RevokeTunnelCredentials(RevokeTunnelCredentials { tunnel_id: 7 })),
    };
    assert!(matches!(r.body, Some(Body::RevokeTunnelCredentials(_))));
}
```

（`tunnel_role` 模块名与 `TunnelRole` 枚举名按 prost 生成实际为准；prost 把 enum 变体生成为 `TunnelRole::Entry`。若 `tunnel_role::Entry` 路径不存在则只用 `TunnelRole::Entry as i32`,删去 `tunnel_role` import 与该断言行——以编译通过为准,不改语义。）

- [ ] **Step 2: 跑测试验证失败**

Run: `cargo test -p emorelay-common --test proto_tunnel`
Expected: 编译 FAIL（Rule 无 tunnel 字段 / TunnelContext 不存在）。

- [ ] **Step 3: 改 proto**

`crates/common/proto/control.proto` 的 `message Rule`,在 `int64 bandwidth_mbps = 11;` 之后加：

```proto
  // P3b 多跳隧道上下文。无隧道 → None。字段号 12(11 已被 bandwidth_mbps 占)。
  TunnelContext tunnel = 12;
```

在 `Rule` 之后(`ApplyRule` 之前)加：

```proto
// 多跳隧道上下文。dispatcher 按 hop 拆 Rule 时填充,告诉 Agent 自己的角色与下一跳。
message TunnelContext {
  int64 tunnel_id = 1;
  TunnelRole role = 2;
  string next_hop_addr = 3;        // 下一跳节点可达地址(public_ip)
  uint32 next_hop_inter_port = 4;  // 下一跳监听的 inter_port(自己 dial 的目标)
  uint32 self_inter_port = 5;      // 本跳监听端口(mid/exit 用;entry 监听业务 listen_port,置 0)
  string transport = 6;            // tcp / tls / wss(全链路统一)
}

enum TunnelRole {
  TUNNEL_ROLE_UNSPECIFIED = 0;
  TUNNEL_ROLE_ENTRY = 1;
  TUNNEL_ROLE_MID = 2;
  TUNNEL_ROLE_EXIT = 3;
}

// 隧道 hop 的 TLS 凭据(transport=tls/wss 时,由面板 CA 在隧道创建时签发并下发)。
// Agent 落盘到 ${AGENT_DATA_DIR}/tunnels/<id>/hop-<ordinal>/{server,client}.{pem,key}。
message TunnelCredentials {
  int64 tunnel_id = 1;
  int32 ordinal = 2;
  string server_cert_pem = 3;
  string server_key_pem = 4;
  string client_cert_pem = 5;
  string client_key_pem = 6;
}

message RevokeTunnelCredentials {
  int64 tunnel_id = 1;
}
```

`message Command` 的 `oneof body` 在 `RestartRule restart_rule = 5;` 之后加：

```proto
    TunnelCredentials tunnel_credentials = 6;
    RevokeTunnelCredentials revoke_tunnel_credentials = 7;
```

- [ ] **Step 4: 跑测试验证通过**

Run: `cargo test -p emorelay-common --test proto_tunnel && cargo test --workspace`
Expected: 全 PASS。注意:proto 加 `Rule.tunnel` 字段后,所有构造 `Rule {}` 字面量的地方（panel-server `grpc/commands.rs::rule_to_proto`、node-agent `store.rs` 的 RuleJson From、relay tcp/udp 测试的 `rule_for`）会因缺 `tunnel` 字段**编译失败**。逐处补 `tunnel: None`:
- `grpc/commands.rs::rule_to_proto`: 末尾加 `tunnel: None,`
- `node-agent/src/store.rs`: `From<RuleJson> for Rule` 末尾加 `tunnel: None,`（RuleJson 本身不加 tunnel 字段——Agent 持久化暂不含隧道,数据面再扩展;`From<&Rule> for RuleJson` 忽略 tunnel）
- `node-agent/src/relay/tcp.rs` + `udp.rs` 测试 `rule_for()`: 加 `tunnel: None,`
这些是 proto 加字段的机械同步,本 Task 一并改（属「因你的改动产生的编译错误」）。

- [ ] **Step 5: Commit**

```bash
git add crates/common/proto/control.proto crates/common/tests/proto_tunnel.rs crates/panel-server/src/grpc/commands.rs crates/node-agent/src/store.rs crates/node-agent/src/relay/tcp.rs crates/node-agent/src/relay/udp.rs
git commit -m "feat(proto): Rule.tunnel context + TunnelRole + Command tunnel-credentials branches"
```

---

## Task 3: routes/tunnels.rs CRUD + 链路校验 + inter_port 分配 + 删除保护

**Files:**
- Create: `crates/panel-server/src/routes/tunnels.rs`
- Modify: `crates/panel-server/src/routes/mod.rs`
- Test: 扩展 `crates/panel-server/tests/api_tunnels.rs`

- [ ] **Step 1: 写失败测试（追加 api_tunnels.rs）**

```rust
use axum::http::{Method, StatusCode};
use serde_json::json;

/// 建 N 个 online 节点,port_pool [30000,30010],返回 ids。
async fn seed_online_nodes(app: &common::TestApp, n: usize) -> Vec<i64> {
    let mut ids = Vec::new();
    for i in 0..n {
        let id = sqlx::query(
            "INSERT INTO nodes (name, agent_token_hash, status, public_ip, port_pool_min, port_pool_max) \
             VALUES (?, 'x', 'online', ?, 30000, 30010)",
        )
        .bind(format!("tn{i}")).bind(format!("10.0.0.{i}"))
        .execute(&app.state.pool).await.unwrap().last_insert_rowid();
        ids.push(id);
    }
    ids
}

#[tokio::test]
async fn create_tunnel_allocates_inter_ports_and_lists() {
    let app = common::make_app().await.unwrap();
    let nodes = seed_online_nodes(&app, 3).await;
    let req = common::auth_req(Method::POST, "/api/tunnels", &app.admin_token,
        Some(json!({ "name": "hk-jp-us", "transport": "tcp", "node_ids": nodes }))).unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    let tid = body["id"].as_i64().unwrap();

    // GET :id 含 hops,ordinal0 inter_port=null,ordinal≥1 有分配端口(池内)。
    let req = common::auth_req(Method::GET, &format!("/api/tunnels/{tid}"), &app.admin_token, None).unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    let hops = body["hops"].as_array().unwrap();
    assert_eq!(hops.len(), 3);
    assert!(hops[0]["inter_port"].is_null());
    let p1 = hops[1]["inter_port"].as_i64().unwrap();
    let p2 = hops[2]["inter_port"].as_i64().unwrap();
    assert!((30000..=30010).contains(&p1) && (30000..=30010).contains(&p2));

    // list 含 hops_count。
    let req = common::auth_req(Method::GET, "/api/tunnels", &app.admin_token, None).unwrap();
    let (_, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(body["total"], 1);
    assert_eq!(body["items"][0]["hops_count"], 3);
}

#[tokio::test]
async fn create_tunnel_rejects_short_chain_dup_and_offline() {
    let app = common::make_app().await.unwrap();
    let nodes = seed_online_nodes(&app, 2).await;
    // < 2 节点。
    let req = common::auth_req(Method::POST, "/api/tunnels", &app.admin_token,
        Some(json!({ "name": "x", "transport": "tcp", "node_ids": [nodes[0]] }))).unwrap();
    let (s, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(s, StatusCode::BAD_REQUEST);
    // 重复节点。
    let req = common::auth_req(Method::POST, "/api/tunnels", &app.admin_token,
        Some(json!({ "name": "x", "transport": "tcp", "node_ids": [nodes[0], nodes[0]] }))).unwrap();
    let (s, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(s, StatusCode::BAD_REQUEST);
    // 含 offline 节点。
    let off = sqlx::query("INSERT INTO nodes (name, agent_token_hash, status) VALUES ('off','x','offline')")
        .execute(&app.state.pool).await.unwrap().last_insert_rowid();
    let req = common::auth_req(Method::POST, "/api/tunnels", &app.admin_token,
        Some(json!({ "name": "x", "transport": "tcp", "node_ids": [nodes[0], off] }))).unwrap();
    let (s, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["message"].as_str().unwrap().contains("online"));
}

#[tokio::test]
async fn delete_tunnel_blocked_by_rule_reference() {
    let app = common::make_app().await.unwrap();
    let nodes = seed_online_nodes(&app, 2).await;
    let req = common::auth_req(Method::POST, "/api/tunnels", &app.admin_token,
        Some(json!({ "name": "t", "transport": "tcp", "node_ids": nodes }))).unwrap();
    let (_, body) = common::send(app.app.clone(), req).await.unwrap();
    let tid = body["id"].as_i64().unwrap();
    // 入口节点上挂一条关联隧道的规则。
    sqlx::query(
        "INSERT INTO forward_rules (user_id, node_id, name, protocol, listen_ip, listen_port, target_host, target_port, tunnel_id) \
         VALUES (?, ?, 'r', 'tcp', '0.0.0.0', 20000, '1.2.3.4', 443, ?)",
    ).bind(app.admin_user_id).bind(nodes[0]).bind(tid)
    .execute(&app.state.pool).await.unwrap();

    let req = common::auth_req(Method::DELETE, &format!("/api/tunnels/{tid}"), &app.admin_token, None).unwrap();
    let (s, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["message"].as_str().unwrap().contains("1"));
}

#[tokio::test]
async fn patch_only_name_and_requires_admin() {
    let app = common::make_app().await.unwrap();
    let nodes = seed_online_nodes(&app, 2).await;
    let req = common::auth_req(Method::POST, "/api/tunnels", &app.admin_token,
        Some(json!({ "name": "t", "transport": "tcp", "node_ids": nodes }))).unwrap();
    let (_, body) = common::send(app.app.clone(), req).await.unwrap();
    let tid = body["id"].as_i64().unwrap();
    let req = common::auth_req(Method::PATCH, &format!("/api/tunnels/{tid}"), &app.admin_token,
        Some(json!({ "name": "renamed" }))).unwrap();
    let (s, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["name"], "renamed");
    // 非 admin。
    let (_uid, token) = common::make_user_token(&app, "u", "password123").await.unwrap();
    let req = common::auth_req(Method::GET, "/api/tunnels", &token, None).unwrap();
    let (s, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(s, StatusCode::FORBIDDEN);
}
```

- [ ] **Step 2: 跑测试验证失败**

Run: `cargo test -p panel-server --test api_tunnels`
Expected: FAIL（路由不存在）。

- [ ] **Step 3: 实现 routes/tunnels.rs**

```rust
use crate::{
    audit,
    auth::extractor::{ActorIp, AuthUser},
    error::{ApiError, ApiResult},
    models::{settings, tunnel::{Tunnel, TunnelHop}},
    state::AppState,
};
use axum::{extract::{Path, Query, State}, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Serialize)]
pub struct TunnelView {
    pub id: i64,
    pub name: String,
    pub transport: String,
    pub status: String,
    pub hops_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Serialize)]
pub struct HopView {
    pub ordinal: i64,
    pub node_id: i64,
    pub inter_port: Option<i64>,
}

#[derive(Serialize)]
pub struct TunnelDetail {
    pub id: i64,
    pub name: String,
    pub transport: String,
    pub status: String,
    pub hops: Vec<HopView>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Deserialize)]
pub struct ListQuery { pub page: Option<i64>, pub page_size: Option<i64> }

#[derive(Serialize)]
pub struct TunnelListResponse {
    pub items: Vec<TunnelView>, pub total: i64, pub page: i64, pub page_size: i64,
}

#[derive(Deserialize)]
pub struct CreateTunnelRequest {
    pub name: String,
    pub transport: String,
    pub node_ids: Vec<i64>,
}

#[derive(Deserialize)]
pub struct UpdateTunnelRequest { pub name: Option<String> }

pub async fn list(
    State(state): State<AppState>, auth: AuthUser, Query(q): Query<ListQuery>,
) -> ApiResult<Json<TunnelListResponse>> {
    auth.require_admin()?;
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).clamp(1, 100);
    let offset = page.saturating_sub(1).saturating_mul(page_size);
    let tunnels = Tunnel::list_paged(&state.pool, page_size, offset).await?;
    let total = Tunnel::count(&state.pool).await?;
    let mut items = Vec::with_capacity(tunnels.len());
    for t in tunnels {
        let hops = TunnelHop::list_for_tunnel(&state.pool, t.id).await?;
        items.push(TunnelView {
            id: t.id, name: t.name, transport: t.transport, status: t.status,
            hops_count: hops.len() as i64, created_at: t.created_at, updated_at: t.updated_at,
        });
    }
    Ok(Json(TunnelListResponse { items, total, page, page_size }))
}

pub async fn get(
    State(state): State<AppState>, auth: AuthUser, Path(id): Path<i64>,
) -> ApiResult<Json<TunnelDetail>> {
    auth.require_admin()?;
    let t = Tunnel::find_by_id(&state.pool, id).await?.ok_or(ApiError::NotFound)?;
    let hops = TunnelHop::list_for_tunnel(&state.pool, id).await?;
    Ok(Json(TunnelDetail {
        id: t.id, name: t.name, transport: t.transport, status: t.status,
        hops: hops.into_iter().map(|h| HopView { ordinal: h.ordinal, node_id: h.node_id, inter_port: h.inter_port }).collect(),
        created_at: t.created_at, updated_at: t.updated_at,
    }))
}

pub async fn create(
    State(state): State<AppState>, auth: AuthUser, actor_ip: ActorIp,
    Json(req): Json<CreateTunnelRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    auth.require_admin()?;
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest("name is required".into()));
    }
    if !matches!(req.transport.as_str(), "tcp" | "tls" | "wss") {
        return Err(ApiError::BadRequest("transport must be tcp | tls | wss".into()));
    }
    // 链路 ≥ 2 + 不重复。
    if req.node_ids.len() < 2 {
        return Err(ApiError::BadRequest("tunnel needs at least 2 nodes".into()));
    }
    let mut seen = std::collections::HashSet::new();
    for nid in &req.node_ids {
        if !seen.insert(*nid) {
            return Err(ApiError::BadRequest("node_ids must be unique".into()));
        }
    }
    // 每个节点存在 + online,记录 port_pool。
    #[derive(sqlx::FromRow)]
    struct NodeRow { id: i64, status: String, port_pool_min: i64, port_pool_max: i64 }
    let reserved = settings::reserved_ports(&state.pool).await;
    let mut pools: std::collections::HashMap<i64, (i64, i64)> = std::collections::HashMap::new();
    for nid in &req.node_ids {
        let row: Option<NodeRow> = sqlx::query_as(
            "SELECT id, status, port_pool_min, port_pool_max FROM nodes WHERE id = ? AND deleted_at IS NULL",
        ).bind(nid).fetch_optional(&state.pool).await?;
        let row = row.ok_or_else(|| ApiError::BadRequest(format!("node {nid} does not exist")))?;
        if row.status != "online" {
            return Err(ApiError::BadRequest(
                "请确保链上所有节点都在线 (all nodes must be online)".into(),
            ));
        }
        pools.insert(row.id, (row.port_pool_min, row.port_pool_max));
    }
    // 为 ordinal ≥ 1 的 hop 在其节点 port_pool 分配 inter_port(排除 reserved + 该节点已占用 listen_port + 同隧道已分配)。
    let mut hops: Vec<(i64, i64, Option<i64>)> = Vec::with_capacity(req.node_ids.len());
    for (ordinal, nid) in req.node_ids.iter().enumerate() {
        if ordinal == 0 {
            hops.push((0, *nid, None));
            continue;
        }
        let (lo, hi) = pools[nid];
        // 该节点已占用端口(活跃 forward_rules.listen_port + 已分配的 inter_port)。
        let taken: Vec<i64> = sqlx::query_scalar(
            "SELECT listen_port FROM forward_rules WHERE node_id = ? AND deleted_at IS NULL \
             UNION SELECT th.inter_port FROM tunnel_hops th JOIN tunnels t ON t.id = th.tunnel_id \
             WHERE th.node_id = ? AND th.inter_port IS NOT NULL AND t.deleted_at IS NULL",
        ).bind(nid).bind(nid).fetch_all(&state.pool).await?;
        let already: std::collections::HashSet<i64> =
            hops.iter().filter(|(_, n, _)| n == nid).filter_map(|(_, _, p)| *p).collect();
        let port = (lo..=hi).find(|p| {
            !reserved.contains(p) && !taken.contains(p) && !already.contains(p)
        }).ok_or_else(|| ApiError::BadRequest(format!(
            "node {nid} port pool exhausted for inter_port allocation"
        )))?;
        hops.push((ordinal as i64, *nid, Some(port)));
    }

    let tid = Tunnel::create_with_hops(&state.pool, name, &req.transport, &hops)
        .await
        .map_err(map_sqlx_to_api)?;

    audit::record_with_ip(&state.pool, Some(auth.0.sub), actor_ip.as_option(),
        "tunnel.create", Some("tunnel"), Some(tid), Some(name), true, None).await;

    // 控制面阶段不 dispatch(隧道不带业务规则,等规则关联;真正下发留数据面)。
    Ok(Json(json!({ "id": tid })))
}

pub async fn update(
    State(state): State<AppState>, auth: AuthUser, actor_ip: ActorIp,
    Path(id): Path<i64>, Json(req): Json<UpdateTunnelRequest>,
) -> ApiResult<Json<TunnelView>> {
    auth.require_admin()?;
    if let Some(name) = req.name.as_deref() {
        if name.trim().is_empty() {
            return Err(ApiError::BadRequest("name cannot be empty".into()));
        }
        let rows = Tunnel::update_name(&state.pool, id, name.trim()).await.map_err(map_sqlx_to_api)?;
        if rows == 0 { return Err(ApiError::NotFound); }
    }
    let t = Tunnel::find_by_id(&state.pool, id).await?.ok_or(ApiError::NotFound)?;
    let hops = TunnelHop::list_for_tunnel(&state.pool, id).await?;
    audit::record_with_ip(&state.pool, Some(auth.0.sub), actor_ip.as_option(),
        "tunnel.update", Some("tunnel"), Some(id), None, true, None).await;
    Ok(Json(TunnelView {
        id: t.id, name: t.name, transport: t.transport, status: t.status,
        hops_count: hops.len() as i64, created_at: t.created_at, updated_at: t.updated_at,
    }))
}

pub async fn delete(
    State(state): State<AppState>, auth: AuthUser, actor_ip: ActorIp, Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    auth.require_admin()?;
    let _t = Tunnel::find_by_id(&state.pool, id).await?.ok_or(ApiError::NotFound)?;
    let refs = Tunnel::active_rule_refs(&state.pool, id).await?;
    if refs > 0 {
        return Err(ApiError::BadRequest(format!(
            "tunnel is referenced by {refs} active rule(s); detach them first"
        )));
    }
    let rows = Tunnel::soft_delete(&state.pool, id).await?;
    if rows == 0 { return Err(ApiError::NotFound); }
    audit::record_with_ip(&state.pool, Some(auth.0.sub), actor_ip.as_option(),
        "tunnel.delete", Some("tunnel"), Some(id), None, true, None).await;
    Ok(Json(json!({ "ok": true })))
}

/// 控制面阶段:restart 仅记 audit(真正下发 TunnelTask 重启留数据面)。
pub async fn restart(
    State(state): State<AppState>, auth: AuthUser, actor_ip: ActorIp, Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    auth.require_admin()?;
    let _t = Tunnel::find_by_id(&state.pool, id).await?.ok_or(ApiError::NotFound)?;
    audit::record_with_ip(&state.pool, Some(auth.0.sub), actor_ip.as_option(),
        "tunnel.restart", Some("tunnel"), Some(id), None, true, None).await;
    Ok(Json(json!({ "ok": true, "dispatched": false })))
}

/// 控制面阶段:status 返回 tunnels.status 字段值(数据面接 hop 心跳后更新)。
pub async fn status(
    State(state): State<AppState>, auth: AuthUser, Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    auth.require_admin()?;
    let t = Tunnel::find_by_id(&state.pool, id).await?.ok_or(ApiError::NotFound)?;
    Ok(Json(json!({ "id": t.id, "status": t.status })))
}

fn map_sqlx_to_api(e: sqlx::Error) -> ApiError {
    if let Some(db) = e.as_database_error() {
        if db.is_unique_violation() {
            return ApiError::BadRequest("tunnel name already exists".into());
        }
        if db.is_check_violation() {
            return ApiError::BadRequest("invalid tunnel fields (check constraint)".into());
        }
    }
    ApiError::Database(e)
}
```

`routes/mod.rs`：`pub mod tunnels;` + 注册（在 bandwidth-profiles 路由块后）：

```rust
        .route("/api/tunnels", get(tunnels::list).post(tunnels::create))
        .route(
            "/api/tunnels/{id}",
            get(tunnels::get).patch(tunnels::update).delete(tunnels::delete),
        )
        .route("/api/tunnels/{id}/restart", post(tunnels::restart))
        .route("/api/tunnels/{id}/status", get(tunnels::status))
```

- [ ] **Step 4: 跑测试验证通过**

Run: `cargo test -p panel-server --test api_tunnels && cargo test --workspace`
Expected: 全 PASS。

- [ ] **Step 5: Commit**

```bash
git add crates/panel-server/src/routes/tunnels.rs crates/panel-server/src/routes/mod.rs crates/panel-server/tests/api_tunnels.rs
git commit -m "feat(server): tunnels CRUD with chain validation, inter_port allocation, delete protection"
```

## Task 4: 节点删除保护扩展 + 规则关联隧道校验

**Files:**
- Modify: `crates/panel-server/src/models/rule.rs`（Rule struct + create 加 tunnel_id）、`crates/panel-server/src/routes/rules.rs`（DTO + 校验）、`crates/panel-server/src/routes/nodes.rs`（delete 查 tunnel_hops）
- Test: `crates/panel-server/tests/api_tunnels.rs`（追加）

- [ ] **Step 1: 写失败测试（追加 api_tunnels.rs）**

```rust
#[tokio::test]
async fn delete_node_blocked_when_in_tunnel() {
    let app = common::make_app().await.unwrap();
    let nodes = seed_online_nodes(&app, 2).await;
    let req = common::auth_req(Method::POST, "/api/tunnels", &app.admin_token,
        Some(json!({ "name": "t", "transport": "tcp", "node_ids": nodes }))).unwrap();
    let (_, _b) = common::send(app.app.clone(), req).await.unwrap();
    // 删参与隧道的节点 → 400。
    let req = common::auth_req(Method::DELETE, &format!("/api/nodes/{}", nodes[1]), &app.admin_token, None).unwrap();
    let (s, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["message"].as_str().unwrap().contains("隧道") || body["message"].as_str().unwrap().contains("tunnel"));
}

#[tokio::test]
async fn rule_tunnel_id_must_match_entry_node() {
    let app = common::make_app().await.unwrap();
    let nodes = seed_online_nodes(&app, 2).await;
    let req = common::auth_req(Method::POST, "/api/tunnels", &app.admin_token,
        Some(json!({ "name": "t", "transport": "tcp", "node_ids": nodes }))).unwrap();
    let (_, body) = common::send(app.app.clone(), req).await.unwrap();
    let tid = body["id"].as_i64().unwrap();
    // node_id = 入口(nodes[0]) → OK。
    let req = common::auth_req(Method::POST, "/api/rules", &app.admin_token,
        Some(json!({ "node_id": nodes[0], "name": "ok", "protocol": "tcp", "listen_port": 20000,
                     "target_host": "1.2.3.4", "target_port": 443, "tunnel_id": tid }))).unwrap();
    let (s, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["tunnel_id"], tid);
    // node_id = 非入口(nodes[1]) → 400。
    let req = common::auth_req(Method::POST, "/api/rules", &app.admin_token,
        Some(json!({ "node_id": nodes[1], "name": "bad", "protocol": "tcp", "listen_port": 20001,
                     "target_host": "1.2.3.4", "target_port": 443, "tunnel_id": tid }))).unwrap();
    let (s, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["message"].as_str().unwrap().contains("entry"));
}
```

- [ ] **Step 2: 跑测试验证失败**

Run: `cargo test -p panel-server --test api_tunnels`
Expected: FAIL（删节点未拦 / 规则无 tunnel_id）。

- [ ] **Step 3: models/rule.rs — Rule struct + create 加 tunnel_id**

`Rule` struct 在 `bandwidth_mbps` 之后加 `pub tunnel_id: Option<i64>,`。

`RULE_COLUMNS` 把 `bandwidth_profile_id,` 那段之后追加 `tunnel_id`(放在派生 bandwidth_mbps 子查询之后、created_at 之前)：

```rust
const RULE_COLUMNS: &str = "id, user_id, node_id, name, protocol, listen_ip, listen_port, \
    target_host, target_port, enabled, rx_bytes, tx_bytes, connection_count, \
    bandwidth_profile_id, \
    (SELECT bp.bandwidth_mbps FROM bandwidth_profiles bp \
        WHERE bp.id = forward_rules.bandwidth_profile_id AND bp.deleted_at IS NULL) AS bandwidth_mbps, \
    tunnel_id, created_at, updated_at";
```

`create` 加末参 `tunnel_id: Option<i64>`,INSERT 列加 `tunnel_id`、VALUES 加一个 `?`、bind 加 `.bind(tunnel_id)`（放 bandwidth_profile_id 之后）：

```rust
            "INSERT INTO forward_rules \
                (user_id, node_id, name, protocol, listen_ip, listen_port, \
                 target_host, target_port, bandwidth_profile_id, tunnel_id) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
```
（`update_fields` 不动 tunnel_id——隧道关联在 create 时定;update 不改。）

- [ ] **Step 4: routes/rules.rs — DTO + 校验**

`RuleView` 加 `pub tunnel_id: Option<i64>,`;`From<Rule>` 同步 `tunnel_id: r.tunnel_id,`。
`CreateRuleRequest` 加 `pub tunnel_id: Option<i64>,`。
create handler 在 node 查询之后、端口解析之前加：

```rust
    // 关联隧道:tunnel_id 给定时,node_id 必须 = 隧道入口(ordinal 0)节点。
    if let Some(tid) = req.tunnel_id {
        use crate::models::tunnel::TunnelHop;
        let hops = TunnelHop::list_for_tunnel(&state.pool, tid).await?;
        let entry = hops.iter().find(|h| h.ordinal == 0)
            .ok_or_else(|| ApiError::BadRequest("tunnel_id does not exist".into()))?;
        if entry.node_id != req.node_id {
            return Err(ApiError::BadRequest(
                "rule.node_id must equal the tunnel entry (ordinal 0) node".into()));
        }
    }
```

`Rule::create(...)` 调用末尾加 `req.tunnel_id`。

- [ ] **Step 5: routes/nodes.rs — delete 扩展查 tunnel_hops**

在现有「查 forward_rules 引用」块之后、`Node::soft_delete` 之前加：

```rust
    // 节点参与任一活跃隧道 → 拒删(P3b)。
    if crate::models::tunnel::TunnelHop::node_in_active_tunnel(&state.pool, id).await? {
        return Err(ApiError::BadRequest(
            "节点正参与活跃隧道,请先删除相关隧道 (node is part of an active tunnel)".into()));
    }
```

- [ ] **Step 6: 跑测试验证通过**

Run: `cargo test -p panel-server --test api_tunnels && cargo test -p panel-server --test api_rules && cargo test --workspace`
Expected: 全 PASS（注意 api_rules 既有测试因 RuleView 加 tunnel_id 字段不受影响——serde 加字段不破坏旧断言;Rule::create 调用点若别处有需同步加 tunnel_id 实参,grep 确认仅 rules.rs + rules_io.rs::execute_create 两处——rules_io.rs 的导入路径也要补 `None`）。

> **重要**：`rules_io.rs::execute_create` 也调 `Rule::create`,加 tunnel_id 参数后此处编译失败,补末参 `None`（导入的规则不关联隧道——隧道关联跨实例不可控,与 bandwidth_profile 同理）。本 Task 一并改 `crates/panel-server/src/routes/rules_io.rs`。

- [ ] **Step 7: Commit**

```bash
git add crates/panel-server/src/models/rule.rs crates/panel-server/src/routes/rules.rs crates/panel-server/src/routes/rules_io.rs crates/panel-server/src/routes/nodes.rs crates/panel-server/tests/api_tunnels.rs
git commit -m "feat(server): node-delete blocked by tunnel membership; rule.tunnel_id must match entry node"
```

---

## Task 5: dispatcher 按 hop 拆 Rule（split_tunnel_rule 纯函数）

**Files:**
- Create: `crates/panel-server/src/grpc/tunnel_split.rs`
- Modify: `crates/panel-server/src/grpc/mod.rs`（`pub mod tunnel_split;`）
- Test: `crates/panel-server/tests/tunnel_split.rs`

> 本 Task 只做纯函数 + 单测。**不接入** rules.rs 的实际 dispatch 路径——无 tunnel 能力的 Agent 收到拆分 Rule 会错误起普通 relay,真实下发留数据面（届时 Agent 消费 tunnel 字段走 TunnelTask）。

- [ ] **Step 1: 写失败测试 `crates/panel-server/tests/tunnel_split.rs`**

```rust
use emorelay_common::control::v1::TunnelRole;
use panel_server::grpc::tunnel_split::{split_tunnel_rule, SplitInput, HopInput};

fn rule_input() -> SplitInput {
    SplitInput {
        rule_id: 100,
        protocol: "tcp".into(),
        listen_ip: "0.0.0.0".into(),
        listen_port: 20000,
        target_host: "9.9.9.9".into(),
        target_port: 443,
        enabled: true,
        bandwidth_mbps: 50,
        tunnel_id: 7,
        transport: "tls".into(),
    }
}

#[test]
fn two_hop_split_entry_and_exit() {
    let hops = vec![
        HopInput { node_id: 1, inter_port: None,        addr: "10.0.0.1".into() }, // entry
        HopInput { node_id: 2, inter_port: Some(30001), addr: "10.0.0.2".into() }, // exit
    ];
    let out = split_tunnel_rule(&rule_input(), &hops);
    assert_eq!(out.len(), 2);

    // entry
    let (n0, r0) = &out[0];
    assert_eq!(*n0, 1);
    let t0 = r0.tunnel.as_ref().unwrap();
    assert_eq!(t0.role, TunnelRole::Entry as i32);
    assert_eq!(t0.next_hop_addr, "10.0.0.2");
    assert_eq!(t0.next_hop_inter_port, 30001);
    assert_eq!(t0.self_inter_port, 0);
    assert_eq!(t0.transport, "tls");
    assert_eq!(r0.listen_port, 20000);     // entry 监听业务端口
    assert_eq!(r0.bandwidth_mbps, 50);     // 限速只在 entry

    // exit
    let (n1, r1) = &out[1];
    assert_eq!(*n1, 2);
    let t1 = r1.tunnel.as_ref().unwrap();
    assert_eq!(t1.role, TunnelRole::Exit as i32);
    assert_eq!(t1.self_inter_port, 30001); // exit 监听自己的 inter_port
    assert_eq!(t1.next_hop_inter_port, 0); // 无下一跳
    assert_eq!(r1.target_host, "9.9.9.9"); // exit 连业务目标
    assert_eq!(r1.bandwidth_mbps, 0);      // 非 entry 不重复限速
}

#[test]
fn three_hop_split_has_mid() {
    let hops = vec![
        HopInput { node_id: 1, inter_port: None,        addr: "10.0.0.1".into() },
        HopInput { node_id: 2, inter_port: Some(30001), addr: "10.0.0.2".into() },
        HopInput { node_id: 3, inter_port: Some(30002), addr: "10.0.0.3".into() },
    ];
    let out = split_tunnel_rule(&rule_input(), &hops);
    assert_eq!(out.len(), 3);
    let t_mid = out[1].1.tunnel.as_ref().unwrap();
    assert_eq!(t_mid.role, TunnelRole::Mid as i32);
    assert_eq!(t_mid.self_inter_port, 30001);     // mid 监听自己
    assert_eq!(t_mid.next_hop_addr, "10.0.0.3");   // dial 下一跳
    assert_eq!(t_mid.next_hop_inter_port, 30002);
    assert_eq!(out[1].1.bandwidth_mbps, 0);
    // exit
    assert_eq!(out[2].1.tunnel.as_ref().unwrap().role, TunnelRole::Exit as i32);
}
```

- [ ] **Step 2: 跑测试验证失败**

Run: `cargo test -p panel-server --test tunnel_split`
Expected: 编译 FAIL（`grpc::tunnel_split` 不存在）。

- [ ] **Step 3: 实现 tunnel_split.rs**

```rust
//! 把一条关联隧道的业务规则按 hop 拆成 N 个带 TunnelContext 的 proto Rule(P3b)。
//! entry 监听业务 listen_port + dial 下一跳;mid 监听 self_inter_port + dial 下一跳;
//! exit 监听 self_inter_port + connect 业务 target。限速只在 entry 计(mid/exit bandwidth_mbps=0)。
//! 纯函数,便于单测;dispatch 接入留数据面。
use emorelay_common::control::v1::{Rule as ProtoRule, TunnelContext, TunnelRole};

/// 拆分输入:业务规则字段 + 隧道 id/transport。
pub struct SplitInput {
    pub rule_id: i64,
    pub protocol: String,
    pub listen_ip: String,
    pub listen_port: u32,
    pub target_host: String,
    pub target_port: u32,
    pub enabled: bool,
    pub bandwidth_mbps: i64,
    pub tunnel_id: i64,
    pub transport: String,
}

/// 单跳输入:节点 id + 该跳监听端口(entry 为 None)+ 节点可达地址。
pub struct HopInput {
    pub node_id: i64,
    pub inter_port: Option<i64>,
    pub addr: String,
}

/// 返回 (node_id, 该节点上要跑的 proto Rule)。hops 按 ordinal 升序。
pub fn split_tunnel_rule(input: &SplitInput, hops: &[HopInput]) -> Vec<(i64, ProtoRule)> {
    let n = hops.len();
    hops.iter().enumerate().map(|(i, hop)| {
        let role = if i == 0 {
            TunnelRole::Entry
        } else if i == n - 1 {
            TunnelRole::Exit
        } else {
            TunnelRole::Mid
        };
        let next = hops.get(i + 1);
        let tunnel = TunnelContext {
            tunnel_id: input.tunnel_id,
            role: role as i32,
            next_hop_addr: next.map(|h| h.addr.clone()).unwrap_or_default(),
            next_hop_inter_port: next.and_then(|h| h.inter_port).unwrap_or(0) as u32,
            self_inter_port: hop.inter_port.unwrap_or(0) as u32,
            transport: input.transport.clone(),
        };
        let proto = ProtoRule {
            id: input.rule_id,
            protocol: input.protocol.clone(),
            listen_ip: input.listen_ip.clone(),
            listen_port: input.listen_port,
            target_host: input.target_host.clone(),
            target_port: input.target_port,
            enabled: input.enabled,
            // 限速只在 entry 起作用,mid/exit 置 0 避免逐跳重复扣量。
            bandwidth_mbps: if i == 0 { input.bandwidth_mbps } else { 0 },
            tunnel: Some(tunnel),
        };
        (hop.node_id, proto)
    }).collect()
}
```

`grpc/mod.rs` 加 `pub mod tunnel_split;`。

- [ ] **Step 4: 跑测试验证通过**

Run: `cargo test -p panel-server --test tunnel_split && cargo test --workspace`
Expected: 全 PASS。

- [ ] **Step 5: Commit**

```bash
git add crates/panel-server/src/grpc/tunnel_split.rs crates/panel-server/src/grpc/mod.rs crates/panel-server/tests/tunnel_split.rs
git commit -m "feat(grpc): split_tunnel_rule splits a rule into per-hop entry/mid/exit proto Rules"
```

## Task 6: P3b 控制面文档收尾

**Files:**
- Modify: `docs/api.md`、`plan.md`

- [ ] **Step 1: docs/api.md**

- 新增 `## Tunnels`（admin only）：7 端点（GET list 含 hops_count / POST create {name, transport, node_ids} / GET :id 含 hops 详情 / PATCH :id 仅改 name / DELETE :id 被规则引用→400 / POST :id/restart / GET :id/status）；说明 transport ∈ {tcp,tls,wss}、链路 ≥2 节点全 online、inter_port 自动分配（ordinal≥1 从各节点 port_pool）、删除保护。
- rules 资源：`tunnel_id`（创建可选；给定时 node_id 必须 = 隧道入口 ordinal 0 节点；响应含 tunnel_id）。
- 节点删除保护：补「节点参与活跃隧道时拒删」。
- audit actions：加 `tunnel.create` / `tunnel.update` / `tunnel.delete` / `tunnel.restart`。
- 注明：隧道转发由 Agent 数据面执行（P3b-数据面 落地后生效）；控制面阶段隧道为「定义就绪、转发待数据面」。

- [ ] **Step 2: plan.md 附录**

「Phase 3a」小节后加：

```markdown
### Phase 3b 控制面（2026-06-10 启动）

多跳隧道控制面 —— DB(tunnels/tunnel_hops) + proto(Rule.tunnel=12 + TunnelContext + Command 6/7) + 隧道 REST CRUD + 节点删除保护扩展 + 按 hop 拆 Rule 纯函数,全部交付。

- Spec: `docs/superpowers/specs/2026-06-10-mvp-followups-design.md` §4.2/4.3/4.5
- Plan: `docs/superpowers/plans/2026-06-10-mvp-followups-phase-3.md`（P3b 控制面段,6 Task）
- migration 0006(tunnels + tunnel_hops + forward_rules.tunnel_id)。
- proto Rule.tunnel 用字段号 12(11 已被 P2 bandwidth_mbps 占)。
- inter_port 语义:ordinal≥1 的 hop 从其节点 port_pool 分配(被上一跳连入),entry inter_port=NULL。
- 隧道全链路统一 transport;创建校验 ≥2 节点全 online + 不重复。
- 删除保护:删隧道(有规则引用)/删节点(参与隧道)均 400。
- `split_tunnel_rule` 纯函数就绪(entry/mid/exit + 限速只在 entry),实际 dispatch 接入留数据面。
- **待 P3b-数据面**:Agent tunnel 模块(transport trait + TCP/TLS/WSS + TunnelTask)+ 真实下发 + 隧道证书签发下发 + status 心跳。
```

- [ ] **Step 3: 全量回归 + Commit**

```bash
cargo test --workspace
git add docs/api.md plan.md
git commit -m "docs(p3b): document tunnels control-plane API, rule.tunnel_id, node-delete protection"
```

---

## P3b 控制面 验收清单

- [ ] migration 0006 建 tunnels/tunnel_hops + forward_rules.tunnel_id → Task 1
- [ ] proto Rule.tunnel(12) + TunnelContext + TunnelRole + Command oneof 6/7 编译且可构造 → Task 2
- [ ] POST /api/tunnels（≥2 online 节点）→ 分配 inter_port（ordinal≥1）+ 事务写 hops → Task 3
- [ ] 创建拒：<2 节点 / 重复节点 / 含 offline → Task 3
- [ ] DELETE 隧道有规则引用 → 400；DELETE 节点参与隧道 → 400 → Task 3 / 4
- [ ] 规则 tunnel_id 给定时 node_id 必须 = 入口节点 → Task 4
- [ ] split_tunnel_rule 双跳/三跳拆出正确角色 + next_hop + inter_port + 限速只 entry → Task 5

## P3b-数据面 概要（待控制面落地后展开）

§4.6 + §4.9 单元 5(dispatch 接入)/6。Task 概要：
1. Agent `tunnel/transport.rs`：`TunnelTransport` trait（dial/bind/accept）+ `tcp_transport.rs`（裸 TCP，先）。store.rs RuleJson 加 tunnel context（serde default 兼容）。
2. Agent `tunnel/task.rs`：`TunnelTask` per (rule_id, role)——entry（bind listen_port → dial 下一跳）/ mid（bind inter_port → dial 下一跳）/ exit（bind inter_port → connect target）；`bridge()` 复用 P2 token bucket（仅 entry）。UDP-over-tunnel 帧（2 字节大端长度前缀，entry/exit 打包拆包）。
3. `manager.rs` RuleManager 扩展：收到带 `tunnel` 的 Rule → 起 TunnelTask 而非 TcpRelayTask/UdpRelayTask；ConfigStore 持久化 + 重启恢复。
4. `tls_transport.rs`（rustls，SNI=tunnel-<id>-hop-<n>.emorelay.internal，复用内置 CA）+ server 端按 (tunnel_id, ordinal) 签隧道 server/client cert + `Command.tunnel_credentials` 下发 + Agent 落盘 `${AGENT_DATA_DIR}/tunnels/<id>/hop-<ordinal>/`。
5. `wss_transport.rs`（tokio-tungstenite + rustls）。
6. 实际 dispatch 接入：rules.rs create 关联隧道时调 `split_tunnel_rule` + 逐 hop dispatch；隧道创建/删除时下发/吊销 TunnelCredentials；隧道 restart 重下发。
7. 隧道 status：hop 心跳聚合（最近 30s 有心跳 = up）。

## P3b 执行注意

1. 严格按 Task 1→6 顺序。T3 依赖 T1（model）；T4 依赖 T1（TunnelHop 方法）+ T3（隧道已能建）；T5 依赖 T2（proto 类型）。
2. T2 改 proto 后所有 `Rule {}` 字面量需补 `tunnel: None`（grpc/commands.rs、node-agent store.rs/relay 测试）——属机械同步。
3. 每个 Task 收尾跑测试 + `cargo test --workspace`,全绿 → commit → spawn 子代理走 `superpowers:code-reviewer`,通过才进下一 Task。
4. 控制面**不接入真实 dispatch**（T3 创建不下发、T5 只纯函数）——避免无 tunnel 能力的 Agent 错误起 relay；真实下发是数据面第一要务。
5. 不顺手改无关代码；发现 spec 与现状冲突（尤其 proto 字段号 11 vs 12、inter_port ordinal 语义）按本计划「关键设计决策」执行。



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


