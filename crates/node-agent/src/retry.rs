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
        let Some(rid) = rule_id_of(&cmd) else {
            return false;
        };
        let attempts = prev_attempts + 1;
        self.items.retain(|e| rule_id_of(&e.cmd) != Some(rid));
        if attempts >= MAX_ATTEMPTS {
            tracing::warn!(rule_id = rid, attempts, "command retry exhausted; giving up");
            return false;
        }
        self.items.push(Entry {
            cmd,
            due: now + RETRY_DELAY,
            attempts,
        });
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
            .map(|e| PendingCommand {
                cmd: e.cmd,
                attempts: e.attempts,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emorelay_common::control::v1::{command::Body, ApplyRule, Command, RemoveRule, Rule};
    use std::time::{Duration, Instant};

    fn apply_cmd(rule_id: i64) -> Command {
        Command {
            body: Some(Body::ApplyRule(ApplyRule {
                rule: Some(Rule {
                    id: rule_id,
                    ..Default::default()
                }),
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
        let remove = Command {
            body: Some(Body::RemoveRule(RemoveRule { rule_id: 7 })),
        };
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
