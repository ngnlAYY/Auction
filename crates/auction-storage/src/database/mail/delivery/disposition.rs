//! 同一邮件的领取、退回与拍卖接受事实之间的持久化互斥。

use auction_application::mail::{MailDeliveryCommand, MailDeliveryOperation, MailDeliveryOrigin};

use super::errors::{MailDeliveryTransactionError, repository_error};

/// 调用者先持有 world 行锁；已安排的退回不能被后来的拍卖录入或领取覆盖。
pub(in crate::database) async fn has_return_intent(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    world_id: &str,
    external_mail_id: &str,
) -> Result<bool, sqlx::Error> {
    has_operation(transaction, world_id, external_mail_id, "return").await
}

pub(super) async fn guard_disposition(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    command: &MailDeliveryCommand,
) -> Result<(), MailDeliveryTransactionError> {
    if command.origin != MailDeliveryOrigin::MailIntake
        || command.operation == MailDeliveryOperation::Send
    {
        return Ok(());
    }
    // 与拍卖入站使用相同 world 行锁。读取相反意图与 INSERT 必须处于同一事务；只在应用
    // 层预读无法阻止旧租约处理器与重试处理器分别决定 Take/Return。
    let world =
        sqlx::query_scalar::<_, String>("SELECT id FROM public.worlds WHERE id = $1 FOR UPDATE")
            .bind(&command.world_id)
            .fetch_optional(&mut **transaction)
            .await?;
    if world.is_none() {
        return Err(repository_error("MoogleMail 资产处置世界不存在").into());
    }
    let opposite = match command.operation {
        MailDeliveryOperation::Return => "take",
        MailDeliveryOperation::Take => "return",
        MailDeliveryOperation::Send => return Ok(()),
    };
    if has_operation(transaction, &command.world_id, &command.external_mail_id, opposite).await? {
        return Err(repository_error("MoogleMail 已存在相反的资产处置意图，需人工核对").into());
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

async fn has_operation(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    world_id: &str,
    external_mail_id: &str,
    operation: &str,
) -> Result<bool, sqlx::Error> {
    // failed/manual_reconcile 也不能解除互斥；远端失败可能结果未知，必须先人工收口。
    sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM public.deliveries
            WHERE origin_kind = 'mail_intake'
              AND payload_world = $1 AND payload->>'external_mail_id' = $2
              AND payload_operation = $3
        )",
    )
    .bind(world_id)
    .bind(external_mail_id)
    .bind(operation)
    .fetch_one(&mut **transaction)
    .await
}
