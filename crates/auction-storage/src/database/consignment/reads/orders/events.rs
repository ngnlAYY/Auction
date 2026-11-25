//! 订单阶段事件：与资金事实或购买事实同事务写入 `realtime_events`（流 `consignment_order:<contract_id>`）。
//!
//! 保护不变量 3：事件与它所描述的订单事实在同一事务内提交，事务回滚则两者都不落库。同一入口还在
//! 同事务里按新阶段刷新该合同的买卖双方权利状态，所以权利的动作子集与事件描述的阶段一同提交、一同
//! 回滚。事件只是“订单阶段变了”的内部通知，负载只有合同 ID 与阶段码；Public relay 只扫描 `auction:` 前缀的流，
//! 这个流没有公开订阅与回放，不含金额、预约码、装备 key 与对方账号。

use auction_time::Timestamp;
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use super::{ORDER_STREAM_PREFIX, load_orders, protocol};
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::rights::{
    contract_right_ids_in_tx, lock_right_in_tx, refresh_right_state_in_tx,
};
use crate::database::realtime::{SequencedEvent, append_sequenced_event};

/// 事件 kind；负载字段见 `docs/contracts/realtime.md` 的“订单阶段事件”。
const ORDER_STAGE_KIND: &str = "consignment.order_stage_changed";

/// 订单阶段变更的唯一同事务入口：追加一条订单阶段事件，再对该合同名下的权利（买方 / 卖方）逐个
/// 刷新状态。调用方必须已在持锁的事务里写完本次变化，且由调用方提交或回滚整笔事务。
///
/// 保护不变量 2、3、8：事件与权利新版本都读自同一事务内的最新事实，与本次变化同提交、同回滚；
/// 权利状态只追加新版本，终态不再改动（见 `refresh_right_state_in_tx`）。
///
/// 锁序：业务行（调用方已持有）→ 权利锁（按 `right_id` 升序，与 `refresh_contract_rights`、续办同一把
/// advisory 锁）。先取齐该合同全部权利锁再刷新，所以“读最新版本 → 追加下一版本”不会与并发刷新交错，
/// 不会重号，也不会因同版本主键冲突而丢掉本次刷新；权利事务持锁后只读不锁业务行，不成环。
pub(crate) async fn record_order_stage_change_in_tx(
    conn: &mut PgConnection,
    contract_id: Uuid,
    now: Timestamp,
) -> Result<(), WriteError> {
    append_order_stage_event_in_tx(&mut *conn, contract_id).await?;
    // 已按 right_id 升序列出。
    let rights = contract_right_ids_in_tx(&mut *conn, contract_id).await?;
    for right in &rights {
        lock_right_in_tx(&mut *conn, *right).await?;
    }
    for right in rights {
        refresh_right_state_in_tx(&mut *conn, right, now).await?;
    }
    Ok(())
}

/// 读取该合同此刻的订单阶段并追加一条事件。调用方必须已在持锁的事务里写完本次变化：
/// 投影读到的是同一事务内的最新事实。
async fn append_order_stage_event_in_tx(
    conn: &mut PgConnection,
    contract_id: Uuid,
) -> Result<(), sqlx::Error> {
    let mut orders = load_orders(&mut *conn, &[contract_id]).await?;
    let Some(order) = orders.pop() else {
        return Err(protocol("订单阶段事件的合同不存在"));
    };
    let stages = order.core.stages;
    let stream_key = format!("{ORDER_STREAM_PREFIX}{contract_id}");
    // 事件的 state_version 记录取号前的流头部加一，只作排序提示；序号本身由流计数行锁串行分配。
    let head: Option<i64> = sqlx::query_scalar(
        "SELECT next_sequence - 1 FROM public.realtime_streams WHERE stream_key = $1",
    )
    .bind(&stream_key)
    .fetch_optional(&mut *conn)
    .await?;
    append_sequenced_event(
        conn,
        SequencedEvent {
            event_id: Uuid::new_v4(),
            stream_key: &stream_key,
            kind: ORDER_STAGE_KIND,
            schema_version: 1,
            payload: json!({
                "contract_id": contract_id,
                "order_state": stages.order_state.as_str(),
                "delivery_stage": stages.delivery.as_str(),
                "payment_stage": stages.payment.as_str(),
                "payout_stage": stages.payout.as_str(),
                "refund_stage": stages.refund.as_str(),
            }),
            state_version: head.unwrap_or(0) + 1,
            origin_command_id: None,
            created_at: None,
        },
    )
    .await
}
