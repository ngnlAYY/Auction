//! WithdrawnReturn 的只读 Sent 核对目标读取。

use super::super::DatabaseStore;
use auction_application::{
    ApplicationError, ApplicationFuture, SettlementFulfillmentSentLookupReader,
    SettlementFulfillmentSentLookupTarget,
};
use uuid::Uuid;

impl SettlementFulfillmentSentLookupReader for DatabaseStore {
    fn read_sent_lookup_target(
        &self,
        _obligation_uuid: Uuid,
    ) -> ApplicationFuture<'_, SettlementFulfillmentSentLookupTarget> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    read_postgres(store, _obligation_uuid).await.map_err(ApplicationError::from)
                }
            }
        })
    }
}

#[cfg(feature = "postgres")]
use super::super::PostgresStore;
#[cfg(feature = "postgres")]
use auction_domain::{SettlementId, SettlementWorld};
#[cfg(feature = "postgres")]
use sqlx::Row;

#[cfg(feature = "postgres")]
async fn read_postgres(
    store: &PostgresStore,
    obligation_uuid: Uuid,
) -> Result<SettlementFulfillmentSentLookupTarget, auction_application::RepositoryError> {
    let row = sqlx::query(
        "SELECT o.world_id, o.recipient_game_identity, o.amount, o.carrier_credit,
                o.equipment_id, o.world_id AS equipment_world, e.external_equipment_id,
                f.delivery_operation, d.origin_kind, d.recipient AS delivery_recipient,
                d.payload, d.sending_at, d.completed_at
         FROM public.settlement_obligations AS o
         JOIN public.settlement_obligation_fulfillments AS f ON f.obligation_id = o.id
         JOIN public.deliveries AS d ON d.id = f.delivery_id
         JOIN public.equipment AS e ON e.id = o.equipment_id
         WHERE o.id = $1 AND o.obligation_kind = 'withdrawn_return'
           AND f.delivery_operation = 'send' AND d.origin_kind = 'settlement'
           AND e.world_id = o.world_id AND d.attempt_count = 1
           AND d.payload ->> 'world' = o.world_id
           AND d.origin_id = o.id::text
           AND d.payload ->> 'schema_version' = '1'
           AND d.payload ->> 'template' = 'settlement_equipment_handoff'
           AND d.payload -> 'variables' ->> 'reference' = o.id::text",
    )
    .bind(obligation_uuid)
    .fetch_optional(store.pool())
    .await
    .map_err(|_| repository_error("读取撤拍 Sent 核对目标失败"))?
    .ok_or_else(|| repository_error("撤拍 Sent 目标不存在或冻结条件不满足"))?;
    let world = match row
        .try_get::<String, _>("world_id")
        .map_err(|_| repository_error("读取撤拍 Sent 世界失败"))?
        .as_str()
    {
        "main" => SettlementWorld::Main,
        "isekai" => SettlementWorld::Isekai,
        _ => return Err(repository_error("撤拍 Sent 世界无效")),
    };
    let recipient: String = row
        .try_get("recipient_game_identity")
        .map_err(|_| repository_error("读取撤拍 Sent 收件人失败"))?;
    let delivery_recipient: String = row
        .try_get("delivery_recipient")
        .map_err(|_| repository_error("读取撤拍 delivery 收件人失败"))?;
    if recipient.is_empty() || recipient != delivery_recipient {
        return Err(repository_error("撤拍 Sent 收件人冻结事实不一致"));
    }
    let payload: serde_json::Value =
        row.try_get("payload").map_err(|_| repository_error("读取撤拍 Sent payload 失败"))?;
    let refs = payload
        .get("equipment_secret_refs")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| repository_error("撤拍 Sent payload 缺少装备引用"))?;
    let equipment_id: Uuid =
        row.try_get("equipment_id").map_err(|_| repository_error("读取撤拍装备标识失败"))?;
    let expected_ref = equipment_id.to_string();
    if payload.get("operation").and_then(serde_json::Value::as_str) != Some("send")
        || payload.get("credits").and_then(serde_json::Value::as_i64) != Some(0)
        || payload.get("cod").and_then(serde_json::Value::as_i64) != Some(0)
        || refs.len() != 1
        || refs[0].get("id").and_then(serde_json::Value::as_str) != Some(expected_ref.as_str())
    {
        return Err(repository_error("撤拍 Sent payload 冻结事实无效"));
    }
    Ok(SettlementFulfillmentSentLookupTarget {
        obligation_id: SettlementId::from_uuid(obligation_uuid),
        world,
        recipient,
        external_equipment_id: row
            .try_get("external_equipment_id")
            .map_err(|_| repository_error("读取撤拍外部装备标识失败"))?,
        credits: row.try_get("amount").map_err(|_| repository_error("读取撤拍金额失败"))?,
        cod: row.try_get("carrier_credit").map_err(|_| repository_error("读取撤拍 COD 失败"))?,
        sending_at: row.try_get("sending_at").map_err(|_| repository_error("读取发送时间失败"))?,
        completed_at: row
            .try_get("completed_at")
            .map_err(|_| repository_error("读取完成时间失败"))?,
    })
}

#[cfg(feature = "postgres")]
fn repository_error(message: &str) -> auction_application::RepositoryError {
    auction_application::RepositoryError::Failed(message.to_owned())
}
