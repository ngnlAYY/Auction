//! PostgreSQL 竞价 realtime 事件追加。

use super::super::mapping::*;
use super::super::*;

pub(in crate::database::competition) async fn insert_realtime_event(
    transaction: &mut PostgresCompetitionTransaction,
    bid: &AcceptedBid,
) -> Result<(), CompetitionTransactionError> {
    let transaction = transaction.transaction_mut()?;
    let auction_id: Uuid = sqlx::query_scalar("SELECT auction_id FROM public.lots WHERE id = $1")
        .bind(bid.lot_id.as_uuid())
        .fetch_one(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
    let stream_key = format!("auction:{auction_id}");
    let payload = json!({
        "auction_id": auction_id,
        "lot_id": bid.lot_id.as_uuid(),
        "bid_id": bid.id.as_uuid(),
        "amount": bid.amount.value(),
        "effective_end": bid.effective_end_at,
        "state_version": bid.state_version,
        "fact_index": bid.fact_index,
        "mechanism": bid_mechanism_name(bid.mechanism),
    });
    sqlx::query(
        r#"INSERT INTO public.realtime_streams (stream_key) VALUES ($1)
         ON CONFLICT (stream_key) DO NOTHING"#,
    )
    .bind(&stream_key)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let sequence: i64 = sqlx::query_scalar(
        "SELECT next_sequence FROM public.realtime_streams WHERE stream_key = $1 FOR UPDATE",
    )
    .bind(&stream_key)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    sqlx::query(
        r#"UPDATE public.realtime_streams SET next_sequence = next_sequence + 1,
         updated_at = CURRENT_TIMESTAMP WHERE stream_key = $1"#,
    )
    .bind(&stream_key)
    .execute(&mut **transaction)
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
