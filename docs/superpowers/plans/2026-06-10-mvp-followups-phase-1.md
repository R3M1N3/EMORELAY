# Phase 1 · 体验与防呆 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 实现 Spec §2 列出的 5 项体验/防呆改进（Toast / 防删节点 / 默认 TCP+UDP / Settings Agent 端点 / 一键安装 URL），完成后 P2 可启动。

**Architecture:** 全部为「在现有 8 表 + 13 routes + 7 前端页」基础上的非破坏性增量。零 schema 改动，零 protobuf 改动。新增 1 个后端 route 模块（install.rs）、1 个前端 lib（toast.tsx）；其余为小范围 modify。

**Tech Stack:** Rust (Axum + SQLx + tower-governor for rate limit) / React 19 + TypeScript + Tailwind 4 + Vite 8 / vitest + tower::ServiceExt for tests / Dockerfile multi-arch cross-compile.

---

## 文件结构（变更面）

**Create:**
- `web/src/lib/toast.tsx` — Toast Provider + `useToast()` hook + 右上角容器
- `web/src/lib/toast.test.tsx` — vitest 测试
- `crates/panel-server/src/routes/install.rs` — `/install.sh` + `/dist/node-agent-linux-{amd64,arm64}` 端点
- `crates/panel-server/tests/api_install.rs` — install routes 集成测试
- `crates/panel-server/tests/api_nodes_delete_protection.rs` — 防删节点集成测试

**Modify:**
- `web/src/App.tsx` — 在 `AuthProvider` 外包一层 `ToastProvider`
- `web/src/pages/Rules.tsx` — RuleForm 默认 protocol = `tcp_udp`；操作 success/error 走 Toast
- `web/src/pages/Nodes.tsx` — 删除失败走 Toast；创建成功 Modal 加「复制安装命令」按钮
- `web/src/pages/NodeDetail.tsx` — 顶部加「复制安装命令」按钮
- `web/src/pages/Settings.tsx` — 加 `agent_control_endpoint` 输入框
- `web/src/lib/api.ts` — `SettingsResponse.settings` 类型补 key；新增 `system.install` helper
- `crates/panel-server/src/routes/nodes.rs::delete` — 前置查 `forward_rules` 数量
- `crates/panel-server/src/routes/system.rs` — `ALLOWED` 加 `agent_control_endpoint`；`validate_setting` 加 URL 校验分支
- `crates/panel-server/src/routes/mod.rs` — 挂 install routes（公开 + rate limit）
- `crates/panel-server/src/main.rs` — 启动时 ensure `${PANEL_DATA_DIR}/agent-dist/` 目录存在
- `crates/panel-server/src/config.rs` — 加 `panel_data_dir`、`panel_public_base_url` 字段
- `crates/panel-server/Cargo.toml` — 加 `tower-governor` dep
- `docker/panel-server.Dockerfile` — builder 阶段 cross-compile linux/amd64 + arm64 agent；runtime 阶段 COPY 二进制到 `/var/lib/emorelay/agent-dist/`
- `docker-compose.yml` — `panel-server` 加 `PANEL_DATA_DIR` + `PANEL_PUBLIC_BASE_URL` env
- `.env.example` — 加 `PANEL_DATA_DIR` + `PANEL_PUBLIC_BASE_URL`

---

## Task 1: Toast Provider + `useToast` hook

**Files:**
- Create: `web/src/lib/toast.tsx`
- Test: `web/src/lib/toast.test.tsx`

- [ ] **Step 1: 写失败测试**

```typescript
// web/src/lib/toast.test.tsx
import { describe, it, expect } from 'vitest'
import { render, screen, act } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { ToastProvider, useToast } from './toast'

function Trigger() {
  const toast = useToast()
  return (
    <>
      <button onClick={() => toast.success('saved')}>ok</button>
      <button onClick={() => toast.error('boom')}>fail</button>
    </>
  )
}

describe('Toast', () => {
  it('renders success and error toasts in fixed container', async () => {
    const user = userEvent.setup()
    render(
      <ToastProvider>
        <Trigger />
      </ToastProvider>,
    )
    await user.click(screen.getByText('ok'))
    expect(screen.getByText('saved')).toBeInTheDocument()
    await user.click(screen.getByText('fail'))
    expect(screen.getByText('boom')).toBeInTheDocument()
  })

  it('auto-dismisses after 4 seconds', async () => {
    vi.useFakeTimers()
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime })
    render(
      <ToastProvider>
        <Trigger />
      </ToastProvider>,
    )
    await user.click(screen.getByText('ok'))
    expect(screen.getByText('saved')).toBeInTheDocument()
    act(() => vi.advanceTimersByTime(4500))
    expect(screen.queryByText('saved')).toBeNull()
    vi.useRealTimers()
  })

  it('useToast throws when used outside provider', () => {
    expect(() => render(<Trigger />)).toThrow(/ToastProvider/)
  })
})
```

加 `import { vi } from 'vitest'` 到文件顶部（已有 `import { describe, it, expect } from 'vitest'` 旁补一个）。

- [ ] **Step 2: 跑测试验证失败**

Run: `cd web && npm test -- toast`
Expected: FAIL "Cannot find module './toast'" 或类似 import 错。

- [ ] **Step 3: 实现 toast.tsx**

```tsx
// web/src/lib/toast.tsx
// 简洁实现:Context + 数组状态 + setTimeout 自动清。
// 不依赖第三方库,UI 风格沿用 zinc + 半透明 + slide-in。
import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useState,
  type ReactNode,
} from 'react'

type Kind = 'success' | 'error' | 'info'

interface ToastItem {
  id: number
  kind: Kind
  message: string
}

interface ToastApi {
  success: (msg: string) => void
  error: (msg: string) => void
  info: (msg: string) => void
}

const ToastContext = createContext<ToastApi | null>(null)

export function useToast(): ToastApi {
  const api = useContext(ToastContext)
  if (!api) throw new Error('useToast must be used within <ToastProvider>')
  return api
}

const AUTO_DISMISS_MS = 4000

export function ToastProvider({ children }: { children: ReactNode }) {
  const [items, setItems] = useState<ToastItem[]>([])

  const push = useCallback((kind: Kind, message: string) => {
    const id = Date.now() + Math.random()
    setItems((prev) => [...prev, { id, kind, message }])
    setTimeout(() => {
      setItems((prev) => prev.filter((it) => it.id !== id))
    }, AUTO_DISMISS_MS)
  }, [])

  const api = useMemo<ToastApi>(
    () => ({
      success: (m) => push('success', m),
      error: (m) => push('error', m),
      info: (m) => push('info', m),
    }),
    [push],
  )

  return (
    <ToastContext.Provider value={api}>
      {children}
      <div
        className="fixed top-3 right-3 z-50 flex flex-col gap-2 max-w-sm"
        role="status"
        aria-live="polite"
      >
        {items.map((it) => (
          <div
            key={it.id}
            className={`rounded-lg border px-3 py-2 text-sm backdrop-blur shadow-lg
              animate-[slide-in_0.18s_ease-out] ${kindCls(it.kind)}`}
          >
            {it.message}
          </div>
        ))}
      </div>
    </ToastContext.Provider>
  )
}

function kindCls(k: Kind): string {
  switch (k) {
    case 'success':
      return 'border-emerald-500/40 bg-emerald-500/15 text-emerald-100'
    case 'error':
      return 'border-red-500/40 bg-red-500/15 text-red-100'
    case 'info':
      return 'border-zinc-500/40 bg-zinc-800/80 text-zinc-100'
  }
}
```

