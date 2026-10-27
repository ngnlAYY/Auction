//! PostgreSQL 竞价 realtime 事件追加。

use auction_application::format_public_time_wire;

use super::super::mapping::*;
use super::super::*;

/// 追加一条 `lot.bid_accepted`。计划已校验全部事实都属于 `final_lot`，拍卖归属经调用方持有的
/// `auction_id` 缓存只读取一次；后续事实持有 stream 行锁时不再重复查询。
pub(in crate::database::competition) async fn insert_realtime_event(
    transaction: &mut PostgresCompetitionTransaction,
    auction_id: &mut Option<Uuid>,
    bid: &AcceptedBid,
) -> Result<(), CompetitionTransactionError> {
    let transaction = transaction.transaction_mut()?;
    let auction_id = match *auction_id {
        Some(auction_id) => auction_id,
        None => {
            let lot_auction_id: Uuid =
                sqlx::query_scalar("SELECT auction_id FROM public.lots WHERE id = $1")
                    .bind(bid.lot_id.as_uuid())
                    .fetch_one(&mut **transaction)
                    .await
                    .map_err(map_sql_error)?;
            *auction_id = Some(lot_auction_id);
            lot_auction_id
        }
    };
    let stream_key = format!("auction:{auction_id}");
    let payload = json!({
        "auction_id": auction_id,
        "lot_id": bid.lot_id.as_uuid(),
        "bid_id": bid.id.as_uuid(),
        "amount": bid.amount.value(),
        "effective_end": format_public_time_wire(bid.effective_end_at),
        "state_version": bid.state_version,
        "fact_index": bid.fact_index,
        "mechanism": bid_mechanism_name(bid.mechanism),
    });
    let sequence =
        crate::database::realtime::allocate_stream_sequence(transaction, &stream_key, None)
            .await
            .map_err(map_sql_error)?;
    sqlx::query(
        r#"INSERT INTO public.realtime_events
            (event_id, stream_key, stream_sequence, kind, schema_version, public_payload,
             state_version, origin_command_id, origin_bid_id, origin_fact_index)
         VALUES ($1, $2, $3, 'lot.bid_accepted', 2, $4, $5, $6, $7, $8)"#,
    )
    .bind(Uuid::new_v4())
    .bind(&stream_key)
    .bind(sequence)
    .bind(payload)
    .bind(i64_version(bid.state_version, "实时事件状态版本")?)
    .bind(bid.command_id.as_uuid())
    .bind(bid.id.as_uuid())
    .bind(i32_version(bid.fact_index, "实时事件事实序号")?)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}
