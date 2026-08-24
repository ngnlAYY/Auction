//! SQLite 竞价 realtime 事件追加。

use super::super::mapping::*;
use super::super::*;

pub(in crate::database::sqlite::competition) async fn insert_realtime_event(
    transaction: &mut SqliteCompetitionTransaction,
    bid: &AcceptedBid,
) -> Result<(), CompetitionTransactionError> {
    let connection = transaction.connection_mut()?;
    let auction_id = uuid_from_blob(
        sqlx::query_scalar::<_, Vec<u8>>("SELECT auction_id FROM lots WHERE id = ?")
            .bind(uuid_blob(bid.lot_id.as_uuid()))
            .fetch_one(&mut *connection)
            .await
            .map_err(map_sql_error)?,
        "实时事件拍卖标识",
    )?;
    let stream_key = format!("auction:{auction_id}");
    let payload = serde_json::to_string(&json!({
        "auction_id": auction_id,
        "lot_id": bid.lot_id.as_uuid(),
        "bid_id": bid.id.as_uuid(),
        "amount": bid.amount.value(),
        "effective_end": bid.effective_end_at,
        "state_version": bid.state_version,
        "fact_index": bid.fact_index,
        "mechanism": bid_mechanism_name(bid.mechanism),
    }))
    .map_err(|_| invalid_record())?;
    sqlx::query(
        "INSERT INTO realtime_streams (stream_key, next_sequence, updated_at)
         VALUES (?, 1, CURRENT_TIMESTAMP)
         ON CONFLICT(stream_key) DO NOTHING",
    )
    .bind(&stream_key)
    .execute(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    let sequence: i64 =
        sqlx::query_scalar("SELECT next_sequence FROM realtime_streams WHERE stream_key = ?")
            .bind(&stream_key)
            .fetch_one(&mut *connection)
            .await
            .map_err(map_sql_error)?;
    sqlx::query(
        "UPDATE realtime_streams SET next_sequence = next_sequence + 1,
         updated_at = CURRENT_TIMESTAMP WHERE stream_key = ?",
    )
    .bind(&stream_key)
    .execute(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    sqlx::query(
        "INSERT INTO realtime_events
            (event_id, stream_key, stream_sequence, kind, schema_version, public_payload,
             state_version, origin_command_id, origin_bid_id, origin_fact_index, created_at)
         VALUES (?, ?, ?, 'lot.bid_accepted', 2, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP)",
    )
    .bind(uuid_blob(Uuid::new_v4()))
    .bind(&stream_key)
    .bind(sequence)
    .bind(payload)
    .bind(i64_version(bid.state_version, "实时事件状态版本")?)
    .bind(uuid_blob(bid.command_id.as_uuid()))
    .bind(uuid_blob(bid.id.as_uuid()))
    .bind(i32_version(bid.fact_index, "实时事件事实序号")?)
    .execute(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}