注：`animate-[slide-in_0.18s_ease-out]` 用 Tailwind 4 任意值类。需在 `web/src/index.css` 加 keyframes:

```css
/* web/src/index.css 末尾追加 */
@keyframes slide-in {
  from { transform: translateX(20px); opacity: 0; }
  to { transform: translateX(0); opacity: 1; }
}
```

- [ ] **Step 4: 跑测试验证通过**

Run: `cd web && npm test -- toast`
Expected: PASS 3 tests。

- [ ] **Step 5: Commit**

```bash
git add web/src/lib/toast.tsx web/src/lib/toast.test.tsx web/src/index.css
git commit -m "feat(web): add Toast provider + useToast hook with auto-dismiss"
```

- [ ] **Step 6: spawn 子代理 review**

按 CLAUDE.md：spawn `general-purpose` 子代理调 `superpowers:code-reviewer` 审查 Task 1 代码（含三个文件）。审查通过后才进 Task 2。

---

## Task 2: ToastProvider 接入 App + Rules 操作走 Toast

**Files:**
- Modify: `web/src/App.tsx:14-33`
- Modify: `web/src/pages/Rules.tsx:104-146`

- [ ] **Step 1: 修改 App.tsx 包裹 ToastProvider**

```tsx
// web/src/App.tsx 顶部 import 加一行
import { ToastProvider } from './lib/toast'

// 修改 App() 返回:
export default function App() {
  return (
    <ToastProvider>
      <AuthProvider>
        <BrowserRouter>
          <Routes>
            {/* ...原内容不变... */}
          </Routes>
        </BrowserRouter>
      </AuthProvider>
    </ToastProvider>
  )
}
```

- [ ] **Step 2: Rules.tsx 引入 useToast 并替换操作 catch**

```tsx
// web/src/pages/Rules.tsx 顶部 import 加:
import { useToast } from '../lib/toast'

// 在 export default function Rules() 顶部加:
const toast = useToast()

// doDelete catch 块改:
} catch (e) {
  const msg = e instanceof ApiError ? e.message : '删除失败'
  toast.error(msg)
  setConfirming(null)
} finally {
  setBusy(false)
}

// doToggle catch 块改:
} catch (e) {
  const msg = e instanceof ApiError ? e.message : '操作失败'
  toast.error(msg)
} finally {
  setActingId(null)
}

// doRestart catch 块改:
} catch (e) {
  const msg = e instanceof ApiError ? e.message : '重启失败'
  toast.error(msg)
} finally {
  setActingId(null)
}

// 删除成功后(setConfirming(null) 前)加:
toast.success('规则已删除')
// 启用/禁用成功后加:
toast.success(rule.enabled ? '已禁用' : '已启用')
// 重启成功后加:
toast.success('已下发重启')
```

加载列表的 `list.error` 路径不动（保持 inline 兜底）。

- [ ] **Step 3: 跑前端 build + lint 验证**

Run: `cd web && npm run build && npm run lint`
Expected: 全绿。

- [ ] **Step 4: Commit**

```bash
git add web/src/App.tsx web/src/pages/Rules.tsx
git commit -m "feat(web): wire ToastProvider into App; route rule op results to toasts"
```

- [ ] **Step 5: spawn 子代理 review**

---

## Task 3: 创建规则默认 TCP+UDP

**Files:**
- Modify: `web/src/pages/Rules.tsx:457`（RuleForm 初值）

- [ ] **Step 1: 改初值**

```tsx
// Rules.tsx::RuleForm 内 useState 初值改:
const [form, setForm] = useState<RuleFormState>({
  node_id: initial ? String(initial.node_id) : nodeList[0] ? String(nodeList[0].id) : '',
  name: initial?.name ?? '',
  // 创建模式默认 TCP+UDP;编辑模式沿用旧值。
  protocol: initial?.protocol ?? 'tcp_udp',
  // ...其余字段不变...
})
```

- [ ] **Step 2: 跑前端 build + 手测**

Run: `cd web && npm run build`
手测：`npm run dev` → 创建规则表单 → 协议下拉默认 TCP+UDP。

- [ ] **Step 3: Commit**

```bash
git add web/src/pages/Rules.tsx
git commit -m "feat(web): default new rule protocol to tcp_udp"
```

- [ ] **Step 4: spawn 子代理 review**

---

## Task 4: 防删节点（后端）

**Files:**
- Modify: `crates/panel-server/src/routes/nodes.rs::delete` (line 300-324)
- Create: `crates/panel-server/tests/api_nodes_delete_protection.rs`

- [ ] **Step 1: 写失败测试**

```rust
// crates/panel-server/tests/api_nodes_delete_protection.rs
mod common;

use axum::http::{Method, StatusCode};
use common::{auth_req, make_app, send};
use serde_json::json;

#[tokio::test]
async fn delete_node_with_active_rules_returns_400() {
    let app = common::make_app().await.unwrap();
    let t = &app.admin_token;

    // 1. 创建节点
    let req = auth_req(
        Method::POST,
        "/api/nodes",
        t,
        Some(json!({ "name": "n-with-rule" })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    let node_id = body["node"]["id"].as_i64().unwrap();

    // 2. 在节点上创建规则
    let req = auth_req(
        Method::POST,
        "/api/rules",
        t,
        Some(json!({
            "node_id": node_id,
            "name": "r1",
            "protocol": "tcp",
            "listen_port": 20000,
            "target_host": "1.2.3.4",
            "target_port": 443,
        })),
    )
    .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);

    // 3. 删节点 → 400 + 错误消息含规则信息
    let req = auth_req(
        Method::DELETE,
        &format!("/api/nodes/{node_id}"),
        t,
        None,
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let msg = body["message"].as_str().unwrap();
    assert!(msg.contains("rule") || msg.contains("规则"));
    assert!(msg.contains("r1"));
}

#[tokio::test]
async fn delete_node_without_rules_succeeds() {
    let app = common::make_app().await.unwrap();
    let t = &app.admin_token;
    let req = auth_req(
        Method::POST,
        "/api/nodes",
        t,
        Some(json!({ "name": "n-empty" })),
    )
    .unwrap();
    let (_, body) = send(app.app.clone(), req).await.unwrap();
    let node_id = body["node"]["id"].as_i64().unwrap();
    let req = auth_req(
        Method::DELETE,
        &format!("/api/nodes/{node_id}"),
        t,
        None,
    )
    .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn delete_node_after_rule_soft_deleted_succeeds() {
    let app = common::make_app().await.unwrap();
    let t = &app.admin_token;
    let req = auth_req(
        Method::POST,
        "/api/nodes",
        t,
        Some(json!({ "name": "n-revivable" })),
    )
    .unwrap();
    let (_, body) = send(app.app.clone(), req).await.unwrap();
    let node_id = body["node"]["id"].as_i64().unwrap();

    let req = auth_req(
        Method::POST,
        "/api/rules",
        t,
        Some(json!({
            "node_id": node_id,
            "name": "r-soft",
            "protocol": "tcp",
            "listen_port": 21000,
            "target_host": "1.2.3.4",
            "target_port": 443,
        })),
    )
    .unwrap();
    let (_, body) = send(app.app.clone(), req).await.unwrap();
    let rule_id = body["id"].as_i64().unwrap();

    // 软删规则
    let req = auth_req(Method::DELETE, &format!("/api/rules/{rule_id}"), t, None).unwrap();
    send(app.app.clone(), req).await.unwrap();

    // 再删节点 → OK
    let req = auth_req(Method::DELETE, &format!("/api/nodes/{node_id}"), t, None).unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
}
```

