//! 同轮同腿第二个不同 MID 的冲突案与资金错误的边界映射。

use auction_application::settlement::FundingError;
use auction_domain::consignment::OriginalMailKey;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::database::consignment::commands::errors::WriteError;
use crate::database::mail::originals::identity::MailKeyColumns;

/// 把第二个不同 MID 的 RTS 记为冲突案，指向已接管同轮同腿的那个 intake。
///
/// 保护不变量 4、6、8：冲突案只追加；不接管、不起写、不动资产，由人工对账。同 K 重复写入不新增。
pub(super) async fn record_second_mid(
    connection: &mut PgConnection,
    key: &OriginalMailKey,
    existing_intake_id: Uuid,
    observation_id: Uuid,
) -> Result<(), WriteError> {
    let columns = MailKeyColumns::from_key(key);
    sqlx::query(
        "INSERT INTO public.consignment_return_conflicts
            (bot_uid, world_id, external_mid, intake_id, observation_id)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT ON CONSTRAINT c6_conflict_pk DO NOTHING",
    )
    .bind(columns.bot_uid)
    .bind(columns.world_id)
    .bind(columns.external_mid)
    .bind(existing_intake_id)
    .bind(observation_id)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

/// 资金原语失败一律让整个事务回滚（起写与阻断、消费与回补必须同提或同回滚）。
///
/// 序列化冲突交还重试；其余失败关闭为“能力不可用”，不对外泄露资金细节。
pub(crate) fn funding_error(error: FundingError) -> WriteError {
    match error {
        FundingError::SerializationConflict => WriteError::RetryConflict,
        FundingError::Invalid
        | FundingError::Conflict
        | FundingError::Unavailable
        | FundingError::Unproven
        | FundingError::Unsupported
        | FundingError::Storage => {
            tracing::warn!(kind = error.safe_kind(), "退件接管的资金原语失败，整笔回滚");
            auction_application::consignment::ConsignmentError::Unavailable.into()
        }
    }
}
