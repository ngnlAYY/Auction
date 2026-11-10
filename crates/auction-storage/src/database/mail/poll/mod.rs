//! MoogleMail Worker 持久化适配器装配；状态与租约在 `state.rs`，轮询 claim/commit/fail 在
//! `claim.rs`，入站事实与命令幂等写入在 `inbound.rs`。外部 GET 不在事务内执行，Worker 拿到
//! claim 后才调 gateway；`reads.rs` 在详情 GET 前登记恢复队列，commit 原子写入事实、cursor 并删除完成前缀。

use auction_application::admin::AdminMailPollState;
use auction_application::auction::ApplicationFuture;
use auction_application::mail::{
    MAX_MAIL_POLL_BATCH_SIZE, MAX_MAIL_POLL_FAILURE_CODE_BYTES, MAX_MAIL_POLL_FAILURE_DETAIL_BYTES,
    MailPollBatch, MailPollClaim, MailPollCompletion, MailPollReadJournal, MailPollReadSummary,
    MailPollStore,
};
use auction_domain::valid_text;
use chrono::{DateTime, Utc};

use crate::database::DatabaseStore;
use crate::database::storage_error;

mod claim;
mod inbound;
mod reads;
mod state;

impl MailPollReadJournal for DatabaseStore {
    fn pending_reads(
        &self,
        claim: MailPollClaim,
        world_id: String,
        limit: u16,
    ) -> ApplicationFuture<'_, Vec<MailPollReadSummary>> {
        Box::pin(async move {
            if !claim.validate()
                || !matches!(world_id.as_str(), "main" | "isekai")
                || !(1..=MAX_MAIL_POLL_BATCH_SIZE).contains(&limit)
            {
                return Err(storage_error("MoogleMail 待读查询参数无效"));
            }
            reads::pending(self.pool(), &claim, &world_id, limit).await
        })
    }

    fn stage_reads(
        &self,
        claim: MailPollClaim,
        world_id: String,
        summaries: Vec<MailPollReadSummary>,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            let mut seen = std::collections::HashSet::new();
            if !claim.validate()
                || !matches!(world_id.as_str(), "main" | "isekai")
                || summaries.is_empty()
                || summaries.len() > usize::from(MAX_MAIL_POLL_BATCH_SIZE)
                || summaries.iter().any(|summary| {
                    !summary.validate()
                        || summary.world_id != world_id
                        || !seen.insert(summary.external_mail_id.as_str())
                })
            {
                return Err(storage_error("MoogleMail 待读登记参数无效"));
            }
            reads::stage(self.pool(), &claim, &world_id, &summaries).await
        })
    }
}

impl MailPollStore for DatabaseStore {
    fn lookup_completion(
        &self,
        mailbox_key: String,
        world_id: String,
        external_mail_id: String,
        received_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, MailPollCompletion> {
        Box::pin(async move {
            if !valid_text(&mailbox_key, 256)
                || !valid_text(&world_id, 256)
                || !valid_text(&external_mail_id, 512)
                || received_at.timestamp_nanos_opt().is_none()
            {
                return Err(storage_error("MoogleMail 完成事实查询参数无效"));
            }
            inbound::lookup_completion(
                self.pool(),
                &mailbox_key,
                &world_id,
                &external_mail_id,
                received_at,
            )
            .await
        })
    }

    fn ensure_state(
        &self,
        mailbox_key: String,
        now: DateTime<Utc>,
        scheduled_next_at: DateTime<Utc>,
        retry_maximum: std::time::Duration,
    ) -> ApplicationFuture<'_, AdminMailPollState> {
        Box::pin(async move {
            let Ok(retry_maximum_seconds) = i64::try_from(retry_maximum.as_secs()) else {
                return Err(storage_error("MoogleMail 邮箱初始化参数无效"));
            };
            if !valid_text(&mailbox_key, 256)
                || now.timestamp_nanos_opt().is_none()
                || scheduled_next_at.timestamp_nanos_opt().is_none()
                || scheduled_next_at <= now
                || retry_maximum_seconds <= 0
            {
                return Err(storage_error("MoogleMail 邮箱初始化参数无效"));
            }
            state::ensure(self.pool(), &mailbox_key, now, scheduled_next_at, retry_maximum_seconds)
                .await
        })
    }

    fn claim_due(
        &self,
        mailbox_key: String,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        limit: u16,
        force: bool,
    ) -> ApplicationFuture<'_, Vec<MailPollClaim>> {
        Box::pin(async move {
            if !valid_text(&mailbox_key, 256)
                || !valid_text(&worker_id, 256)
                || now.timestamp_nanos_opt().is_none()
                || lease_until <= now
                || limit == 0
                || limit > MAX_MAIL_POLL_BATCH_SIZE
            {
                return Err(storage_error("MoogleMail 轮询领取参数无效"));
            }
            claim::claim(self.pool(), &mailbox_key, &worker_id, now, lease_until, limit, force)
                .await
        })
    }

    fn commit(
        &self,
        claim: MailPollClaim,
        batch: MailPollBatch,
    ) -> ApplicationFuture<'_, AdminMailPollState> {
        Box::pin(async move {
            if !claim.validate() || !batch.validate(&claim) {
                return Err(storage_error("MoogleMail 轮询提交参数无效"));
            }
            claim::commit(self.pool(), &claim, &batch, None).await
        })
    }

    fn renew(
        &self,
        claim: MailPollClaim,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
    ) -> ApplicationFuture<'_, MailPollClaim> {
        Box::pin(async move {
            if !claim.validate()
                || now.timestamp_nanos_opt().is_none()
                || lease_until <= now
                || lease_until <= claim.lease_until
            {
                return Err(storage_error("MoogleMail 轮询续租参数无效"));
            }
            state::renew(self.pool(), claim, now, lease_until).await
        })
    }

    fn fail_after_prefix(
        &self,
        claim: MailPollClaim,
        mut batch: MailPollBatch,
        retry_at: DateTime<Utc>,
        failure_code: String,
        safe_detail: String,
    ) -> ApplicationFuture<'_, AdminMailPollState> {
        Box::pin(async move {
            batch.next_poll_at = retry_at;
            if !claim.validate()
                || !batch.validate(&claim)
                || !valid_text(&failure_code, MAX_MAIL_POLL_FAILURE_CODE_BYTES)
                || !valid_text(&safe_detail, MAX_MAIL_POLL_FAILURE_DETAIL_BYTES)
            {
                return Err(storage_error("MoogleMail 失败前缀提交参数无效"));
            }
            claim::commit(self.pool(), &claim, &batch, Some((&failure_code, &safe_detail))).await
        })
    }

    fn fail(
        &self,
        claim: MailPollClaim,
        retry_at: DateTime<Utc>,
        failure_code: String,
        safe_detail: String,
    ) -> ApplicationFuture<'_, AdminMailPollState> {
        Box::pin(async move {
            if !claim.validate()
                || retry_at.timestamp_nanos_opt().is_none()
                || !valid_text(&failure_code, MAX_MAIL_POLL_FAILURE_CODE_BYTES)
                || !valid_text(&safe_detail, MAX_MAIL_POLL_FAILURE_DETAIL_BYTES)
            {
                return Err(storage_error("MoogleMail 轮询失败记录参数无效"));
            }
            claim::fail(self.pool(), &claim, retry_at, &failure_code, &safe_detail).await
        })
    }
}