- [ ] **Step 2: 跑测试验证失败**

Run: `cargo test -p panel-server --test api_nodes_delete_protection`
Expected: 1 个 PASS（without_rules），2 个 FAIL（with_rules / after_soft_delete 行为符合旧实现因此 with_rules 测试 fail）。

- [ ] **Step 3: 修改 routes/nodes.rs::delete 加前置检查**

```rust
// crates/panel-server/src/routes/nodes.rs::delete 内,
// `let rows = Node::soft_delete(&state.pool, id).await?;` 之前加:

#[derive(sqlx::FromRow)]
struct ConflictRule {
    id: i64,
    name: String,
}

let conflicts: Vec<ConflictRule> = sqlx::query_as(
    "SELECT id, name FROM forward_rules \
     WHERE node_id = ? AND deleted_at IS NULL \
     ORDER BY id LIMIT 4",
)
.bind(id)
.fetch_all(&state.pool)
.await?;

if !conflicts.is_empty() {
    let shown = conflicts
        .iter()
        .take(3)
        .map(|r| format!("#{}({})", r.id, r.name))
        .collect::<Vec<_>>()
        .join(", ");
    let suffix = if conflicts.len() > 3 {
        format!("...还有 {} 条", conflicts.len() - 3)
    } else {
        String::new()
    };
    return Err(ApiError::BadRequest(format!(
        "节点上仍有活跃规则,请先删除: {shown}{suffix}"
    )));
}
```

`ConflictRule` 局部定义即可，不必加到 models/。

- [ ] **Step 4: 跑测试验证通过**

Run: `cargo test -p panel-server --test api_nodes_delete_protection`
Expected: 3 PASS。

- [ ] **Step 5: 全套回归**

Run: `cargo test --workspace`
Expected: 全绿（含原 `api_nodes::node_full_crud_cycle`，该测试不创建规则，删节点仍然 OK）。

- [ ] **Step 6: Commit**

```bash
git add crates/panel-server/src/routes/nodes.rs crates/panel-server/tests/api_nodes_delete_protection.rs
git commit -m "feat(server): reject DELETE /api/nodes/:id when active rules exist"
```

- [ ] **Step 7: spawn 子代理 review**

---

## Task 5: 防删节点（前端 Toast）

**Files:**
- Modify: `web/src/pages/Nodes.tsx:63-76`（doDelete catch）

- [ ] **Step 1: 改 doDelete 失败路径走 Toast + 加成功 Toast**

```tsx
// Nodes.tsx 顶部 import 加:
import { useToast } from '../lib/toast'

// export default function Nodes() 顶部加:
const toast = useToast()

// doDelete 改:
async function doDelete(node: NodeView) {
  setBusy(true)
  try {
    await nodes.del(node.id)
    setConfirming(null)
    toast.success(`节点 ${node.name} 已删除`)
    await reload()
  } catch (e) {
    const msg = e instanceof ApiError ? e.message : '删除失败'
    toast.error(msg)
    setConfirming(null)
  } finally {
    setBusy(false)
  }
}
```

不再把 `setList((s) => ({ ...s, error: msg }))` 写 inline error，全走 toast。

- [ ] **Step 2: 跑前端 build**

Run: `cd web && npm run build`
Expected: 绿。

- [ ] **Step 3: 手测**

`npm run dev` → 创建节点 + 节点上挂规则 → 删节点 → 右上角红色 Toast 列出冲突规则。

- [ ] **Step 4: Commit**

```bash
git add web/src/pages/Nodes.tsx
git commit -m "feat(web): show node-delete failures as toast with conflict rules"
```

- [ ] **Step 5: spawn 子代理 review**

---

## Task 6: Settings 后端加 `agent_control_endpoint` key

**Files:**
- Modify: `crates/panel-server/src/routes/system.rs:227-326`
- Modify: `migrations/0001_init.sql:171-175`（追加 INSERT，仅在新库；现有库要单独迁移）
- Test: `crates/panel-server/tests/api_system.rs`（追加用例）

注：`0001_init.sql` 修改对**新部署**生效；对已运行的 DB 需要应用层在启动时 `INSERT OR IGNORE`。两手都要：

- [ ] **Step 1: 在 migrations/0001_init.sql 末尾 INSERT 加一行**

```sql
-- 0001_init.sql 末尾的 INSERT 改成:
INSERT INTO system_settings (key, value) VALUES
    ('reserved_ports',               '[22, 80, 443, 3306, 5432]'),
    ('default_traffic_limit_bytes',  ''),
    ('default_bandwidth_limit_mbps', ''),
    ('stats_retention_days',         '30'),
    ('agent_control_endpoint',       '');
```

这是 plan.md 第一节明确允许的 — MVP 期间 0001 可直接修改。

- [ ] **Step 2: bootstrap.rs 内新增 `seed_default_settings`（已存在 DB 升级路径）**

`bootstrap.rs` 现仅含 `ensure_admin_user`。新增一个 `pub async fn seed_default_settings`，由 `main.rs` 在 migrate + `ensure_admin_user` 之后调用一次：

```rust
// crates/panel-server/src/bootstrap.rs 末尾追加:
use sqlx::SqlitePool;

/// 对历史 DB(在新 key 加入之前迁过)兜底插入默认设置。
/// 不覆盖管理员已设值,使用 INSERT OR IGNORE。
pub async fn seed_default_settings(pool: &SqlitePool) -> anyhow::Result<()> {
    let defaults: &[(&str, &str)] = &[
        ("agent_control_endpoint", ""),
    ];
    for (k, v) in defaults {
        sqlx::query("INSERT OR IGNORE INTO system_settings (key, value) VALUES (?, ?)")
            .bind(k)
            .bind(v)
            .execute(pool)
            .await
            .with_context(|| format!("seed default setting {k}"))?;
    }
    Ok(())
}
```

文件顶部 `use anyhow::Context;` 已存在;不需要加。

`crates/panel-server/src/main.rs` 中,`ensure_admin_user(&pool).await?;` 之后追加一行:

```rust
bootstrap::seed_default_settings(&pool).await?;
```

