"""EMORELAY dev mock 数据灌注 —— 仅 dev,不在生产跑。

策略:
  1. 通过 REST API 创建节点 / 用户 / 规则(走完整路径,audit_logs 会有内容)。
  2. 直接 sqlite3 写入"已运行"模拟数据:
     - nodes.status='online' + 资源 + 累计 rx/tx
     - node_stats / rule_stats 144 个分钟级 bucket(过去 ~2.4 小时)
     - forward_rules.rx/tx/connection 累计求和

  panel-server 必须先启动(WAL 模式下 sqlite 并发写不会冲突)。
"""

import json
import random
import sqlite3
import sys
import urllib.request
from datetime import datetime, timedelta, timezone
from pathlib import Path

# Windows GBK 控制台无法编码 ✅ 等非 GBK 字符(末尾 print 会抛 UnicodeEncodeError,但数据已 commit);
# 统一把 stdout 切到 UTF-8(errors=replace 兜底,piped/重定向亦安全)。
try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
except Exception:
    pass

API = "http://localhost:8080"
DB = Path(r"C:\Users\EMOPAO\Desktop\relay\data\emorelay.db")
ADMIN_USER = "admin"
ADMIN_PASS = "admin12345"

random.seed(42)  # 确定性数据,重跑结果一致


def call(method: str, path: str, body=None, token=None):
    req = urllib.request.Request(f"{API}{path}", method=method)
    if body is not None:
        req.data = json.dumps(body).encode("utf-8")
        req.add_header("Content-Type", "application/json")
    if token:
        req.add_header("Authorization", f"Bearer {token}")
    try:
        with urllib.request.urlopen(req) as r:
            return json.loads(r.read())
    except urllib.error.HTTPError as e:
        msg = e.read().decode("utf-8", "replace")
        raise SystemExit(f"HTTP {e.code} {method} {path}: {msg}")


def login(u, p):
    return call("POST", "/api/auth/login", {"username": u, "password": p})["token"]


