//! MoogleMail 周期观察与恢复：Inbox 由持久 intake journal 处理，Read Mail 仅观察。
//!
//! 新 Send 只凭原尝试冻结的主题、正文摘要、世界、收件人和完整资产唯一恢复；
//! 旧行保留原有保守证明，不能以当前登录 key 重建历史发送。未找到 Sent 不证明未发送，
//! 未知 Send 永不自动重试。compose 草稿仍只在精确匹配后持短时独占锁受控丢弃。
//!
//! 子模块分工：`sweep` 负责单轮候选轮转与预算，`sent` 负责 Sent/Take 恢复与 Read Mail
//! 观察，`cleanup` 负责精确匹配草稿的受控丢弃，`diagnostics` 负责预算错误与远端读取警告。

mod cleanup;
mod diagnostics;
mod sent;
mod sweep;

use std::sync::Arc;

use uuid::Uuid;

use crate::ports::core::Clock;
use crate::ports::mail::{MoogleMailboxEvidenceStore, MoogleMailboxMaintenance};

/// 单轮维护结果，只包含安全统计，不含邮件正文。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MailboxMaintenanceReport {
    pub compose_clean: bool,
    pub compose_error: Option<String>,
    pub matched_delivery: Option<Uuid>,
    pub discarded: bool,
    pub compose_matches: usize,
    pub sent_observed: usize,
    pub sent_recovered: usize,
    pub sent_candidates: usize,
    pub sent_matched: usize,
    pub sent_state_failed: usize,
    /// 最近一次详情读取失败的安全分类，便于定位协议漂移。
    pub sent_state_error: Option<String>,
    /// 本轮退回收发队列重试的投递数。
    pub sweep_requeued: usize,
}

/// 维护服务；由 Worker 在固定周期调用，不存在自动重试队列。
pub struct MailboxMaintenanceService {
    maintenance: Arc<dyn MoogleMailboxMaintenance>,
    store: Arc<dyn MoogleMailboxEvidenceStore>,
    clock: Arc<dyn Clock>,
}

impl MailboxMaintenanceService {
    pub fn new(
        maintenance: Arc<dyn MoogleMailboxMaintenance>,
        store: Arc<dyn MoogleMailboxEvidenceStore>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self { maintenance, store, clock }
    }
}