- [ ] **Step 3: routes/system.rs::ALLOWED 加 key + validate**

```rust
// routes/system.rs::update_settings 内 ALLOWED:
const ALLOWED: &[&str] = &[
    "reserved_ports",
    "default_traffic_limit_bytes",
    "default_bandwidth_limit_mbps",
    "stats_retention_days",
    "agent_control_endpoint",  // P1 新增
];

// validate_setting 新增分支:
"agent_control_endpoint" => {
    if value.is_empty() {
        return Ok(()); // 空 = 未配置
    }
    // 必须 http(s)://host[:port][/path]
    let url = url::Url::parse(value).map_err(|e| {
        ApiError::BadRequest(format!("agent_control_endpoint 必须是合法 URL: {e}"))
    })?;
    match url.scheme() {
        "http" | "https" => Ok(()),
        s => Err(ApiError::BadRequest(format!(
            "agent_control_endpoint scheme 必须是 http/https,得到 {s}"
        ))),
    }
}
```

- [ ] **Step 4: Cargo.toml 加 `url` 依赖**

```toml
# crates/panel-server/Cargo.toml 的 [dependencies] 段:
url = "2"
```

- [ ] **Step 5: 写测试**

```rust
// crates/panel-server/tests/api_system.rs 末尾追加(沿用现有 mod common 模式):
#[tokio::test]
async fn agent_control_endpoint_accepts_https() {
    let app = common::make_app().await.unwrap();
    let req = common::auth_req(
        axum::http::Method::PATCH,
        "/api/system/settings",
        &app.admin_token,
        Some(serde_json::json!({
            "settings": { "agent_control_endpoint": "https://relay.example.com:50051" }
        })),
    )
    .unwrap();
    let (status, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, axum::http::StatusCode::OK);
}

#[tokio::test]
async fn agent_control_endpoint_rejects_bad_scheme() {
    let app = common::make_app().await.unwrap();
    let req = common::auth_req(
        axum::http::Method::PATCH,
        "/api/system/settings",
        &app.admin_token,
        Some(serde_json::json!({
            "settings": { "agent_control_endpoint": "ftp://x.com" }
        })),
    )
    .unwrap();
    let (status, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn agent_control_endpoint_empty_accepted() {
    let app = common::make_app().await.unwrap();
    let req = common::auth_req(
        axum::http::Method::PATCH,
        "/api/system/settings",
        &app.admin_token,
        Some(serde_json::json!({
            "settings": { "agent_control_endpoint": "" }
        })),
    )
    .unwrap();
    let (status, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, axum::http::StatusCode::OK);
}
```

- [ ] **Step 6: 跑测试**

Run: `cargo test -p panel-server --test api_system`
Expected: 全绿，含 3 个新测试。

- [ ] **Step 7: Commit**

```bash
git add crates/panel-server/src/routes/system.rs \
        crates/panel-server/src/bootstrap.rs \
        crates/panel-server/Cargo.toml \
        crates/panel-server/Cargo.lock \
        migrations/0001_init.sql \
        crates/panel-server/tests/api_system.rs
git commit -m "feat(server): add agent_control_endpoint setting with URL validation"
```

- [ ] **Step 8: spawn 子代理 review**

---

## Task 7: Settings 前端加 `agent_control_endpoint` 字段

**Files:**
- Modify: `web/src/pages/Settings.tsx:11-33, 95-135, 158-210`
- Modify: `web/src/lib/api.ts`（无新类型；`Record<string, string>` 已通用）

- [ ] **Step 1: SettingsFormState 加字段**

```tsx
// Settings.tsx
interface SettingsFormState {
  reserved_ports: string
  default_traffic_limit_bytes: string
  default_bandwidth_limit_mbps: string
  stats_retention_days: string
  agent_control_endpoint: string  // 新增
}

const EMPTY_FORM: SettingsFormState = {
  reserved_ports: '',
  default_traffic_limit_bytes: '',
  default_bandwidth_limit_mbps: '',
  stats_retention_days: '',
  agent_control_endpoint: '',  // 新增
}

// useEffect 内初始化加:
form: {
  reserved_ports: initial.reserved_ports ?? '',
  default_traffic_limit_bytes: initial.default_traffic_limit_bytes ?? '',
  default_bandwidth_limit_mbps: initial.default_bandwidth_limit_mbps ?? '',
  stats_retention_days: initial.stats_retention_days ?? '',
  agent_control_endpoint: initial.agent_control_endpoint ?? '',  // 新增
},

// onSubmit 内 keys 数组加:
const keys: (keyof SettingsFormState)[] = [
  'reserved_ports',
  'default_traffic_limit_bytes',
  'default_bandwidth_limit_mbps',
  'stats_retention_days',
  'agent_control_endpoint',  // 新增
]

// resp 写回 form 时加同款:
agent_control_endpoint: resp.settings.agent_control_endpoint ?? '',
```

- [ ] **Step 2: 表单新增输入块（放在 reserved_ports 块上面，作为第一项）**

```tsx
// 在 form 顶部 <textarea reserved_ports> 之前插入:
<div>
  <label className={fieldLabelCls}>Agent 上报端点</label>
  <input
    type="text"
    value={state.form.agent_control_endpoint}
    onChange={(e) => set('agent_control_endpoint', e.target.value)}
    className={fieldInputCls}
    placeholder="https://relay.example.com:50051"
  />
  <p className="text-[11px] text-zinc-500 mt-1">
    Agent 默认 gRPC 连入地址。新建节点的「安装命令」会嵌入这个值;
    生产建议用 https。留空表示未配置(节点详情页的安装命令按钮会禁用)。
  </p>
</div>
```

- [ ] **Step 3: 用 Toast 替换 saveError/savedAt 提示**

```tsx
// 顶部 import:
import { useToast } from '../lib/toast'

// 组件顶部:
const toast = useToast()

// onSubmit 内成功路径加 toast.success:
} catch (e) {
  const msg = e instanceof ApiError ? e.message : '保存失败'
  setState((p) => ({ ...p, saving: false, saveError: msg }))
  toast.error(msg)  // 新增
}
// 成功路径在 setState 后加:
toast.success('设置已保存')

// inline 提示块 state.saveError / state.savedAt 仍保留(展示在表单内的视觉锚点),
// 双轨期不冲突;P1 完成后可全部删掉。
```

- [ ] **Step 4: 跑前端 build + lint**

Run: `cd web && npm run build && npm run lint`
Expected: 绿。

- [ ] **Step 5: Commit**

```bash
git add web/src/pages/Settings.tsx
git commit -m "feat(web): add agent_control_endpoint setting field; route save results to toast"
```

- [ ] **Step 6: spawn 子代理 review**

---

## Task 8: `install.rs` 端点（install.sh + 二进制 serve）

**Files:**
- Create: `crates/panel-server/src/routes/install.rs`
- Modify: `crates/panel-server/src/routes/mod.rs`（加 use + 挂路由）
- Modify: `crates/panel-server/src/config.rs`（加 panel_data_dir、panel_public_base_url）
- Modify: `crates/panel-server/src/main.rs`（启动 ensure dir）
- Modify: `.env.example`
- Modify: `docker-compose.yml`
- Test: `crates/panel-server/tests/api_install.rs`

