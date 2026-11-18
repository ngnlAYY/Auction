//! 人工 Sent 责任事实与原投递同事务绑定；现金只收口原未知支出，不创建来源或收款。

use auction_application::core::ApplicationError;
use auction_application::core::ApplicationFuture;
use auction_application::delivery::{
    DeliverySentConfirmationPort, ManualSentEvidence, SentConfirmationCommand,
    SentConfirmationPreview, SentConfirmationResult, SentObservedAttachments,
};
use auction_application::settlement::FundingError;
use auction_time::Timestamp;
use serde::Deserialize;
use serde::de::Error as _;
use serde_json::Value;
use uuid::Uuid;

use crate::database::DatabaseStore;

mod snapshot;
mod transaction;

pub(in crate::database) use snapshot::admission_rows;

pub(in crate::database) const SENT_OBSERVATION_KINDS: [&str; 2] =
    ["mailbox_sweep", "sent_reference_sighting"];

/// Sent 观察的有效身份、版本与实见资产。
pub(in crate::database) struct DecodedSentObservation<'a> {
    pub mid: &'a str,
    pub observed_at: Option<Timestamp>,
    pub attachments: SentObservedAttachments,
}

/// 同一持久观察由详情投影与确认事务按同一口径读取。
pub(in crate::database) fn decode_sent_observation(
    observation: &Value,
) -> Result<DecodedSentObservation<'_>, serde_json::Error> {
    let mid = observation
        .get("mid")
        .or_else(|| observation.get("external_mail_id"))
        .and_then(Value::as_str)
        .filter(|mid| auction_domain::valid_positive_decimal(mid, 64))
        .ok_or_else(|| serde_json::Error::custom("Sent 观察的 MID 无效"))?;
    // 仅缺字段属于历史格式；格式损坏的版本不能为附件提供证明（不变量 6）。
    let observed_at = observation
        .get("observed_at")
        .map(|value| {
            let time = value.as_str().ok_or_else(|| {
                serde_json::Error::custom("Sent 观察的 observed_at 必须为时间文本")
            })?;
            Timestamp::parse_from_rfc3339(time).map_err(|error| {
                serde_json::Error::custom(format_args!(
                    "Sent 观察的 observed_at 无效（{:?}）",
                    error.kind()
                ))
            })
        })
        .transpose()?;
    let attachments = SentObservedAttachments::deserialize(observation)?;
    let count = observation
        .get("remaining_equipment_count")
        .filter(|count| !count.is_null())
        .map(|count| {
            count
                .as_i64()
                .and_then(|count| usize::try_from(count).ok())
                .ok_or_else(|| serde_json::Error::custom("Sent 观察的剩余装备数量无效"))
        })
        .transpose()?;
    let valid_ids = |ids: &[String]| {
        ids.iter().all(|id| auction_domain::valid_positive_decimal(id, 64))
            && ids.windows(2).all(|pair| pair[0] < pair[1])
    };
    if !attachments.remaining_equipment_ids.as_deref().is_none_or(valid_ids)
        || !attachments
            .pre_collection_equipment_ids
            .as_deref()
            .is_none_or(|ids| !ids.is_empty() && valid_ids(ids))
        || attachments
            .remaining_equipment_ids
            .as_ref()
            .zip(count)
            .is_some_and(|(ids, count)| ids.len() != count)
    {
        return Err(serde_json::Error::custom("Sent 观察的附件清单无效"));
    }
    Ok(DecodedSentObservation { mid, observed_at, attachments })
}

enum Error {
    Sql(sqlx::Error),
    Funding(FundingError),
    Application(ApplicationError),
    Json(serde_json::Error),
    Rejected,
}

impl Error {
    fn retryable(&self) -> bool {
        match self {
            Self::Sql(error) => crate::database::observed_serialization_conflict(error),
            Self::Funding(FundingError::SerializationConflict) => true,
            _ => false,
        }
    }

    /// SQL 错误对外统一为依赖不可用，丢弃前记录安全投影。
    fn application(self) -> ApplicationError {
        match self {
            Self::Application(error) => error,
            Self::Json(error) => ApplicationError::Serialization(error),
            Self::Rejected => ApplicationError::FulfillmentDependencyMismatch,
            Self::Sql(error) => {
                crate::database::log_discarded_sql_error(
                    &error,
                    "delivery_sent_confirmation_storage_failed",
                    "人工 Sent 确认数据库操作失败",
                );
                ApplicationError::FulfillmentDependencyUnavailable
            }
            Self::Funding(_) => ApplicationError::FulfillmentDependencyUnavailable,
        }
    }
}

impl From<sqlx::Error> for Error {
    fn from(error: sqlx::Error) -> Self {
        Self::Sql(error)
    }
}
impl From<FundingError> for Error {
    fn from(error: FundingError) -> Self {
        Self::Funding(error)
    }
}
impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl DeliverySentConfirmationPort for DatabaseStore {
    fn preview_sent_confirmation<'a>(
        &'a self,
        delivery_id: Uuid,
        evidence: ManualSentEvidence,
        evidence_reference: Option<&'a str>,
    ) -> ApplicationFuture<'a, SentConfirmationPreview> {
        Box::pin(async move {
            evidence.validate()?;
            if delivery_id.is_nil() {
                return Err(ApplicationError::FulfillmentDependencyMismatch);
            }
            let operation = async {
                let mut tx = self
                    .pool()
                    .begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY")
                    .await?;
                sqlx::query("SET LOCAL statement_timeout='15s'").execute(&mut *tx).await?;
                let snapshot =
                    snapshot::load(&mut tx, delivery_id, &evidence, evidence_reference).await?;
                tx.rollback().await?;
                Ok::<_, Error>(snapshot.preview)
            }
            .await;
            operation.map_err(Error::application)
        })
    }

    fn confirm_sent<'a>(
        &'a self,
        command: SentConfirmationCommand,
        evidence_reference: Option<&'a str>,
    ) -> ApplicationFuture<'a, SentConfirmationResult> {
        Box::pin(async move {
            command.validate()?;
            let command = &command;
            crate::database::retry_conflicts(
                self.pool(),
                auction_observability::DatabaseTransactionOwner::SentConfirmation,
                crate::database::SERIALIZABLE_ATTEMPTS,
                Error::retryable,
                || async move {
                    let mut tx =
                        self.pool().begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await?;
                    sqlx::query("SET LOCAL transaction_timeout='30s'").execute(&mut *tx).await?;
                    crate::database::lock_command_id(&mut tx, command.command_id).await?;
                    if let Some(result) = transaction::replay(&mut tx, command).await? {
                        tx.commit().await?;
                        return Ok(result);
                    }
                    let result = transaction::apply(&mut tx, command, evidence_reference).await?;
                    tx.commit().await?;
                    Ok::<_, Error>(result)
                },
            )
            .await
            .map_err(Error::application)
        })
    }
}
