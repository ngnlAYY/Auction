//! 同一邮件的领取、退回与拍卖接受事实之间的持久化互斥。
//!
//! 入站邮件、恢复动作与结算退件动作都可能对 Bot 收件箱里的同一封邮件做资产处置；三者在同一
//! world 行锁下读取相反或并行的处置意图，失败、未知的旧意图也不解除互斥（远端结果可能未知，
//! 必须先人工收口）。

use auction_application::mail::{MailDeliveryCommand, MailDeliveryOperation, MailDeliveryOrigin};

use super::errors::MailDeliveryTransactionError;
use crate::database::repository_error;

/// 携带邮件资产处置的来源；入站邮件的退回/领取与结算退件的领取互相可见。
const INTAKE_VISIBLE_ORIGINS: &[&str] = &["mail_intake", "settlement_return_action"];

/// 结算退件 Take 与任何来源对同一邮件的领取或退回互斥。
const RETURN_TAKE_EXCLUSIVE_ORIGINS: &[&str] =
    &["mail_intake", "settlement_recovery_action", "settlement_return_action"];

/// 恢复 Take 不改变既有恢复轮次规则，只额外与结算退件 Take 互斥。
const RECOVERY_TAKE_EXCLUSIVE_ORIGINS: &[&str] = &["settlement_return_action"];

/// 调用者先持有 world 行锁；已安排的退回不能被后来的拍卖录入或领取覆盖。
pub(in crate::database) async fn has_return_intent(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    world_id: &str,
    external_mail_id: &str,
) -> Result<bool, sqlx::Error> {
    has_operation(
        transaction,
        world_id,
        external_mail_id,
        &["return"],
        INTAKE_VISIBLE_ORIGINS,
        None,
    )
    .await
}

pub(super) async fn guard_disposition(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    command: &MailDeliveryCommand,
) -> Result<(), MailDeliveryTransactionError> {
    match (command.origin, command.operation) {
        (_, MailDeliveryOperation::Send) => Ok(()),
        (MailDeliveryOrigin::MailIntake, _) => guard_intake(transaction, command).await,
        (MailDeliveryOrigin::SettlementReturnAction, MailDeliveryOperation::Take) => {
            guard_exclusive_take(transaction, command, RETURN_TAKE_EXCLUSIVE_ORIGINS).await
        }
        (MailDeliveryOrigin::SettlementRecoveryAction, MailDeliveryOperation::Take) => {
            guard_exclusive_take(transaction, command, RECOVERY_TAKE_EXCLUSIVE_ORIGINS).await
        }
        _ => Ok(()),
    }
}

/// 与拍卖入站使用相同 world 行锁。读取相反意图与 INSERT 必须处于同一事务；只在应用层预读无法
/// 阻止旧租约处理器与重试处理器分别决定 Take/Return。
async fn lock_world(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    world_id: &str,
) -> Result<(), MailDeliveryTransactionError> {
    let world =
        sqlx::query_scalar::<_, String>("SELECT id FROM public.worlds WHERE id = $1 FOR UPDATE")
            .bind(world_id)
            .fetch_optional(&mut **transaction)
            .await?;
    if world.is_none() {
        return Err(repository_error("MoogleMail 资产处置世界不存在").into());
    }
    Ok(())
}

async fn guard_intake(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    command: &MailDeliveryCommand,
) -> Result<(), MailDeliveryTransactionError> {
    lock_world(transaction, &command.world_id).await?;
    let opposite: &[&str] = match command.operation {
        MailDeliveryOperation::Return => &["take"],
        MailDeliveryOperation::Take => &["return"],
        MailDeliveryOperation::Send => return Ok(()),
    };
    if has_operation(
        transaction,
        &command.world_id,
        &command.external_mail_id,
        opposite,
        INTAKE_VISIBLE_ORIGINS,
        None,
    )
    .await?
    {
        return Err(repository_error("MoogleMail 已存在相反的资产处置意图，需人工核对").into());
    }
    // 结算退件 Take 不是入站处置，但同一封邮件同样不能再被入站流程领取。
    if command.operation == MailDeliveryOperation::Take
        && has_operation(
            transaction,
            &command.world_id,
            &command.external_mail_id,
            &["take"],
            &["settlement_return_action"],
            None,
        )
        .await?
    {
        return Err(repository_error("MoogleMail 已存在结算退件领取意图，需人工核对").into());
    }
    let accepted = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
            SELECT 1 FROM public.auction_submissions AS submission
            INNER JOIN public.auctions AS auction ON auction.id = submission.auction_id
            WHERE submission.source_kind = 'mail' AND auction.world_id = $1
              AND submission.external_mail_id = $2
        )",
    )
    .bind(&command.world_id)
    .bind(&command.external_mail_id)
    .fetch_one(&mut **transaction)
    .await?;
    if (command.operation == MailDeliveryOperation::Return && accepted)
        || (command.operation == MailDeliveryOperation::Take && !accepted)
    {
        return Err(repository_error("MoogleMail 资产处置与已提交拍卖接受事实冲突").into());
    }
    Ok(())
}

/// 同一邮件只允许一个领取：除同一命令的幂等重放外，任一列出来源的领取或退回意图都失败关闭。
async fn guard_exclusive_take(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    command: &MailDeliveryCommand,
    origins: &[&str],
) -> Result<(), MailDeliveryTransactionError> {
    lock_world(transaction, &command.world_id).await?;
    if has_operation(
        transaction,
        &command.world_id,
        &command.external_mail_id,
        &["take", "return"],
        origins,
        Some(command.command_id.as_uuid()),
    )
    .await?
    {
        return Err(
            repository_error("MoogleMail 同一邮件已存在其他资产处置意图，需人工核对").into()
        );
    }
    Ok(())
}

async fn has_operation(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    world_id: &str,
    external_mail_id: &str,
    operations: &[&str],
    origins: &[&str],
    except_command: Option<uuid::Uuid>,
) -> Result<bool, sqlx::Error> {
    // failed/manual_reconcile 也不能解除互斥；远端失败可能结果未知，必须先人工收口。
    sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM public.deliveries
            WHERE origin_kind = ANY($4)
              AND payload_world = $1 AND payload->>'external_mail_id' = $2
              AND payload_operation = ANY($3)
              AND command_id IS DISTINCT FROM $5
        )",
    )
    .bind(world_id)
    .bind(external_mail_id)
    .bind(operations)
    .bind(origins)
    .bind(except_command)
    .fetch_one(&mut **transaction)
    .await
}

/// 结算退件 Take 准备前的只读互斥检查：调用方已持 world 行锁；存在其他来源对同一邮件的领取或
/// 退回意图时返回 true，由调用方把动作转人工核对而不写入投递。
pub(in crate::database) async fn settlement_return_take_conflicts(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    world_id: &str,
    external_mail_id: &str,
    command_id: uuid::Uuid,
) -> Result<bool, sqlx::Error> {
    has_operation(
        transaction,
        world_id,
        external_mail_id,
        &["take", "return"],
        RETURN_TAKE_EXCLUSIVE_ORIGINS,
        Some(command_id),
    )
    .await
}
