//! 损失案件人工结果的追加：按已提交案件版本加一，每条结果同事务带一份卖家回执。
//!
//! 保护不变量 1、6、8：同一命令 ID 同请求重放返回原结果，同 ID 异请求为 `command_id_conflict`；
//! 版本落后判 `stale_version`，已办结的案件不再接受结果；追加行只有状态与说明，没有任何金额、
//! 付款或发行列，也不写任何资金表（人工结果不授予自由现金写入权，决定 22）。

use auction_application::consignment::{
    ConsignmentError, DepositBusinessRejection, LossCaseOutcome, LossCaseOutcomeReceipt,
};
use auction_domain::CommandId;
use auction_time::Timestamp;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use crate::database::consignment::commands::errors::WriteError;

/// 追加一条人工结果所需的输入；调用方负责命令账本与 Admin 身份。
pub(crate) struct LossOutcomeInput<'a> {
    /// 结果记录 UUID。
    pub(crate) outcome_id: Uuid,
    /// 卖家可查回执 UUID。
    pub(crate) seller_receipt_id: Uuid,
    /// 案件 UUID。
    pub(crate) case_id: Uuid,
    /// 目标 position，必须与案件一致。
    pub(crate) position_id: Uuid,
    /// 调用方已提交的案件版本。
    pub(crate) expected_case_version: u64,
    /// 结果种类。
    pub(crate) outcome: LossCaseOutcome,
    /// 依据 UUID。
    pub(crate) basis_id: Uuid,
    /// 规范私有说明。
    pub(crate) private_note: &'a str,
    /// Admin 命令。
    pub(crate) command_id: CommandId,
    /// 数据库锁后时刻。
    pub(crate) now: Timestamp,
}

/// 追加一条人工结果并返回回执。
pub(crate) async fn append_loss_case_outcome(
    connection: &mut PgConnection,
    input: &LossOutcomeInput<'_>,
) -> Result<LossCaseOutcomeReceipt, WriteError> {
    let command_id = input.command_id;
    // 命令账本之前的第一道幂等：同命令已有结果行时，只在请求逐项相同才返回原回执。
    let replay = sqlx::query(
        "SELECT o.outcome_id, o.case_id, o.case_version, o.outcome, o.basis_id, o.private_note,
                r.receipt_id
         FROM public.consignment_loss_case_outcomes AS o
         JOIN public.consignment_loss_receipts AS r ON r.outcome_id = o.outcome_id
         WHERE o.command_id = $1",
    )
    .bind(command_id.as_uuid())
    .fetch_optional(&mut *connection)
    .await?;
    if let Some(row) = replay {
        let same = row.try_get::<Uuid, _>("case_id")? == input.case_id
            && row.try_get::<String, _>("outcome")? == input.outcome.as_str()
            && row.try_get::<Option<Uuid>, _>("basis_id")? == Some(input.basis_id)
            && row.try_get::<Option<String>, _>("private_note")?.as_deref()
                == Some(input.private_note)
            // 追加行的版本恒为期望版本加一，故重放必须带同一期望版本。
            && row.try_get::<i64, _>("case_version")?.cast_unsigned()
                == input.expected_case_version.saturating_add(1);
        if !same {
            return Err(ConsignmentError::CommandIdConflict.into());
        }
        return Ok(LossCaseOutcomeReceipt::new(
            command_id,
            input.case_id,
            row.try_get::<i64, _>("case_version")?.cast_unsigned(),
            row.try_get("outcome_id")?,
            row.try_get("receipt_id")?,
        ));
    }

    let case = sqlx::query(
        "SELECT seller_user_id FROM public.consignment_loss_cases
         WHERE case_id = $1 AND position_id = $2",
    )
    .bind(input.case_id)
    .bind(input.position_id)
    .fetch_optional(&mut *connection)
    .await?;
    let Some(case) = case else {
        return Err(ConsignmentError::Deposit(DepositBusinessRejection::SourceMismatch).into());
    };
    let seller_user_id: Uuid = case.try_get("seller_user_id")?;

    // 案件不加行锁（运行账号对案件表只有 SELECT / INSERT）：并发追加由 Serializable 与
    // (case_id, case_version) 唯一约束仲裁，落败方整事务重试后读到新版本，判 stale_version。
    let latest = sqlx::query(
        "SELECT case_version, outcome FROM public.consignment_loss_case_outcomes
         WHERE case_id = $1 ORDER BY case_version DESC LIMIT 1",
    )
    .bind(input.case_id)
    .fetch_one(&mut *connection)
    .await?;
    let current_version = latest.try_get::<i64, _>("case_version")?.cast_unsigned();
    if current_version != input.expected_case_version {
        return Err(ConsignmentError::Deposit(DepositBusinessRejection::StaleVersion).into());
    }
    if latest.try_get::<String, _>("outcome")? == LossCaseOutcome::Closed.as_str() {
        return Err(ConsignmentError::Deposit(DepositBusinessRejection::ActionNotAllowed).into());
    }
    let next_version = current_version
        .checked_add(1)
        .ok_or_else(|| WriteError::invariant("consignment_loss_case_version_range"))?;

    sqlx::query(
        "INSERT INTO public.consignment_loss_case_outcomes
            (outcome_id, case_id, case_version, outcome, basis_id, private_note, command_id,
             occurred_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(input.outcome_id)
    .bind(input.case_id)
    .bind(next_version.cast_signed())
    .bind(input.outcome.as_str())
    .bind(input.basis_id)
    .bind(input.private_note)
    .bind(command_id.as_uuid())
    .bind(input.now)
    .execute(&mut *connection)
    .await?;
    sqlx::query(
        "INSERT INTO public.consignment_loss_receipts
            (receipt_id, case_id, outcome_id, seller_user_id, recorded_at)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(input.seller_receipt_id)
    .bind(input.case_id)
    .bind(input.outcome_id)
    .bind(seller_user_id)
    .bind(input.now)
    .execute(connection)
    .await?;
    Ok(LossCaseOutcomeReceipt::new(
        command_id,
        input.case_id,
        next_version,
        input.outcome_id,
        input.seller_receipt_id,
    ))
}