- [ ] **Step 1: config.rs 加字段**

```rust
// crates/panel-server/src/config.rs Config 结构内加:
/// Agent 二进制 + install.sh 存放根目录。默认 ${PANEL_DATA_DIR}/agent-dist 之下。
pub panel_data_dir: String,
/// 面板对外可访问的 base URL,用于生成安装命令(install.sh 自身从这里拉二进制)。
/// 留空 → 节点详情页隐藏「复制安装命令」按钮。
pub panel_public_base_url: Option<String>,

// from_env() 末尾加:
panel_data_dir: env::var("PANEL_DATA_DIR")
    .unwrap_or_else(|_| "/data".into()),
panel_public_base_url: env::var("PANEL_PUBLIC_BASE_URL")
    .ok()
    .filter(|s| !s.is_empty()),
```

- [ ] **Step 2: 写失败测试**

```rust
// crates/panel-server/tests/api_install.rs
mod common;

use axum::body::to_bytes;
use axum::http::{Method, Request, StatusCode};
use common::make_app;
use tower::ServiceExt;

#[tokio::test]
async fn install_sh_returns_bash_script_with_node_id() {
    let app = make_app().await.unwrap();
    let req = Request::builder()
        .method(Method::GET)
        .uri("/install.sh?node=42")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(ct.starts_with("text/x-shellscript") || ct.starts_with("text/plain"));
    let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let body = std::str::from_utf8(&bytes).unwrap();
    assert!(body.starts_with("#!/usr/bin/env bash") || body.starts_with("#!/bin/bash"));
    assert!(body.contains("AGENT_NODE_ID=42"));
    assert!(body.contains("--token="));
}

#[tokio::test]
async fn install_sh_missing_node_returns_400() {
    let app = make_app().await.unwrap();
    let req = Request::builder()
        .method(Method::GET)
        .uri("/install.sh")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn install_sh_uses_endpoint_from_settings() {
    let app = make_app().await.unwrap();
    // 先设端点
    let req = common::auth_req(
        Method::PATCH,
        "/api/system/settings",
        &app.admin_token,
        Some(serde_json::json!({
            "settings": { "agent_control_endpoint": "https://relay.example.com:50051" }
        })),
    )
    .unwrap();
    common::send(app.app.clone(), req).await.unwrap();

    // 拉 install.sh
    let req = Request::builder()
        .method(Method::GET)
        .uri("/install.sh?node=7")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let body = std::str::from_utf8(&bytes).unwrap();
    assert!(body.contains("AGENT_CONTROL_ENDPOINT=https://relay.example.com:50051"));
}

#[tokio::test]
async fn dist_unknown_arch_returns_404() {
    let app = make_app().await.unwrap();
    let req = Request::builder()
        .method(Method::GET)
        .uri("/dist/node-agent-linux-mips")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
```

- [ ] **Step 3: 跑测试验证失败**

Run: `cargo test -p panel-server --test api_install`
Expected: 全部 4 个 FAIL（端点不存在）。

- [ ] **Step 4: 实现 install.rs**

```rust
// crates/panel-server/src/routes/install.rs
use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::path::PathBuf;

use crate::{error::ApiError, state::AppState};

#[derive(Deserialize)]
pub struct InstallScriptQuery {
    pub node: Option<i64>,
}

/// 返回参数化 bash 安装脚本。
/// 无需鉴权;token 通过 `--token=` 参数在使用者复制安装命令时一次性带入。
/// 配 rate limit(由 routes/mod.rs 挂载时的 governor 中间件提供)。
pub async fn install_sh(
    State(state): State<AppState>,
    Query(q): Query<InstallScriptQuery>,
) -> Result<Response, ApiError> {
    let node_id = q
        .node
        .ok_or_else(|| ApiError::BadRequest("missing ?node=<id>".into()))?;

    // 从 system_settings 拉 agent_control_endpoint。
    let endpoint: Option<(String,)> = sqlx::query_as(
        "SELECT value FROM system_settings WHERE key = 'agent_control_endpoint'",
    )
    .fetch_optional(&state.pool)
    .await?;
    let endpoint = endpoint.map(|(v,)| v).unwrap_or_default();

    // base URL 用于二进制下载;留空时脚本里报错。
    let base = state
        .config
        .panel_public_base_url
        .clone()
        .unwrap_or_else(|| "PANEL_PUBLIC_BASE_URL_NOT_SET".into());

    let script = render_install_sh(node_id, &endpoint, &base);

    let body = axum::body::Body::from(script);
    let resp = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/x-shellscript; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .body(body)
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("response build: {e}")))?;
    Ok(resp)
}

fn render_install_sh(node_id: i64, control_endpoint: &str, base_url: &str) -> String {
    // 注意:base_url 在 dev 期可能为占位;脚本内显式检查并报错,避免静默 curl 401。
    format!(
        r##"#!/usr/bin/env bash
# EMORELAY node-agent 一键安装脚本
# 生成于:本脚本由 panel-server `/install.sh` 端点动态渲染。
set -euo pipefail

TOKEN=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --token=*) TOKEN="${{1#*=}}"; shift ;;
    --token)   TOKEN="$2"; shift 2 ;;
    *) echo "unknown arg: $1" >&2; exit 64 ;;
  esac
done
if [[ -z "${{TOKEN:-}}" ]]; then
  echo "missing --token=<agent_token>" >&2
  exit 64
fi

BASE_URL="{base_url}"
if [[ "$BASE_URL" == "PANEL_PUBLIC_BASE_URL_NOT_SET" ]]; then
  echo "panel-server is missing PANEL_PUBLIC_BASE_URL env; cannot bootstrap agent." >&2
  exit 78
fi

ARCH=""
case "$(uname -m)" in
  x86_64|amd64)  ARCH=amd64 ;;
  aarch64|arm64) ARCH=arm64 ;;
  *) echo "unsupported arch: $(uname -m)" >&2; exit 70 ;;
esac

# 1. 下载二进制
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
echo "downloading agent binary (linux-$ARCH)..."
curl -fsSL "${{BASE_URL}}/dist/node-agent-linux-${{ARCH}}" -o "$TMP/node-agent"
install -m 0755 "$TMP/node-agent" /usr/local/bin/emorelay-agent

# 2. 写 env 文件
install -d -m 0755 /etc/emorelay
cat > /etc/emorelay/agent.env <<EOF
AGENT_NODE_ID={node_id}
AGENT_TOKEN=$TOKEN
AGENT_CONTROL_ENDPOINT={control_endpoint}
AGENT_STATE_PATH=/var/lib/emorelay/agent-state.json
EOF
chmod 0600 /etc/emorelay/agent.env
install -d -m 0755 /var/lib/emorelay

# 3. 写 systemd unit
cat > /etc/systemd/system/emorelay-agent.service <<'EOF'
[Unit]
Description=EMORELAY node agent
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
EnvironmentFile=/etc/emorelay/agent.env
ExecStart=/usr/local/bin/emorelay-agent
Restart=on-failure
RestartSec=5
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/var/lib/emorelay

[Install]
WantedBy=multi-user.target
EOF

# 4. 启动
systemctl daemon-reload
systemctl enable --now emorelay-agent
sleep 1
systemctl status emorelay-agent --no-pager || true
echo
echo "done. agent connecting to {control_endpoint} for node #{node_id}"
"##,
        base_url = base_url,
        control_endpoint = control_endpoint,
        node_id = node_id,
    )
}

/// 提供预编译 agent 二进制下载。仅 amd64 / arm64 两个 target。
pub async fn dist_binary(
    State(state): State<AppState>,
    Path(filename): Path<String>,
) -> Result<Response, ApiError> {
    // 严格白名单,防 path traversal。
    let allowed = matches!(
        filename.as_str(),
        "node-agent-linux-amd64" | "node-agent-linux-arm64"
    );
    if !allowed {
        return Err(ApiError::NotFound);
    }
    let mut path = PathBuf::from(&state.config.panel_data_dir);
    path.push("agent-dist");
    path.push(&filename);
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| ApiError::NotFound)?;
    let resp = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        )
        .body(axum::body::Body::from(bytes))
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("response build: {e}")))?;
    Ok(resp)
}
```

