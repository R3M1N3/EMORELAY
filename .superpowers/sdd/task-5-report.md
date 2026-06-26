# Task 5 报告:StatsCollector counter 对账清理(Gap #4)

**status: DONE_WITH_CONCERNS**(实现完整、测试全绿;concern 为对任务前提的一处事实修正,非缺陷)

## 改动文件

- `crates/node-agent/src/stats.rs`
  - 新增 `StatsCollector::remove(&self, rule_id: i64)` —— 单规则删除即时丢弃其 counter。
  - 新增 `StatsCollector::retain(&self, keep_ids: &HashSet<i64>)` —— 对账兜底,移除所有不在 keep_ids 内的规则 counter。
  - import 由 `use std::collections::HashMap;` 改为 `use std::collections::{HashMap, HashSet};`。
  - 新增 3 个单测:`retain_drops_unknown_rules`、`retain_preserves_active_counter_values`、`remove_drops_single_rule`。
- `crates/node-agent/src/manager.rs`
  - `RuleManager::remove` 末尾增 `self.stats.remove(rule_id);`(即时清理)。
  - `RuleManager::reconcile` 在删孤儿 handle 后增 `self.stats.retain(&keep);`(兜底对账;复用函数内已有的 `keep: HashSet<i64>`)。
  - 新增 1 个 manager 层接通测试:`remove_and_reconcile_clean_stats_counters`。

reconcile 调用链入口:`agent.rs:380` `Body::ReconcileRules(rec) => mgr.reconcile(&rec.rule_ids)`(未改,无需改)。

## 测试命令与输出

```
cargo test -p node-agent
→ test result: ok. 92 passed; 0 failed; 0 ignored

本 task 相关用例(全 ok):
  manager::tests::remove_and_reconcile_clean_stats_counters ... ok
  stats::tests::remove_drops_single_rule ... ok
  stats::tests::retain_drops_unknown_rules ... ok
  stats::tests::retain_preserves_active_counter_values ... ok
  stats::tests::restore_rebuilds_removed_counter ... ok   (回归,在途回填仍重建,未破)
```

TDD 流程:
- RED:加测试后 `cargo test -p node-agent --lib stats::` → E0599 `method retain/remove not found`(预期编译失败)。
- GREEN:实现后全量 92 passed。
- clippy:`cargo clippy -p node-agent --all-targets` → stats.rs / manager.rs **零警告**(仓库其余 14 个预存警告均在 store.rs 等未触文件,未处理,符合外科手术原则)。

## Self-Review(对照红线)

- **无 proto/迁移/新依赖**:仅加 2 方法 + 2 处一行调用;`HashSet` 出自 std。✅
- **retain 不误删活跃规则 counter**:`retain(|id,_| keep_ids.contains(id))` 精确保留 keep 集合;`retain_preserves_active_counter_values` 断言存活 counter 累计值(123)未被改动。reconcile 时孤儿 relay task 已先 stop(handle 先删),hot-path 不会再 `ensure` 复活被清规则。✅
- **与即时 remove 不冲突**:二者互补、同一把 `counters` 写锁;`remove` 走单规则删除路径,`retain` 走 reconcile 路径,不交叉矛盾。✅
- **上报 drain/restore 不受影响**:`drain_snapshot`/`restore` 未改;`run_session` 单 task select 循环 → `report_stats`(drain/restore)与 `handle_command`(remove/retain)不并发,逐 await 串行。最坏假设(drain 与失败回填之间被清)由 `restore` 经 `ensure` 重建兜底 —— 既有 `restore_rebuilds_removed_counter` 仍绿,不丢计费数。✅
- **不碰 TCP 转发热路径**:`ensure`(唯一热路径入口)与 relay/splice/缓冲零改动;remove/retain 仅在删除/对账等稀疏控制面事件时取写锁。✅
- **YAGNI**:2 个极简方法 + 2 行调用点,无投机设计。✅

## Concerns

1. **任务前提事实修正(非缺陷)**:任务描述称"删规则时 counter 被 remove,但缺对账兜底"。实测代码中 `StatsCollector` **原本没有任何 remove**,`RuleManager::remove`/`reconcile` **从不触碰 stats** —— counter 自创建(`ensure`)后从不清理,规则每次生命周期都按历史峰值累积。故本实现同时补了「即时 remove(接入删除路径)」**与**「retain 兜底」,比单加兜底更彻底地关闭无界增长 gap。这与 plan「保留正常删除时的即时 remove」一致 —— 只是该「即时 remove」此前并不存在,本次一并补齐。

2. **回填后短暂残留(已知、有界、可接受)**:若 reconcile/remove 清掉某 counter 后,一个更早 drain 的失败上报恰好 restore 该规则,会经 `ensure` 重建一个残留 counter,延续到下次 reconcile 才被 `retain` 清掉。残留有界(下个 reconcile 即清),且优先保证不丢计费数,符合既有 restore 语义取舍。

无阻塞性问题。