def main():
    if not DB.exists():
        sys.exit(f"db not found: {DB}; start panel-server first")

    print("=== 1. login admin ===")
    admin_token = login(ADMIN_USER, ADMIN_PASS)

    print("=== 2. create nodes ===")
    nodes_def = [
        ("hk-relay-01", "HK", "103.245.10.5",  "https://hk.agent.example.com:50051", 20000, 25000),
        ("jp-relay-02", "JP", "52.69.12.8",     "https://jp.agent.example.com:50051", 20000, 25000),
        ("sg-relay-03", "SG", "178.128.55.42",  "https://sg.agent.example.com:50051", 30000, 35000),
    ]
    node_ids = []
    for name, region, ip, ep, pmin, pmax in nodes_def:
        resp = call("POST", "/api/nodes", {
            "name": name, "region": region, "public_ip": ip,
            "grpc_endpoint": ep,
            "port_pool_min": pmin, "port_pool_max": pmax,
        }, token=admin_token)
        node_ids.append(resp["node"]["id"])
        print(f"  node #{resp['node']['id']} {name}  (agent_token 已只显示一次)")

    print("=== 3. create users (alice, bob) ===")
    # P7 起节点默认拒绝:建用户时直接授权,否则第 4 步用户建规则会 400。
    alice = call("POST", "/api/users", {
        "username": "alice", "password": "alice12345", "role": "user",
        "granted_node_ids": node_ids,
    }, token=admin_token)
    bob = call("POST", "/api/users", {
        "username": "bob", "password": "bob12345", "role": "user",
        "granted_node_ids": node_ids,
    }, token=admin_token)
    print(f"  alice(#{alice['id']}) / bob(#{bob['id']}) created + 全节点授权")

    print("=== 4. create rules ===")
    # (node_idx, name, protocol, listen_port, target_host, target_port, owner_user_id)
    # 新建用户默认 must_change_password=1,其登录 token 受限(仅 me/改密),无法建规则;
    # 故用户归属规则统一由 admin token 带 user_id 创建(admin 不受限,且用户已授权全节点)。
    rules_def = [
        (0, "game-jp-route",   "tcp",     20001, "game-us.example.com",   443,   None),
        (0, "voice-hk-route",  "udp",     20002, "voice-us.example.com",  8888,  None),
        (1, "alice-web-proxy", "tcp_udp", 20003, "203.0.113.45",          443,   alice["id"]),
        (1, "bob-ssh-jump",    "tcp",     20004, "198.51.100.22",         22,    bob["id"]),
        (2, "game-sg-route",   "tcp",     30001, "game-eu.example.com",   25565, None),
        (2, "alice-stream",    "udp",     30002, "stream-eu.example.com", 1935,  alice["id"]),
    ]
    rule_ids = []
    for ni, name, proto, lp, th, tp, owner_id in rules_def:
        body = {
            "node_id": node_ids[ni], "name": name, "protocol": proto,
            "listen_port": lp, "target_host": th, "target_port": tp,
        }
        if owner_id is not None:
            body["user_id"] = owner_id
        resp = call("POST", "/api/rules", body, token=admin_token)
        rule_ids.append(resp["id"])
        print(f"  rule #{resp['id']} {name}  ({proto} {lp}→{th}:{tp})")

    print("=== 5. inject 'running' stats via SQL ===")
    GB = 1024 ** 3
    node_metrics = [
        # (cpu_avg%, mem_avg%, load_avg, rx_total_bytes, tx_total_bytes)
        (18.5, 42.3, 0.45, int(3.2 * GB),  int(5.1 * GB)),
        (35.7, 61.2, 0.85, int(8.4 * GB),  int(12.7 * GB)),
        (12.1, 28.4, 0.22, int(1.5 * GB),  int(2.0 * GB)),
    ]

    conn = sqlite3.connect(str(DB), timeout=10.0)
    conn.execute("PRAGMA journal_mode=WAL")
    conn.execute("PRAGMA busy_timeout=10000")
    cur = conn.cursor()

    # 新建用户默认 must_change_password=1(首登强制改密);dev 直接清掉,使 alice/bob 可登录自助页。
    cur.execute("UPDATE users SET must_change_password=0 WHERE username IN ('alice','bob')")

    # nodes 表标在线 + 资源 + 累计流量
    for nid, (cpu, mem, load, rx, tx) in zip(node_ids, node_metrics):
        cur.execute(
            "UPDATE nodes SET status='online', last_seen_at=datetime('now'), "
            "cpu_usage=?, memory_usage=?, load_average=?, rx_bytes_total=?, tx_bytes_total=?, "
            "updated_at=datetime('now') WHERE id=?",
            (cpu, mem, load, rx, tx, nid),
        )

    # 144 个分钟级 bucket (~2.4h),server 端 ORDER BY bucket_at DESC LIMIT 144 正好取这些
    now = datetime.now(timezone.utc).replace(second=0, microsecond=0)
    buckets = [(now - timedelta(minutes=i)).strftime("%Y-%m-%d %H:%M:%S") for i in range(144)]

    # node_stats
    node_stats_rows = 0
    for nid, (cpu_avg, mem_avg, load_avg, _, _) in zip(node_ids, node_metrics):
        for bt in buckets:
            cur.execute(
                "INSERT OR IGNORE INTO node_stats (node_id, bucket_at, cpu_usage, memory_usage, load_average, rx_bytes, tx_bytes) "
                "VALUES (?,?,?,?,?,?,?)",
                (
                    nid, bt,
                    max(0.0, cpu_avg + random.uniform(-8, 8)),
                    max(0.0, mem_avg + random.uniform(-5, 5)),
                    max(0.0, load_avg + random.uniform(-0.2, 0.2)),
                    random.randint(500_000, 5_000_000),
                    random.randint(500_000, 5_000_000),
                ),
            )
            node_stats_rows += 1

    # rule_stats + forward_rules 累计
    rule_stats_rows = 0
    for rid in rule_ids:
        total_rx = 0
        total_tx = 0
        total_conn = random.randint(120, 5200)
        for bt in buckets:
            rx = random.randint(10_000, 1_000_000)
            tx = random.randint(10_000, 1_000_000)
            conn_cnt = random.randint(0, 30)
            err = 1 if random.random() < 0.02 else 0
            cur.execute(
                "INSERT OR IGNORE INTO rule_stats (rule_id, bucket_at, rx_bytes, tx_bytes, connection_count, error_count) "
                "VALUES (?,?,?,?,?,?)",
                (rid, bt, rx, tx, conn_cnt, err),
            )
            total_rx += rx
            total_tx += tx
            rule_stats_rows += 1
        cur.execute(
            "UPDATE forward_rules SET rx_bytes=?, tx_bytes=?, connection_count=?, "
            "updated_at=datetime('now') WHERE id=?",
            (total_rx, total_tx, total_conn, rid),
        )

    conn.commit()
    conn.close()

    print(f"  nodes online: {len(node_ids)}")
    print(f"  node_stats rows: {node_stats_rows}")
    print(f"  rule_stats rows: {rule_stats_rows}")
    print()
    print("✅ done. 刷新 http://localhost:5173:")
    print("   - 概览:节点/规则/连接/总流量/24h 流量都该亮")
    print("   - 节点详情:点击 hk-relay-01 看 CPU/MEM/LOAD + rx/tx 时序")
    print("   - 规则详情:点击 game-jp-route 看 144 个 bucket 折线")
    print("   - 设置:底部 audit logs 有十几条 (登录 + 创建)")
    print("   - 用户:看到 admin + alice + bob")
    print()
    print("额外可登录:")
    print("   alice / alice12345 (普通用户,只看到 alice-web-proxy + alice-stream)")
    print("   bob   / bob12345   (只看到 bob-ssh-jump)")


if __name__ == "__main__":
    main()