注：`ApiError::Internal` 的签名为 `Internal(#[from] anyhow::Error)`（见 `src/error.rs`），因此必须包成 `anyhow::anyhow!(...)`。文件顶部加 `use crate::error::ApiError;` 与 `use crate::state::AppState;`。

- [ ] **Step 5: routes/mod.rs 挂路由**

```rust
// crates/panel-server/src/routes/mod.rs:
pub mod install;
// ...

pub fn router(state: AppState) -> Router {
    Router::new()
        // ...现有 /api/* 路由不变...
        // 公开 install/dist:
        .route("/install.sh", get(install::install_sh))
        .route("/dist/{filename}", get(install::dist_binary))
        .with_state(state)
}
```

注：rate limit 在 Task 9 加。本 Task 先实现核心逻辑通过测试。

- [ ] **Step 6: main.rs 启动确保 agent-dist 目录存在**

```rust
// crates/panel-server/src/main.rs 启动逻辑里(数据库 connect 之后):
let dist_dir = std::path::PathBuf::from(&config.panel_data_dir).join("agent-dist");
if let Err(e) = tokio::fs::create_dir_all(&dist_dir).await {
    tracing::warn!(error = ?e, path = ?dist_dir, "failed to ensure agent-dist dir");
}
```

- [ ] **Step 7: .env.example 加变量**

```bash
# 末尾追加:
# panel-server 数据目录(SQLite 文件 + agent 二进制 + 后续 TLS 材料)。
PANEL_DATA_DIR=/data
# 面板对外可访问的 base URL,用于生成节点安装命令的下载基址。
# 例:https://relay.example.com  留空 → 节点详情隐藏「复制安装命令」按钮。
PANEL_PUBLIC_BASE_URL=
```

- [ ] **Step 8: docker-compose.yml 加 env**

```yaml
# services.panel-server.environment 段追加:
PANEL_DATA_DIR: /data
PANEL_PUBLIC_BASE_URL: ${PANEL_PUBLIC_BASE_URL:-}
```

- [ ] **Step 9: 跑测试验证通过**

Run: `cargo test -p panel-server --test api_install`
Expected: 4 PASS。

- [ ] **Step 10: 全套回归**

Run: `cargo test --workspace`
Expected: 全绿。

- [ ] **Step 11: Commit**

```bash
git add crates/panel-server/src/routes/install.rs \
        crates/panel-server/src/routes/mod.rs \
        crates/panel-server/src/main.rs \
        crates/panel-server/src/config.rs \
        crates/panel-server/tests/api_install.rs \
        .env.example docker-compose.yml
git commit -m "feat(server): add /install.sh and /dist/<binary> endpoints"
```

- [ ] **Step 12: spawn 子代理 review**

---

## Task 9: Install endpoints 加 rate limit

**Files:**
- Modify: `crates/panel-server/Cargo.toml`（加 `tower_governor`）
- Modify: `crates/panel-server/src/routes/mod.rs`
- Modify: `crates/panel-server/tests/api_install.rs`（新增 burst 测试）

- [ ] **Step 1: Cargo.toml 加依赖**

```toml
[dependencies]
tower_governor = "0.4"
```

- [ ] **Step 2: routes/mod.rs 用 GovernorLayer 包 install/dist**

```rust
// routes/mod.rs:
use tower_governor::{governor::GovernorConfigBuilder, GovernorLayer};

pub fn router(state: AppState) -> Router {
    let install_governor = GovernorConfigBuilder::default()
        .per_second(1)          // 1 req/s 平均
        .burst_size(60)         // 60 req 突发(60 req/min 相当)
        .finish()
        .expect("governor config");

    let install_routes = Router::new()
        .route("/install.sh", get(install::install_sh))
        .route("/dist/{filename}", get(install::dist_binary))
        .layer(GovernorLayer {
            config: install_governor.into(),
        });

    Router::new()
        // ...所有 /api/* 不变...
        .merge(install_routes)
        .with_state(state)
}
```

注：`GovernorLayer` 与 `with_state` 顺序有讲究；如果编译错调整 `.with_state` 在 `.merge` 之前或用 `Router::new().merge(install_routes).with_state(state)` 形式。

- [ ] **Step 3: 加 burst 测试**

```rust
// tests/api_install.rs 末尾追加:
#[tokio::test]
async fn install_sh_rate_limited_after_burst() {
    let app = make_app().await.unwrap();
    // 60 次 burst 应该都能过。
    for _ in 0..60 {
        let req = Request::builder()
            .method(Method::GET)
            .uri("/install.sh?node=1")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
    // 第 61 次应该 429.
    let req = Request::builder()
        .method(Method::GET)
        .uri("/install.sh?node=1")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
}
```

- [ ] **Step 4: 跑测试**

Run: `cargo test -p panel-server --test api_install`
Expected: 5 PASS。

注：tower_governor 默认按 PeerIp 限流；测试用 oneshot 走的是同 mock socket，效果等同。如测试 flaky，改成显式 `KeyExtractor` 用常量 key。

- [ ] **Step 5: Commit**

```bash
git add crates/panel-server/src/routes/mod.rs \
        crates/panel-server/Cargo.toml \
        crates/panel-server/Cargo.lock \
        crates/panel-server/tests/api_install.rs
git commit -m "feat(server): apply rate limit to /install.sh and /dist/* endpoints"
```

- [ ] **Step 6: spawn 子代理 review**

---

## Task 10: Dockerfile cross-compile + docker-compose 挂载

**Files:**
- Modify: `docker/panel-server.Dockerfile`
- Modify: `docker-compose.yml`

- [ ] **Step 1: Dockerfile builder 阶段加 cross-compile**

