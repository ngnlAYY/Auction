//! 进程内有界邮件观测；只供只读 control 查询，不能作为业务或持久游标依据。

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use auction_application::{
    MAX_MAIL_PROGRESS_POLLERS, MailPollClaim, MailPollProgressFailure, MailPollProgressObserver,
    MailPollProgressOutcome, MailPollProgressSnapshot, MailPollProgressStage,
    WorkerControlCondition, WorkerControlMailProgress, is_safe_mail_poll_id,
};
use chrono::Utc;
use uuid::Uuid;

/// Main/Isekai 共享的轻量观测 owner；重启清空，锁损坏仅令诊断不可用。
#[derive(Clone, Default)]
pub struct MailPollProgressTracker {
    records: Arc<Mutex<BTreeMap<String, TrackedPoll>>>,
}

struct TrackedPoll {
    claim_token: Uuid,
    state_version: i64,
    snapshot: MailPollProgressSnapshot,
}

impl TrackedPoll {
    fn matches(&self, claim: &MailPollClaim) -> bool {
        self.claim_token == claim.claim_token && self.state_version == claim.state_version
    }
}

impl MailPollProgressTracker {
    /// 读取新的进程快照；不可用与尚未观测不会伪装为空闲。
    pub fn snapshot(&self) -> WorkerControlMailProgress {
        let Ok(records) = self.records.lock() else {
            return WorkerControlMailProgress::unavailable(WorkerControlCondition::Unavailable);
        };
        let snapshot = WorkerControlMailProgress {
            condition: WorkerControlCondition::Available,
            observed_at: Some(Utc::now()),
            pollers: records.values().map(|record| record.snapshot.clone()).collect(),
        };
        if snapshot.validate() {
            snapshot
        } else {
            WorkerControlMailProgress::unavailable(WorkerControlCondition::Incompatible)
        }
    }
}

impl MailPollProgressObserver for MailPollProgressTracker {
    fn begin(&self, claim: &MailPollClaim, world_id: &str) {
        if !claim.validate() || !matches!(world_id, "main" | "isekai") {
            return;
        }
        let Ok(mut records) = self.records.lock() else { return };
        if !records.contains_key(&claim.mailbox_key) && records.len() >= MAX_MAIL_PROGRESS_POLLERS {
            return;
        }
        // 相同或更旧版本的 begin 不得把已结束的 claim 重新显示为在途。
        if records
            .get(&claim.mailbox_key)
            .is_some_and(|record| record.state_version >= claim.state_version)
        {
            return;
        }
        let last_failure =
            records.get(&claim.mailbox_key).and_then(|record| record.snapshot.last_failure.clone());
        records.insert(
            claim.mailbox_key.clone(),
            TrackedPoll {
                claim_token: claim.claim_token,
                state_version: claim.state_version,
                snapshot: MailPollProgressSnapshot {
                    mailbox_key: claim.mailbox_key.clone(),
                    world_id: world_id.to_owned(),
                    stage: MailPollProgressStage::Fetching,
                    external_mail_id: None,
                    updated_at: Utc::now(),
                    last_failure,
                },
            },
        );
    }

    fn update(
        &self,
        claim: &MailPollClaim,
        stage: MailPollProgressStage,
        external_mail_id: Option<&str>,
    ) {
        if !stage.is_active() {
            return;
        }
        let Ok(mut records) = self.records.lock() else { return };
        let Some(record) = records.get_mut(&claim.mailbox_key) else { return };
        if !record.matches(claim) || !record.snapshot.stage.is_active() {
            return;
        }
        record.snapshot.stage = stage;
        record.snapshot.external_mail_id = if matches!(
            stage,
            MailPollProgressStage::ReadingDetail | MailPollProgressStage::Processing
        ) {
            external_mail_id.filter(|value| is_safe_mail_poll_id(value)).map(str::to_owned)
        } else {
            None
        };
        record.snapshot.updated_at = Utc::now();
    }

    fn finish(&self, claim: &MailPollClaim, outcome: MailPollProgressOutcome) {
        let Ok(mut records) = self.records.lock() else { return };
        let Some(record) = records.get_mut(&claim.mailbox_key) else { return };
        if !record.matches(claim) || !record.snapshot.stage.is_active() {
            return;
        }
        let now = Utc::now();
        if outcome == MailPollProgressOutcome::Failed {
            record.snapshot.last_failure = Some(MailPollProgressFailure {
                stage: record.snapshot.stage,
                external_mail_id: record.snapshot.external_mail_id.clone(),
                occurred_at: now,
            });
        }
        record.snapshot.stage = match outcome {
            MailPollProgressOutcome::Succeeded => MailPollProgressStage::Idle,
            MailPollProgressOutcome::Failed => MailPollProgressStage::Failed,
            MailPollProgressOutcome::Canceled => MailPollProgressStage::Canceled,
        };
        record.snapshot.external_mail_id = None;
        record.snapshot.updated_at = now;
    }
}
