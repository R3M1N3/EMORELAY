# flux-panel 2.0.7-beta 对比评审报告

日期：2026-06-13
对比对象：[bqlpfy/flux-panel](https://github.com/bqlpfy/flux-panel) tag `2.0.7-beta`（commit `761db18`，2026-01-06，2.x 系列最新 tag；本地检出于 `C:/Users/EMOPAO/Desktop/flux-panel-ref`）
方法：3 个只读子代理分别通读 flux 前端（`vite-frontend`，约 1.19 万行 TS/TSX）、flux 后端与工程（`springboot-backend` + 魔改 `go-gost` + 安装脚本）、EMORELAY 前端现状（`web/src` 全部 36 个文件）；对涉及我们自身行为的结论由主代理直接核对 Rust 源码确认。

---

## 1. flux 2.x 总体形态

```
[React 前端 / iOS·Android WebView 壳] ──REST(全 POST)──► [Spring Boot :6365 + SQLite]
                                                              │  ▲
                          WebSocket /system-info?secret=明文（控制面，AES-GCM 消息加密）
                                                              ▼  │
                                                       [魔改 fork 的 go-gost]
                                                              │
                          HTTP POST /flow/upload（5s 增量流量）、/flow/config（10min 全量对账）
```

- 节点端**不是自研 relay**，而是 fork 整个 `go-gost/x` 源码树，内嵌 WS 客户端接收 `AddService/Pause/TcpPing/SetProtocol` 等命令，进程内热加载 gost registry。
- 控制面鉴权 = URL query string 明文 secret + `ws://` 明文传输；消息体 AES-256-GCM（key=SHA256(secret)），**解密失败静默回退明文**，加密是装饰性的。
- 前端 React 18 + HeroUI + Tailwind v4，无状态库、无 i18n、无测试；移动端三套布局（桌面/H5/H5二级页）+ WebView 原生壳。

一句话定位：**产品功能面（计费颗粒度、诊断、移动端）领先我们，工程与安全水位显著低于我们**。

## 2. 值得借鉴——产品功能（按优先级）

### P0：低成本高收益，建议尽快做

| # | 功能 | flux 做法（源码参照） | 对我们的落点 | 估算 |
|---|---|---|---|---|
| 1 | **点击复制入口地址** | 转发卡片单地址一击复制、多地址弹列表+「复制全部」，自动拼 `[IPv6]:port`（`forward.tsx:749-826`） | 面板最高频日常操作就是复制地址发给用户；我们 Rules 列表/详情完全没有行内复制 | 半天 |
| 2 | **强制删除逃生门** | 常规删除失败（节点失联）时 confirm 询问「强制删除（跳过节点端验证）」（`forward.tsx:478-508`） | 我们有命令重试队列，但节点长期失联时规则仍无法干净删除，需要同款逃生门（仅 admin） | 半天~1 天 |
| 3 | **首登强制改密** | 登录响应带 `requirePasswordChange`，前端强制跳无布局改密页（`index.tsx:184-193`） | 我们默认 admin 密码只靠文档提醒；该机制可靠且便宜 | 1 天 |
| 4 | **到期预警 + 去重** | Dashboard 检测 7 天内到期，分级 toast，localStorage 记 key 防重复轰炸（`dashboard.tsx:73-160`） | 我们用户到期仅在 Users 列表可见，用户自己无感知 | 半天 |

### P1：高价值、中等成本

| # | 功能 | flux 做法 | 对我们的落点 | 估算 |
|---|---|---|---|---|
| 5 | **逐段链路诊断**（两个分析代理共同的首推） | 面板下发 `TcpPing`，逐段测 入口→每跳→出口→目标 的 TCP 连通/延迟/失败率，前端按跳分组渲染（Java `diagnoseForward/diagnoseTunnel`，Go `handleTcpPing`，前端 `forward.tsx:622-678`） | 我们隧道只有 hop 心跳状态聚合，无主动探测。需给 Agent 加一个白名单探测指令（符合「只接受白名单规则操作」红线，getStats 同类），proto 加 `Probe` 命令 | 2-3 天 |
| 6 | **配置对账自愈** | 节点每 10 分钟全量上报运行中 gost 配置，面板比对 DB 删孤儿（`CheckGostConfigAsync`） | 我们只有「面板→节点」的下发重试，缺「节点→面板」反向对账；可让心跳附带运行规则摘要（id+版本 hash），面板比对后补发/清理 | 2 天 |
| 7 | **协议嗅探阻断** | 节点对首包做 HTTP/TLS/SOCKS 指纹嗅探，命中即断连，防端口转发被当开放代理滥用（Go `detectProtocol`，节点级开关） | 自研 relay 加首包前缀匹配很薄；作为节点级开关，默认关 | 1-2 天 |
| 8 | **流量倍率 + 单/双向计费** | 隧道级 `traffic_ratio`（0-100x）+ `flow`∈{单向,双向}，计费时乘系数（`FlowController.processFlowData`） | 不同成本的隧道（直连 vs 三跳）配不同倍率是真实商家诉求；我们配额体系加一个乘法即可 | 1-2 天 |
| 9 | **每月固定日重置** | `flow_reset_time`(1-31，月末容错) + 每日定时任务重置；可一键手动重置且确认框展示当前用量（`ResetFlowAsync`、`user.tsx`） | 我们是滚动 30 天窗口；「自然月某日清零」更贴近商家卖套餐心智，可作为与滚动窗口并存的可选模式 | 2 天 |
| 10 | **节点实时监控推送** | WS 单端点双用途：节点上行 CPU/内存/网速 2s 一帧，广播给所有在线 admin；前端按 uptime 差分算实时网速、指数退避重连（`node.tsx:131-263`） | 我们 15s 轮询，数字不会「动」；可用 SSE（比 WS 改动小）把已有心跳数据推给前端 | 3 天 |
| 11 | **订阅用量 API** | `GET /open_api/sub_store` 返回 Clash 风格 `subscription-userinfo` 头（用量/总量/到期）（`OpenApiController`） | 用户在 Clash/Sub-Store 客户端直接看到套餐余量，约 50-100 行。**注意**：CLAUDE.md 范围外功能列有「订阅」，本项虽仅为只读用量披露、不含套餐分发/计费，但字面落在禁区内，**排期前需用户裁决** | 半天~1 天 |

### P2：按需 / 已在候选池

| # | 功能 | 说明 |
|---|---|---|
| 12 | **多目标 + 负载策略** | flux 支持 `remote_addr` 多目标 + fifo/round/rand/hash 策略，且仅多地址时才显示策略选择器（渐进披露，`forward.tsx:1650-1667`）。已在我们 P11 候选池（`docs/p1-gap-review-2026-06-13.md`），UI 形态可直接参考 |
| 13 | **行为验证码** | tianai-captcha 滑块/点选/旋转/拼接，服务端开关。我们已有 per-IP 登录限速，2FA 在 P11 候选池；优先级低于 2FA |
| 14 | **转发卡片拖拽排序** | @dnd-kit + DB `inx` 字段持久化。锦上添花 |
| 15 | **移动端 H5 细节包** | safe-area 工具类、`100dvh`、底部 Tabbar、诊断结果桌面表格/移动卡片双渲染、路由切换 scrollTo(0,0)。我们移动端目前只有汉堡侧栏+表格横滚，若认真做移动端这是现成清单 |
| 16 | **暂停时强断存量连接** | flux 用 `tcpkill` 外部工具（脏）；我们自研 relay 可在 stop 时干净地 abort 全部 bridge task——见 §5 问题 A |

## 3. 值得借鉴——工程与 UX 做法

1. **乐观更新 + 失败回滚**（`forward.tsx:570-620`）：启停 Switch 先翻转再请求，失败恢复 + toast。我们的规则启停目前是「请求→等响应→重拉」，体感慢。
2. **流量上报「成功才清零」**（`global_traffic_manager.go`）：本地累积增量→上报成功才减去已报数、失败保留下轮补报。我们正相反，见 §5 问题 B。
3. **导入逐行结果回显**（`forward.tsx:895-1010`）：行式文本格式（可从 Excel 粘贴）+ 逐行成功/失败原因 + `成功 x / 总计 y` 计数。我们 JSON dry-run 预检机制更强，但**结果呈现**可借鉴这种逐行面板。
4. **约束在 UI 层即时可见**（`tunnel.tsx:786-840`）：多跳编排下拉里禁用已占用节点并用 Chip 标注「已选为入口/出口/第 N 跳」，离线节点禁选。我们隧道链编辑只做了重复节点拦截。
5. **防主题闪烁内联脚本**（`index.html:8-47`）：首帧前同步打 dark class。我们目前仅暗色无切换；若做浅色模式必抄。
6. **声明式配置项渲染**（`config.tsx:46-102`）：CONFIG_ITEMS 数组定义 key/label/type/dependsOn，渲染器统一处理显隐，新增配置零模板。我们 Settings 页是手写表单，配置项继续增多时值得迁移。
7. **配置 localStorage 缓存先渲染**（`config/site.ts`）：首屏用缓存，后台静默 diff 刷新。
8. **变更追踪 UI**（`config.tsx`）：与原值 diff，变更项边框变黄 + 底部「未保存」脉冲条。
9. **安装脚本网络自适应**（`panel_install.sh`）：CN IP 自动换 GitHub 加速前缀（`ghfast.top`）、探测 IPv6 自动改 docker daemon、更新前 SIGTERM 优雅停等 WAL 落盘。我们 `deploy.sh` 拉 GitHub Release，CN 加速回退值得加。
10. **入口多节点端口求交算法**（`ForwardServiceImpl.get_port`）：指定端口全员校验→最小公共端口→各自分配，支撑「一条转发多入口」。若我们将来做多入口可参考。

## 4. 我们已领先 flux 的部分（保持，不动）

| 维度 | EMORELAY | flux 2.0.7-beta |
|---|---|---|
| 控制面安全 | 内置 CA + 强制 mTLS + CRL 吊销 + 凭据自动轮换 | `ws://` 明文 + URL query 明文 secret；AES 层解密失败**静默回退明文**；无法防 MITM |
| 密码存储 | Argon2 | **密码路径全程无盐 MD5**（`Md5Util` 备有带盐变体但登录/建号/改密均走裸 `md5()`） |
| JWT | secret 从环境变量、标准库 | 手写 JWT，过期 90 天，无吊销；util 层多个 getter 不验签直接解 payload（`/api/**` 有拦截器先验签兜底，被排除的 `/flow/**` 等路径无此保障） |
| 上报归属校验 | 证书绑 node_id + 上报归属校验（2026-06-13 加固） | `/flow/upload` 凭 secret 可给**任意用户**刷流量/触发停服（正是我们刚修的漏洞的活样本） |
| 目标校验 | 禁内网目标（P5）+ host 形状校验 | `remote_addr` 零校验，SSRF/内网穿透敞开 |
| 保留端口 | 22/80/443 等默认黑名单 | 无任何系统端口保护 |
| 登录防护 | per-IP 限速（常开） | 仅验证码（配置可关，关了裸奔），无限速无锁定 |
| 审计 | `audit_logs` 全危险操作落库 | 无审计表；logback 把**含密码的请求参数**打进日志 |
| 节点离线一致性 | 命令重试队列 + Agent 规则落盘自愈恢复 | 节点离线变更直接失败丢弃，重连后只删孤儿**不补发缺失服务** |
| 删除语义 | 软删除 + 删除预检/保护 | 全硬删，删节点级联删隧道删转发 |
| 数据库 | SQLx migrations + 索引 + PG 兼容 | `CREATE TABLE IF NOT EXISTS` 无迁移体系、**零索引**、N+1 遍地 |
| 节点内核 | Rust 自研 relay（splice 零拷贝、token bucket、并发上限） | fork 整个 go-gost 源码树，与上游脱钩（我们拒绝 fork realm 的反面教材） |
| Agent 运维 | 一键升级（P10b）+ 凭据轮换 | 无面板内升级，靠脚本菜单重装 |
| 授权模型 | 节点/隧道 ACL 默认拒绝（P7） | 用户×隧道授权有（且更细），但无默认拒绝概念 |
| 通知 | webhook 四事件 | 无任何系统级通知 |
| 前端数据新鲜度 | 全站 15/30s 自动刷新 | 除节点页 WS 外全是打开时快照，数字不会动 |
| 测试 | cargo workspace 全绿 + vitest + e2e | **零测试**（前后端皆无） |
| 在线判定 | 心跳超时 sweeper + 掉线 webhook | 纯 WS 连接事件，面板重启后状态错乱，无告警 |

另有后端明显 bug 可佐证其工程水位：`updateUserTunnel` 成功路径返回错误（`UserTunnelServiceImpl:101`）、`createForward` 失败回滚循环里 `return` 写在 for 内只回滚第一个节点、锁 map 只增不删慢性泄漏、CORS 全开 `*`。

## 5. 本次对比暴露的我们自身的问题（已核实源码）

**A. 禁用/删除规则不断存量连接**
`crates/node-agent/src/relay/tcp.rs`：`stop()` 只停 listener，per-connection bridge task 是 detached spawn，注释明确「沿用 stop 不断存量连接的 MVP 语义」（tcp.rs:75）。后果：被禁用/删除/超配额停用的规则，存量长连接（视频流、隧道内 TCP）**继续转发并继续计量**，与 flux 需要 tcpkill 解决的是同一问题。连接就在我们进程内（当前 bridge task 的 JoinHandle 是 spawn 后即丢弃，并未持有），修复比 flux 干净得多：bridge task 挂到 JoinSet/CancellationToken，stop 时主动 abort。建议列入下个迭代（注意 UDP/tunnel task 同样核查）。

**B. 流量上报存在丢数窗口**
`crates/node-agent/src/agent.rs:226`：`drain_snapshot()` 先把计数器 swap 清零，再发 gRPC；`report_rule_stats` 失败（面板重启/网络抖动）时已 drain 的快照被丢弃。flux 的「上报成功才清零」语义（`global_traffic_manager.go`）更正确。修复：失败时把快照加回计数器（fetch_add 回去）或本地暂存重试。计费面板丢账是商家敏感问题，建议与 A 同批修。

## 6. 明确不学的部分

- 全 POST API、Promise 永不 reject、靠魔法字符串 `msg === '未登录或token已过期'` 判 token 失效。
- 2400 行单文件组件、组件定义在父组件函数体内（每次渲染重建）、格式化函数复制 3-4 份。
- `minify: false` + `treeshake: false` 的构建配置、死依赖（react-beautiful-dnd/sonner 装而不用）。
- admin 标志存 localStorage 做前端菜单鉴权、`atob` 手解 JWT。
- 排序状态 DB + localStorage 双写两套优先级逻辑。
- 业务逻辑塞 Controller、`synchronized` + 进程内锁 map 的并发模型。

## 7. 建议行动汇总

| 优先级 | 事项 | 说明 |
|---|---|---|
| **修复** | §5-A 断存量连接、§5-B 上报丢数窗口 | 计费正确性问题，建议最先做 |
| P0 | 点击复制、强制删除逃生门、首登强制改密、到期预警 toast | 共约 2-3 天，纯增量 |
| P1 | 逐段诊断、配置对账自愈、协议嗅探开关、流量倍率/单双向、月度重置模式、SSE 节点实时推送、订阅用量 API | 各 1-3 天，按业务需要排期 |
| P2 | 多目标 LB（P11 已有）、拖拽排序、移动端 H5 包、声明式 Settings、deploy.sh CN 加速 | 按需触发 |

flux 对我们最大的价值不是「功能更多」，而是它验证了**商家真实跑业务需要什么颗粒度**（用户×隧道二维套餐、倍率计费、自然月重置、逐段诊断、给用户复制地址的顺手程度）；而它的安全与一致性短板，反向确认了我们坚持自研 Agent、mTLS、迁移体系、重试队列这些「慢功夫」是正确的护城河。
