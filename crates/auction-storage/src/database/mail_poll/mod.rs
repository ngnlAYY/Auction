//! MoogleMail Worker 持久化适配器装配。
//!
//! 状态与租约位于 `state.rs`，轮询 claim/commit/fail 位于 `poll.rs`，入站事实和邮件命令
//! 幂等写入位于 `inbound.rs`。外部 GET 不在数据库事务内执行；Worker 只有拿到 claim 后才
//! 调用 gateway，并在 commit 中原子写入事实与 cursor。

use auction_application::{
    AdminMailPollState, ApplicationFuture, MAX_MAIL_POLL_BATCH_SIZE,
    MAX_MAIL_POLL_FAILURE_CODE_BYTES, MAX_MAIL_POLL_FAILURE_DETAIL_BYTES, MailPollBatch,
    MailPollClaim, MailPollCompletion, MailPollStore,
};
use chrono::{DateTime, Utc};

use super::DatabaseStore;

#[cfg(feature = "postgres")]
mod inbound;
#[cfg(feature = "postgres")]
mod poll;
#[cfg(feature = "postgres")]
mod state;

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
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    inbound::lookup_completion(
                        store.pool(),
                        &mailbox_key,
                        &world_id,
                        &external_mail_id,
                        received_at,
                    )
                    .await
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(capability_error()),
            }
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
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    state::ensure(
                        store.pool(),
                        &mailbox_key,
                        now,
                        scheduled_next_at,
                        retry_maximum_seconds,
                    )
                    .await
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(capability_error()),
            }
        })
    }

    fn claim_due(
        &self,
        mailbox_key: String,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        limit: u16,
        _force: bool,
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
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    poll::claim(
                        store.pool(),
                        &mailbox_key,
                        &worker_id,
                        now,
                        lease_until,
                        limit,
                        _force,
                    )
                    .await
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(capability_error()),
            }
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
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => poll::commit(store.pool(), &claim, &batch).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(capability_error()),
            }
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
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => state::renew(store.pool(), claim, now, lease_until).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(capability_error()),
            }
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
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    poll::fail(store.pool(), &claim, retry_at, &failure_code, &safe_detail).await
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(capability_error()),
            }
        })
    }
}

fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

fn storage_error(message: &str) -> auction_application::ApplicationError {
    auction_application::RepositoryError::Failed(message.to_owned()).into()
}

#[cfg(feature = "sqlite")]
fn capability_error() -> auction_application::ApplicationError {
    auction_application::ApplicationError::CapabilityUnavailable(
        "MoogleMail Worker 轮询仅支持 PostgreSQL 完整档",
    )
}