```dockerfile
# docker/panel-server.Dockerfile builder 阶段在 cargo build 之前加:

# musl 工具链 + 两个 linux target,用于编静态 agent 二进制。
RUN apt-get update && apt-get install -y --no-install-recommends \
    musl-tools gcc-aarch64-linux-gnu \
    && rm -rf /var/lib/apt/lists/*

RUN rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl

ENV CC_aarch64_unknown_linux_musl=aarch64-linux-gnu-gcc \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=aarch64-linux-gnu-gcc

RUN cargo build --release -p panel-server
RUN cargo build --release -p node-agent --target x86_64-unknown-linux-musl
RUN cargo build --release -p node-agent --target aarch64-unknown-linux-musl
```

runtime 阶段加 COPY:

```dockerfile
# runtime 阶段 COPY --from=builder 段后追加:
COPY --from=builder \
  /build/target/x86_64-unknown-linux-musl/release/node-agent \
  /var/lib/emorelay/agent-dist/node-agent-linux-amd64
COPY --from=builder \
  /build/target/aarch64-unknown-linux-musl/release/node-agent \
  /var/lib/emorelay/agent-dist/node-agent-linux-arm64

RUN chmod 0755 /var/lib/emorelay/agent-dist/node-agent-linux-amd64 \
               /var/lib/emorelay/agent-dist/node-agent-linux-arm64 \
    && chown -R emorelay:emorelay /var/lib/emorelay
```

调整 `EXPOSE`（不变）和 `ENTRYPOINT`（不变）。

- [ ] **Step 2: docker-compose.yml 调整**

`PANEL_DATA_DIR` 在镜像内已默认 `/data`（sqlite volume），但 `agent-dist/` 在 `/var/lib/emorelay/agent-dist/`。两个分离。

修正 plan：把 `PANEL_DATA_DIR` 默认改为 `/var/lib/emorelay`，sqlite 文件路径同步移到 `/var/lib/emorelay/emorelay.db`：

```yaml
# docker-compose.yml services.panel-server.environment:
PANEL_DATABASE_URL: sqlite:///var/lib/emorelay/emorelay.db
PANEL_DATA_DIR: /var/lib/emorelay
PANEL_PUBLIC_BASE_URL: ${PANEL_PUBLIC_BASE_URL:-}

# volumes:
- sqlite-data:/var/lib/emorelay

# 镜像内 Dockerfile 的 runtime 阶段同步改:
RUN mkdir -p /var/lib/emorelay && chown emorelay:emorelay /var/lib/emorelay
```

Dockerfile 内的 `/data` 全部替换为 `/var/lib/emorelay`（含 volume mount 点）。

- [ ] **Step 3: 本地 build 验证**

Run:
```bash
docker compose build panel-server
```
Expected: build 成功；最终镜像内 `/var/lib/emorelay/agent-dist/` 含两个二进制（手测 `docker run --rm emorelay/panel-server:dev ls /var/lib/emorelay/agent-dist`）。

如本机磁盘紧张可只验证 builder 阶段 cross-compile 成功：
```bash
docker compose build --no-cache panel-server 2>&1 | tail -50
```

- [ ] **Step 4: e2e smoke**

```bash
docker compose up -d --build
curl -fsS http://localhost:8080/install.sh?node=1 | head -20
# 应输出 bash 脚本头(此时 PANEL_PUBLIC_BASE_URL 未设 → 脚本含 PANEL_PUBLIC_BASE_URL_NOT_SET sentinel)
```

- [ ] **Step 5: Commit**

```bash
git add docker/panel-server.Dockerfile docker-compose.yml
git commit -m "feat(docker): cross-compile node-agent for linux amd64+arm64 into image"
```

- [ ] **Step 6: spawn 子代理 review**

---

## Task 11: 节点 Modal「复制安装命令」按钮

**Files:**
- Modify: `web/src/pages/Nodes.tsx`（Token Modal 加按钮）
- Modify: `web/src/pages/NodeDetail.tsx`（顶部加按钮 + 二次发卡入口）
- Modify: `web/src/lib/api.ts`（导出 helper）

- [ ] **Step 1: api.ts 加 install helper**

前端无法直接读 `PANEL_PUBLIC_BASE_URL` env（不暴露给 web）。用 `window.location.origin` 作 base URL — 在生产由 Caddy/Nginx 反代到 panel-server 时，origin 即等于 `PANEL_PUBLIC_BASE_URL`。

```ts
// web/src/lib/api.ts 文件末尾追加:

/**
 * 生成节点安装命令字符串(用户复制走)。
 * base URL 取自 window.location.origin —— 生产期需用反代将面板对外 origin 指向 panel-server,
 * 否则脚本里 curl 不到 /install.sh 与 /dist/*。
 * token 一次性,UI 仅在创建节点 / 后续轮换凭据 Modal 内可调用。
 */
export function renderInstallCommand(opts: { nodeId: number; token: string }): string {
  const base = window.location.origin
  return `curl -fsSL ${base}/install.sh?node=${opts.nodeId} | sudo bash -s -- --token=${opts.token}`
}
```

`agent_control_endpoint` 是否已配的判断由调用方做（Step 2 在 Modal 内）。

- [ ] **Step 2: Nodes.tsx Token Modal 加按钮**

```tsx
// Nodes.tsx 顶部 import:
// 现有: import { useEffect, useState, type FormEvent } from 'react'
// 新增: 在已有 api import 行追加 renderInstallCommand, system
import {
  ApiError,
  formatBytes,
  nodes,
  renderInstallCommand,
  shortTime,
  system,
  type CreateNodeRequest,
  type NodeView,
  type UpdateNodeRequest,
} from '../lib/api'

// 在 export default function Nodes() 顶部 useState 后追加:
const [settings, setSettings] = useState<Record<string, string>>({})
useEffect(() => {
  system.getSettings().then((r) => setSettings(r.settings)).catch(() => {})
}, [])

// token 状态扩展存 id —— 现有签名:
//   useState<{ token: string; name: string } | null>(null)
// 改成:
const [token, setToken] = useState<{ token: string; name: string; id: number } | null>(null)

// NodeForm 的 onSuccess callback 同步改签名;调用处:
//   await onSuccess({ token: r.agent_token, name: r.node.name })  // 旧
//   await onSuccess({ token: r.agent_token, name: r.node.name, id: r.node.id })  // 新
// onSuccess 类型同步:
//   onSuccess: (createdToken: { token: string; name: string; id: number } | null) => void | Promise<void>

// Token Modal 内 token 显示框之后、"我已保存" 按钮之前插入:
{(() => {
  const endpoint = settings.agent_control_endpoint || ''
  if (!endpoint) {
    return (
      <p className="mt-3 text-[11px] text-amber-300">
        提示:请先到「设置」配 Agent 上报端点,再回到这里复制安装命令。
      </p>
    )
  }
  const cmd = renderInstallCommand({ nodeId: token.id, token: token.token })
  return (
    <div className="mt-3">
      <div className="text-[11px] text-zinc-500 mb-1">一键安装命令</div>
      <div className="rounded-lg border border-white/10 bg-zinc-950 px-3 py-2 font-mono text-[11px] text-emerald-100 break-all">
        {cmd}
      </div>
      <button
        type="button"
        onClick={() => navigator.clipboard?.writeText(cmd).catch(() => {})}
        className="mt-2 rounded-md bg-zinc-800 hover:bg-zinc-700 px-2.5 py-1 text-xs"
      >
        复制安装命令
      </button>
    </div>
  )
})()}
```

