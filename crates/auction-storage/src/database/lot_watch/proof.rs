//! watch owner 的独立发送资格；不借用论坛命令或任意邮件模板证明来源。

use sqlx::Postgres;
use uuid::Uuid;

use crate::database::TrackedTransaction as Transaction;

// SQL 片段只在本 owner 定义，由 delivery 的领取和 sending fence 共用。
macro_rules! delivery_watch_gate {
    () => {
        r#"(
        (deliveries.origin_kind <> 'lot_watch' AND
         COALESCE(deliveries.payload_template, '') <> 'lot_watch')
        OR (
          deliveries.origin_kind = 'lot_watch'
          AND deliveries.origin_id = 'lot_watch:' || deliveries.command_id::text
          AND deliveries.payload_template = 'lot_watch'
          AND deliveries.payload_operation = 'send'
          AND EXISTS (
            SELECT 1 FROM public.lot_watch_notifications notification
            JOIN public.lot_watches watch ON watch.user_id = notification.user_id
                AND watch.lot_id = notification.lot_id
            JOIN public.lots lot ON lot.id = notification.lot_id
            WHERE notification.delivery_id = deliveries.id
              AND notification.channel = 'mm'
              AND notification.command_id = deliveries.command_id
              AND notification.request_digest = deliveries.request_digest
              AND notification.recipient = deliveries.recipient
              AND notification.payload = deliveries.payload
              AND notification.invalidated_at IS NULL
              AND watch.watched AND watch.channel = 'mm'
              AND watch.version >= notification.watch_version
              AND lot.leader_user_id IS DISTINCT FROM notification.user_id
              AND lot.effective_end_at IS NOT NULL
              AND lot.effective_end_at > clock_timestamp()
          )
        )
    )"#
    };
}
pub(crate) use delivery_watch_gate;

// 已失效是不可逆事实，delivery owner 可无 lot 锁收口尚未发送的任务；不删除来源证明。
// 提醒资格同时随 lot 有效截止结束（决策 14）：截止后尚未开始的提醒按同一预检退役语义收口，
// 不再进入 sending 围栏；已经 sending 的行不在此列，其外部结果仍由 delivery owner 保留。
macro_rules! delivery_watch_expired {
    () => {
        r#"(deliveries.origin_kind = 'lot_watch' AND EXISTS (
            SELECT 1 FROM public.lot_watch_notifications notification
            JOIN public.lots lot ON lot.id = notification.lot_id
            WHERE notification.delivery_id = deliveries.id
              AND (
                  notification.invalidated_at IS NOT NULL
                  OR (lot.effective_end_at IS NOT NULL AND lot.effective_end_at <= clock_timestamp())
              )
        ))"#
    };
}
pub(crate) use delivery_watch_expired;

// 玩家改名后，提醒的冻结收件人不再是该用户当前名称，远端已不存在该收件人。
// 未开始发送的提醒按预检退役收口并记录原因；已 sending 的行仍由 delivery owner 保留未知结果。
macro_rules! delivery_watch_renamed {
    () => {
        r#"(deliveries.origin_kind = 'lot_watch' AND EXISTS (
            SELECT 1 FROM public.lot_watch_notifications notification
            JOIN public.users recipient ON recipient.id = notification.user_id
            WHERE notification.delivery_id = deliveries.id
              AND recipient.username IS DISTINCT FROM notification.recipient
        ))"#
    };
}
pub(crate) use delivery_watch_renamed;

/// Admin 重试/释放也只能使用同一来源证明；调用方先持有 lot 锁再锁 delivery。
/// 收件人已改名的提醒已按预检退役，重放只会再次失败，因此不提供重试资格。
pub(crate) async fn delivery_eligible(
    transaction: &mut Transaction<'_, Postgres>,
    delivery_id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(concat!(
        "SELECT ",
        delivery_watch_gate!(),
        " AND NOT ",
        delivery_watch_renamed!(),
        " FROM public.deliveries AS deliveries WHERE deliveries.id = $1"
    ))
    .bind(delivery_id)
    .fetch_one(&mut **transaction)
    .await
}

/// sending 必须先取 lot 锁再取 delivery 锁；取消/重新领先与开始发送有明确线性化点。领取阶段只做
/// 只读证明、不倒取 lot，最终开始发送再次在锁内复核。SQLSTATE 保留给调用方重试边界分类。
pub(crate) async fn lock_delivery_lot(
    transaction: &mut Transaction<'_, Postgres>,
    delivery_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "SELECT lot.id FROM public.lots lot
        JOIN public.lot_watch_notifications notification ON notification.lot_id = lot.id
        WHERE notification.delivery_id = $1 FOR UPDATE OF lot",
    )
    .bind(delivery_id)
    .fetch_optional(&mut **transaction)
    .await?;
    Ok(())
}
