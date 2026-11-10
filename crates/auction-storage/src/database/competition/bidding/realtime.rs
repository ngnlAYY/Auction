//! PostgreSQL 竞价 realtime 事件追加。
//!
//! 竞价事件与出价事实同事务写入（不变量 3），但序号在提交后分配。旧实现在
//! Serializable 事务内递增同一 `auction:<id>` 的 `realtime_streams` 行，使同场不同 lot 的
//! 重叠出价形成读写反依赖并互相中止；这里只 INSERT 事件行（`stream_sequence` 为
//! NULL），由 `PostgresCompetitionTransaction` 收集待定 stream key，提交后补号。
//! 未分配序号的事件不进入回放：`stream_sequence > after` 对 NULL 不成立。

use auction_application::auction::format_public_time_wire;

use super::super::mapping::*;
use crate::database::competition::PostgresCompetitionTransaction;
use auction_application::core::CompetitionTransactionError;
use auction_domain::AcceptedBid;
use serde_json::json;
use uuid::Uuid;

/// 追加一条 `lot.bid_accepted`，序号留待提交后分配。
/// 计划已校验全部事实都属于 `final_lot`；场次归属直接取锁内快照已读出的 ID（`loaded_auction_id`），
/// 只在快照缓存缺失时才回退到一次等值读取。后续事实写入事件行时不再读写共享的 stream 计数行。
pub(in crate::database::competition) async fn insert_realtime_event(
    transaction: &mut PostgresCompetitionTransaction,
    auction_id: &mut Option<Uuid>,
    bid: &AcceptedBid,
) -> Result<(), CompetitionTransactionError> {
    let auction_id = match *auction_id {
        Some(auction_id) => auction_id,
        None => {
            let lot_auction_id = match transaction.loaded_auction_id() {
                Some(auction_id) => auction_id,
                // 快照缓存缺失（例如未来新增的调用路径）：保留一次等值读取作为兜底，
                // 不改变场次归属的权威来源。
                None => {
                    let transaction = transaction.transaction_mut()?;
                    sqlx::query_scalar("SELECT auction_id FROM public.lots WHERE id = $1")
                        .bind(bid.lot_id.as_uuid())
                        .fetch_one(&mut **transaction)
                        .await
                        .map_err(map_sql_error)?
                }
            };
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
        "mechanism": bid.mechanism.as_str(),
    });
    {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            r#"INSERT INTO public.realtime_events
                (event_id, stream_key, stream_sequence, kind, schema_version, public_payload,
                 state_version, origin_command_id, origin_bid_id, origin_fact_index)
             VALUES ($1, $2, NULL, 'lot.bid_accepted', 2, $3, $4, $5, $6, $7)"#,
        )
        .bind(Uuid::new_v4())
        .bind(&stream_key)
        .bind(payload)
        .bind(i64_version(bid.state_version, "实时事件状态版本")?)
        .bind(bid.command_id.as_uuid())
        .bind(bid.id.as_uuid())
        .bind(i32_version(bid.fact_index, "实时事件事实序号")?)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
    }
    // stream 行由拍卖创建事务（`auction.created`）建立；同一命令的多条事实共享同一个 key。
    transaction.record_deferred_stream(&stream_key);
    Ok(())
}
