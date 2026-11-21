//! 非竞价实时事件的同事务追加：在调用方的业务事务内立即取号并写入 `realtime_events`。
//!
//! 拍卖创建、排期、Admin lot 修改、生命周期、录入、重挂与结算都经这里写事件，共用的负载也在
//! 这里构造；字段集合须与公开注册表（`PublicRealtimeEvent::validate`）一致，否则推送与回放都会
//! 拒绝该事件（回放置 refetch_required）。
//! 竞价事实走提交后补号（见 `sequence` 模块），Admin 重放写的是引用原事件的副本，都不经过这里。

use auction_time::Timestamp;
use serde_json::{Value, json};
use uuid::Uuid;

use super::sequence::allocate_stream_sequence;

/// 立即取号事件的唯一 INSERT；`created_at` 为空时取事务时间，与列默认值相同。
const SEQUENCED_EVENT_INSERT_SQL: &str = "INSERT INTO public.realtime_events
        (event_id, stream_key, stream_sequence, kind, schema_version, public_payload,
         state_version, origin_command_id, created_at)
     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, COALESCE($9, transaction_timestamp()))";

/// 一条在业务事务内立即取号的持久实时事件。
pub(crate) struct SequencedEvent<'a> {
    pub(crate) event_id: Uuid,
    pub(crate) stream_key: &'a str,
    pub(crate) kind: &'a str,
    pub(crate) schema_version: i32,
    pub(crate) payload: Value,
    pub(crate) state_version: i64,
    /// 只关联 `auction_commands` 中的命令；专用命令账本的来源事件留空。
    pub(crate) origin_command_id: Option<Uuid>,
    /// 同时作为流计数行的 `updated_at` 与事件的 `created_at`；`None` 取事务时间，结算路径
    /// 传入业务发生时间。
    pub(crate) created_at: Option<Timestamp>,
}

/// 在调用方事务内为事件分配流内下一个序号并写入事件行。
///
/// 不变量 3：事件与被接受的业务事实同事务写入，任一步失败都随业务事务回滚。取号持有流计数
/// 行锁直到事务结束，同一流的立即取号写入因此串行，流内序号连续无空号。
pub(crate) async fn append_sequenced_event(
    connection: &mut sqlx::PgConnection,
    event: SequencedEvent<'_>,
) -> Result<(), sqlx::Error> {
    let sequence =
        allocate_stream_sequence(&mut *connection, event.stream_key, event.created_at).await?;
    sqlx::query(SEQUENCED_EVENT_INSERT_SQL)
        .bind(event.event_id)
        .bind(event.stream_key)
        .bind(sequence)
        .bind(event.kind)
        .bind(event.schema_version)
        .bind(event.payload)
        .bind(event.state_version)
        .bind(event.origin_command_id)
        .bind(event.created_at)
        .execute(connection)
        .await?;
    Ok(())
}

/// `preview_catalog_changed` v1 负载；Admin 录入、MoogleMail 入站与 recovery 重挂共用。
pub(crate) fn preview_catalog_changed_payload(
    world: &str,
    auction_id: Uuid,
    preview_revision: i64,
) -> Value {
    json!({
        "world": world,
        "auction_id": auction_id,
        "preview_revision": preview_revision,
    })
}

/// `auction.created` v1 负载；只由 Admin 创建事务写入该拍卖自己的流。
pub(crate) fn auction_created_payload(
    auction_id: Uuid,
    world_id: &str,
    sequence: i64,
    state_version: i64,
) -> Value {
    json!({
        "auction_id": auction_id,
        "world_id": world_id,
        "sequence": sequence,
        "state_version": state_version,
    })
}

#[cfg(test)]
mod tests {
    use auction_application::realtime::PublicRealtimeEvent;
    use auction_time::Timestamp;
    use serde_json::Value;
    use uuid::Uuid;

    use super::auction_created_payload;
    use super::preview_catalog_changed_payload;

    fn event(stream_key: String, kind: &str, payload: Value) -> PublicRealtimeEvent {
        PublicRealtimeEvent {
            event_id: Uuid::new_v4(),
            stream_key,
            stream_sequence: 1,
            kind: kind.to_owned(),
            schema_version: 1,
            payload,
            created_at: Timestamp::now(),
        }
    }

    #[test]
    fn shared_payloads_match_public_registry() {
        let auction_id = Uuid::new_v4();
        let stream_key = format!("auction:{auction_id}");
        let preview = preview_catalog_changed_payload("main", auction_id, 3);
        assert_eq!(
            event(stream_key.clone(), "preview_catalog_changed", preview).validate(),
            Ok(())
        );
        let created = auction_created_payload(auction_id, "main", 7, 2);
        assert_eq!(event(stream_key, "auction.created", created).validate(), Ok(()));
    }

    #[test]
    fn shared_payloads_bind_to_their_auction_stream() {
        let auction_id = Uuid::new_v4();
        let other_stream = format!("auction:{}", Uuid::new_v4());
        let preview = preview_catalog_changed_payload("main", auction_id, 3);
        assert!(
            event(other_stream.clone(), "preview_catalog_changed", preview).validate().is_err()
        );
        let created = auction_created_payload(auction_id, "main", 7, 2);
        assert!(event(other_stream, "auction.created", created.clone()).validate().is_err());
        // 迁移 000011 的历史发现流不是公开流：负载合法也不能通过注册表。
        let discovery = "auction:discovery".to_owned();
        assert!(event(discovery, "auction.created", created).validate().is_err());
        // 预览修订号与状态版本必须为正，构造器不替调用方修正非法值。
        let stream_key = format!("auction:{auction_id}");
        let unrevised = preview_catalog_changed_payload("main", auction_id, 0);
        assert!(event(stream_key, "preview_catalog_changed", unrevised).validate().is_err());
    }
}
