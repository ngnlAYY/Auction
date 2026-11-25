//! 损失案件的开案：案件、版本 1 的开案记录与卖家可查回执同事务、各只产生一次。
//!
//! 保护不变量 3、8：案件按 (position, 损失事实, 卖家) 唯一；同一确认命令重放返回原案件，
//! 另一条命令想对同一事实再开案判 `disposition_conflict`，不新建、不改旧案。
//! 本原语由 7C-4b 的确认命令在写入唯一损失事实之后、同一事务内调用；开案同事务登记卖家的 `loss_case` 权利。

use auction_application::consignment::{ConsignmentError, DepositBusinessRejection};
use auction_domain::CommandId;
use auction_time::Timestamp;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::rights::register_loss_case_right_in_tx;

/// 已写入的损失事实的身份；案件必须与它的 position 和卖家一致。
pub(crate) struct LossFactRef {
    /// 损失事实 UUID。
    pub(crate) loss_fact_id: Uuid,
    /// position UUID。
    pub(crate) position_id: Uuid,
    /// 卖家本地用户 UUID。
    pub(crate) seller_user_id: Uuid,
}

/// 开案产生的三个标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenedLossCase {
    /// 案件 UUID。
    pub case_id: Uuid,
    /// 版本 1 的开案记录 UUID。
    pub outcome_id: Uuid,
    /// 卖家可查回执 UUID。
    pub seller_receipt_id: Uuid,
}

/// 为损失事实开一个案件；调用方提供的三个 UUID 只在首次开案时使用。
pub(crate) async fn open_loss_case(
    connection: &mut PgConnection,
    fact: &LossFactRef,
    command_id: CommandId,
    ids: OpenedLossCase,
    now: Timestamp,
) -> Result<OpenedLossCase, WriteError> {
    let fact_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.consignment_loss_facts
                        WHERE loss_fact_id = $1 AND position_id = $2 AND seller_user_id = $3)",
    )
    .bind(fact.loss_fact_id)
    .bind(fact.position_id)
    .bind(fact.seller_user_id)
    .fetch_one(&mut *connection)
    .await?;
    if !fact_exists {
        return Err(ConsignmentError::Deposit(DepositBusinessRejection::SourceMismatch).into());
    }

    let existing = sqlx::query(
        "SELECT c.case_id, c.origin_command_id, o.outcome_id, r.receipt_id
         FROM public.consignment_loss_cases AS c
         JOIN public.consignment_loss_case_outcomes AS o
           ON o.case_id = c.case_id AND o.case_version = 1
         JOIN public.consignment_loss_receipts AS r ON r.outcome_id = o.outcome_id
         WHERE c.position_id = $1 AND c.loss_fact_id = $2 AND c.seller_user_id = $3",
    )
    .bind(fact.position_id)
    .bind(fact.loss_fact_id)
    .bind(fact.seller_user_id)
    .fetch_optional(&mut *connection)
    .await?;
    if let Some(row) = existing {
        if row.try_get::<Uuid, _>("origin_command_id")? != command_id.as_uuid() {
            return Err(
                ConsignmentError::Deposit(DepositBusinessRejection::DispositionConflict).into()
            );
        }
        return Ok(OpenedLossCase {
            case_id: row.try_get("case_id")?,
            outcome_id: row.try_get("outcome_id")?,
            seller_receipt_id: row.try_get("receipt_id")?,
        });
    }

    sqlx::query(
        "INSERT INTO public.consignment_loss_cases
            (case_id, loss_fact_id, position_id, seller_user_id, origin_command_id, created_at)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(ids.case_id)
    .bind(fact.loss_fact_id)
    .bind(fact.position_id)
    .bind(fact.seller_user_id)
    .bind(command_id.as_uuid())
    .bind(now)
    .execute(&mut *connection)
    .await?;
    sqlx::query(
        "INSERT INTO public.consignment_loss_case_outcomes
            (outcome_id, case_id, case_version, outcome, basis_id, private_note, command_id,
             occurred_at)
         VALUES ($1, $2, 1, 'opened', NULL, NULL, $3, $4)",
    )
    .bind(ids.outcome_id)
    .bind(ids.case_id)
    .bind(command_id.as_uuid())
    .bind(now)
    .execute(&mut *connection)
    .await?;
    sqlx::query(
        "INSERT INTO public.consignment_loss_receipts
            (receipt_id, case_id, outcome_id, seller_user_id, recorded_at)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(ids.seller_receipt_id)
    .bind(ids.case_id)
    .bind(ids.outcome_id)
    .bind(fact.seller_user_id)
    .bind(now)
    .execute(&mut *connection)
    .await?;
    // 卖家对该 position 的只读 `loss_case` 权利与开案同事务登记（重放走上面的既有案件分支，不再登记）。
    register_loss_case_right_in_tx(connection, fact.position_id, command_id.as_uuid(), now).await?;
    Ok(ids)
}
