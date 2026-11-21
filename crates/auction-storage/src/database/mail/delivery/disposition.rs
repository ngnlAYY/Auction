//! 同一邮件的领取、退回与拍卖接受事实之间的持久化互斥。
//!
//! 入站邮件、恢复动作与结算退件动作都可能对 Bot 收件箱里的同一封邮件做资产处置；三者在同一
//! world 行锁下读取相反或并行的处置意图，失败、未知的旧意图也不解除互斥（远端结果可能未知，
//! 必须先人工收口）。唯一的交接例外只用于结算退件 Take：另一 owner 的意图已以持久事实确定从未
//! 执行（终态 failed、从未进入发送围栏、没有任何发送或领取尝试、结果元数据明确记录
//! `write_started` 与 `outcome_unknown` 都是布尔 `false`），此时邮件未被触碰，结算退件 Take 可以
//! 接手；该意图之后也不能再被投递台的普通重试复活（见 `delivery/admin`）。

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

/// 投递 `$alias` 以持久事实确定从未执行：终态 failed、从未进入发送围栏、没有发送或领取尝试、
/// 结果元数据中 `write_started` 与 `outcome_unknown` 都是明确的 JSON 布尔 `false`。结算退件 Take
/// 据此接手另一 owner 失败的同邮件意图。缺键、null 或错误类型（如字符串 `"false"`）都判为
/// “可能已执行”，与案件 owner 的重启准入同一口径；每一项都写成非空布尔，避免 NULL 让外层 `NOT`
/// 失效。
macro_rules! unexecuted_intent_sql {
    ($alias:literal) => {
        concat!(
            "(",
            $alias,
            ".status = 'failed' AND ",
            $alias,
            ".sending_at IS NULL AND ",
            $alias,
            ".external_reference IS NULL AND COALESCE(",
            $alias,
            ".result_metadata @> '{\"write_started\": false, \"outcome_unknown\": false}'::jsonb,
                       false)
              AND NOT EXISTS (SELECT 1 FROM public.delivery_take_attempts AS take_history
                              WHERE take_history.delivery_id = ",
            $alias,
            ".id)
              AND NOT EXISTS (SELECT 1 FROM public.delivery_send_attempts AS send_history
                              WHERE send_history.delivery_id = ",
            $alias,
            ".id))"
        )
    };
}

/// 结算退件 Take 对世界 `$world`、邮件 `$mail` 是否仍被其他处置意图阻断（相关 SQL 表达式）：
/// 任一来源的领取或退回意图都阻断，已确定从未执行的失败意图除外。调用方传入的世界与 MID 为
/// 列引用或参数；结算退件自身的 Take 由调用方另行排除。
macro_rules! return_take_blocked_sql {
    ($world:literal, $mail:literal) => {
        concat!(
            "EXISTS (SELECT 1 FROM public.deliveries AS intent
                     WHERE intent.origin_kind IN
                               ('mail_intake', 'settlement_recovery_action', 'settlement_return_action')
                       AND intent.payload_world = ", $world, "
                       AND intent.payload->>'external_mail_id' = ", $mail, "
                       AND intent.payload_operation IN ('take', 'return')
                       AND NOT ",
            $crate::database::mail::delivery::disposition::unexecuted_intent_sql!("intent"),
            ")"
        )
    };
}
pub(in crate::database) use return_take_blocked_sql;
pub(in crate::database) use unexecuted_intent_sql;

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
        false,
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
            guard_exclusive_take(transaction, command, RETURN_TAKE_EXCLUSIVE_ORIGINS, true).await
        }
        (MailDeliveryOrigin::SettlementRecoveryAction, MailDeliveryOperation::Take) => {
            guard_exclusive_take(transaction, command, RECOVERY_TAKE_EXCLUSIVE_ORIGINS, false).await
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
        false,
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
            false,
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

/// 同一邮件只允许一个领取：除同一命令的幂等重放外，任一列出来源的领取或退回意图都失败关闭；
/// `handover` 为真（结算退件 Take）时已确定从未执行的失败意图不再阻断。
async fn guard_exclusive_take(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    command: &MailDeliveryCommand,
    origins: &[&str],
    handover: bool,
) -> Result<(), MailDeliveryTransactionError> {
    lock_world(transaction, &command.world_id).await?;
    if has_operation(
        transaction,
        &command.world_id,
        &command.external_mail_id,
        &["take", "return"],
        origins,
        Some(command.command_id.as_uuid()),
        handover,
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
    handover: bool,
) -> Result<bool, sqlx::Error> {
    // failed/manual_reconcile 也不能解除互斥；远端失败可能结果未知，必须先人工收口。只有结算
    // 退件 Take 的交接口径（`handover`）放行已确定从未执行的失败意图。
    sqlx::query_scalar(concat!(
        "SELECT EXISTS (
            SELECT 1 FROM public.deliveries AS intent
            WHERE intent.origin_kind = ANY($4)
              AND intent.payload_world = $1 AND intent.payload->>'external_mail_id' = $2
              AND intent.payload_operation = ANY($3)
              AND intent.command_id IS DISTINCT FROM $5
              AND NOT ($6 AND ",
        unexecuted_intent_sql!("intent"),
        "))"
    ))
    .bind(world_id)
    .bind(external_mail_id)
    .bind(operations)
    .bind(origins)
    .bind(except_command)
    .bind(handover)
    .fetch_one(&mut **transaction)
    .await
}

/// 结算退件 Take 准备前的只读互斥检查：调用方已持 world 行锁；存在其他来源对同一邮件的领取或
/// 退回意图时返回 true，由调用方把动作转人工核对而不写入投递。另一 owner 已确定从未执行的失败
/// 意图按交接口径不阻断。
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
        true,
    )
    .await
}

/// 投递是入站或恢复对某封邮件的领取/退回意图，而同一邮件已有结算退件 Take（交接之后）。投递台
/// 据此拒绝重新激活该意图：交接只认“确定从未执行”，普通重试复活它会让两个 owner 各领取一次。
/// Serializable 下与交接激活互斥：双方各读对方的行并写自己的行，交错时一方以序列化冲突重试。
pub(in crate::database) async fn mail_handed_over(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    delivery_id: uuid::Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        r#"SELECT EXISTS (
               SELECT 1
               FROM public.deliveries AS intent
               JOIN public.deliveries AS owner
                 ON owner.origin_kind = 'settlement_return_action'
                AND owner.payload_operation = 'take'
                AND owner.payload_world = intent.payload_world
                AND owner.payload->>'external_mail_id' = intent.payload->>'external_mail_id'
                AND owner.id <> intent.id
               WHERE intent.id = $1
                 AND intent.origin_kind IN ('mail_intake', 'settlement_recovery_action')
                 AND intent.payload_operation IN ('take', 'return')
           )"#,
    )
    .bind(delivery_id)
    .fetch_one(&mut **transaction)
    .await
}
