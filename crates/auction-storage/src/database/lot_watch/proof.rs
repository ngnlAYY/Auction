//! watch owner 的独立发送资格；不借用论坛命令或任意邮件模板证明来源。

use crate::database::TrackedTransaction as Transaction;
use sqlx::Postgres;
use uuid::Uuid;

// SQL 片段只在本 owner 定义，由 delivery 的领取和 sending fence 共用。
macro_rules! delivery_watch_gate {
    () => {
        r#"(
        (deliveries.origin_kind <> 'lot_watch' AND
         COALESCE(deliveries.payload ->> 'template', '') <> 'lot_watch')
        OR (
          deliveries.origin_kind = 'lot_watch'
          AND deliveries.origin_id = 'lot_watch:' || deliveries.command_id::text
          AND deliveries.payload ->> 'template' = 'lot_watch'
          AND deliveries.payload ->> 'operation' = 'send'
          AND EXISTS (
            SELECT 1 FROM public.lot_watch_notifications notification
            JOIN public.lot_watches watch ON watch.user_id = notification.user_id
                AND watch.lot_id = notification.lot_id
            JOIN public.lot_watch_leadership leadership ON leadership.lot_id = notification.lot_id
            WHERE notification.delivery_id = deliveries.id
              AND notification.channel = 'mm'
              AND notification.command_id = deliveries.command_id
              AND notification.request_digest = deliveries.request_digest
              AND notification.recipient = deliveries.recipient
              AND notification.payload = deliveries.payload
              AND notification.invalidated_at IS NULL
              AND watch.watched AND watch.channel = 'mm'
              AND watch.version >= notification.watch_version
              AND leadership.leader_user_id IS DISTINCT FROM notification.user_id
          )
        )
    )"#
    };
}
pub(crate) use delivery_watch_gate;

// 已失效是不可逆事实，delivery owner 可无 lot 锁收口尚未发送的任务；不删除来源证明。
macro_rules! delivery_watch_invalidated {
    () => {
        r#"(deliveries.origin_kind = 'lot_watch' AND EXISTS (
            SELECT 1 FROM public.lot_watch_notifications notification
            WHERE notification.delivery_id = deliveries.id AND notification.invalidated_at IS NOT NULL
        ))"#
    };
}
pub(crate) use delivery_watch_invalidated;

/// Admin 重试/释放也只能使用同一来源证明；调用方先持有 lot 锁再锁 delivery。
pub(crate) async fn delivery_eligible(
    transaction: &mut Transaction<'_, Postgres>,
    delivery_id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(concat!(
        "SELECT ",
        delivery_watch_gate!(),
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