- [ ] **Step 3: NodeDetail.tsx 顶部加按钮（不可见 token，但提示「凭据已遗失需重置」入口）**

P1 阶段不实现「重置 Agent token」——token 仅创建时显示。NodeDetail 加一段告知用户：

```tsx
// NodeDetail.tsx 节点详情顶部信息卡内加一段:
<div className="text-[11px] text-zinc-500 mt-2">
  Agent 安装命令需要创建节点时一次性显示的 token;
  如已遗失,后续(P2 阶段)将提供「轮换 Agent 凭据」入口。
</div>
```

不加按钮（避免承诺 P1 没有的能力）。

- [ ] **Step 4: 跑前端 build**

Run: `cd web && npm run build`
Expected: 绿。

- [ ] **Step 5: 手测**

`npm run dev` → 在 Settings 配 `agent_control_endpoint=http://localhost:50051` → 新建节点 → Modal 出现「一键安装命令」+ 复制按钮 → 命令含 `node=<新 id>` 和 `--token=<明文>`。

- [ ] **Step 6: Commit**

```bash
git add web/src/lib/api.ts web/src/pages/Nodes.tsx web/src/pages/NodeDetail.tsx
git commit -m "feat(web): show install command on node creation modal; rely on origin as base"
```

- [ ] **Step 7: spawn 子代理 review**

---

## Task 12: P1 文档 + e2e smoke + plan.md 附录更新

**Files:**
- Modify: `README.md`（特性段补 Toast、一键安装、防删节点）
- Modify: `docs/deployment.md`（加「P1 一键安装节点」一节）
- Modify: `docs/api.md`（加 `/install.sh` 与 `/dist/*` 段）
- Modify: `plan.md`（附录·实施状态加一段 P1 完成）

- [ ] **Step 1: README.md 特性段补**

```markdown
## 特性

- ...原有条目不动...
- 通知:右上角全局 Toast 反馈所有写操作。
- 节点安装:Settings 配 Agent 端点后,创建节点 Modal 一键复制安装命令,
  目标机 `curl ... | sudo bash` 完成接入。
- 防呆:节点上仍有活跃规则时拒绝删除。
```

- [ ] **Step 2: docs/deployment.md 新增「P1 一键安装节点」一节**

放在「Caddy 反代」之后：

```markdown
## 一键安装节点(P1)

前置:在 Web 面板「设置」页填 **Agent 上报端点**(如 `https://relay.example.com:50051`),
并确保 `PANEL_PUBLIC_BASE_URL` env 配为面板对外可访问的 origin(如 `https://relay.example.com`)。

步骤:

1. 面板「节点」页点「新增节点」,提交后弹出 Modal 显示 Agent token + 一键安装命令。
2. 复制命令(形如 `curl -fsSL https://relay.example.com/install.sh?node=42 | sudo bash -s -- --token=<明文>`)。
3. 在目标机以 root 执行该命令。脚本会:
   - 下载 `/dist/node-agent-linux-<amd64|arm64>` 到 `/usr/local/bin/emorelay-agent`
   - 写 `/etc/emorelay/agent.env`(权限 0600)
   - 写 `/etc/systemd/system/emorelay-agent.service`
   - `systemctl enable --now emorelay-agent`
4. 回到面板「节点」页,节点状态在 1-2 分钟内变 `online`。

token 仅创建时显示一次;复制走再丢失只能(P2 阶段)走「轮换凭据」入口。
```

- [ ] **Step 3: docs/api.md 加 install 段**

放在最末:

```markdown
## 安装相关(公开,带 rate limit)

### `GET /install.sh?node=<id>`

返回参数化 bash 脚本,Content-Type `text/x-shellscript`。需要调用时附加 `--token=<明文>`:

```sh
curl -fsSL https://relay.example.com/install.sh?node=42 | sudo bash -s -- --token=<明文>
```

Rate limit:60 req/分钟/IP。

### `GET /dist/node-agent-linux-{amd64,arm64}`

提供预编译 agent 二进制下载。其他文件名 → 404。Rate limit 同上。
```

- [ ] **Step 4: plan.md 附录·实施状态加一段**

```markdown
### Phase 1(2026-06-10 启动)

(7) 全局 Toast、(8) 防删节点、(11) 创建规则默认 TCP+UDP、(2) Settings 加 Agent 上报端点、
(1) 一键安装 URL —— 全部交付。验收清单见 `docs/superpowers/specs/2026-06-10-mvp-followups-design.md` §2.7。
对应 plan 见 `docs/superpowers/plans/2026-06-10-mvp-followups-phase-1.md`。
```

- [ ] **Step 5: 跑所有测试 + build 终验**

```bash
cargo test --workspace
cd web && npm test && npm run build && npm run lint
```
Expected: 全绿。

- [ ] **Step 6: e2e smoke**

```bash
docker compose down
docker compose up -d --build
sleep 5
# 用 .env 里的 admin 登录,通过 API 创建一个节点,确认返回 agent_token:
TOKEN=$(curl -s -X POST http://localhost:8080/api/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"<.env 的 PANEL_BOOTSTRAP_ADMIN_PASSWORD>"}' \
  | python -c 'import sys,json;print(json.load(sys.stdin)["token"])')
curl -s -X POST http://localhost:8080/api/nodes \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"name":"smoke-1"}'
# 校验返回含 agent_token + node.id
# 再设端点 + 拉 install.sh
curl -s -X PATCH http://localhost:8080/api/system/settings \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"settings":{"agent_control_endpoint":"http://127.0.0.1:50051"}}'
curl -fsS 'http://localhost:8080/install.sh?node=1' | head -5
# 应输出 #!/usr/bin/env bash + AGENT_NODE_ID=1
```

- [ ] **Step 7: Commit**

```bash
git add README.md docs/deployment.md docs/api.md plan.md
git commit -m "docs(p1): document Phase 1 features and one-click install flow"
```

- [ ] **Step 8: 最终 spawn 子代理 review**

把整批 P1 commits 的 diff（`git log master..HEAD` 范围）一次性交给 `general-purpose` 子代理调 `superpowers:code-reviewer` 做 phase-end 审查。审查通过 → P1 收尾，开始 P2 plan 展开。

---

## P1 收尾清单

完成所有 Task 后核对：

- [ ] Spec §2.7 验收清单全部勾掉
- [ ] `cargo test --workspace` 全绿
- [ ] `cd web && npm test && npm run build && npm run lint` 全绿
- [ ] `docker compose up -d --build` 一键起服务
- [ ] 干净 VM 上跑安装命令 → Agent online
- [ ] 12 个 Task 的子代理 review 全部 YES（无阻塞性问题）
- [ ] plan.md 附录·实施状态加了 P1 完成段

收尾后请告知用户 P1 完成，并询问是否启动 P2 plan 展开。
