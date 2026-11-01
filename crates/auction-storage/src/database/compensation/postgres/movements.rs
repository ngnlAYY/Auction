//! Public 单场补偿账本查询。

use auction_application::{
    ApplicationError, PublicCompensationMovement, PublicCompensationMovementKind,
    PublicCompensationMovementPage, PublicCompensationMovementRequest,
};
use chrono::DateTime;
use sqlx::Row;
use uuid::Uuid;

use super::super::storage_error;
use super::load_round;
use crate::database::PostgresStore;

/// 在同一只读快照中读取批次摘要和该场补偿账本。
#[cfg(feature = "postgres")]
pub(crate) async fn movements(
    store: &PostgresStore,
    auction_id: Uuid,
    request: PublicCompensationMovementRequest,
) -> Result<Option<PublicCompensationMovementPage>, ApplicationError> {
    let mut transaction = store
        .pool()
        .begin_read_snapshot()
        .await
        .map_err(|_| storage_error("开启公开补偿账本读取事务失败"))?;

    let Some(round) = load_round(&mut transaction, auction_id).await? else {
        transaction.commit().await.map_err(|_| storage_error("提交公开补偿账本空结果事务失败"))?;
        return Ok(None);
    };

    let rows = sqlx::query(concat!(
        "SELECT
            ledger.auction_id,
            ledger.entry_sequence,
            ledger.lot_id,
            ledger.movement_kind,
            ledger.amount,
            ledger.balance_after,
            ledger.created_at,
            lot.category_number AS lot_number,
            equipment.id AS equipment_id,
            equipment.name AS equipment_name,
            equipment.category AS equipment_category,
            equipment.world_id AS equipment_world_id,
            equipment.external_equipment_id,
            settlement.hold_ppm,
            settlement.adjustment_ppm,
            CASE WHEN ledger.lot_id IS NULL THEN NULL ELSE ",
        leader_held_nanos!(),
        " END AS leader_held_nanos
         FROM public.compensation_ledger AS ledger
         LEFT JOIN public.lots AS lot ON lot.id = ledger.lot_id
         LEFT JOIN public.equipment AS equipment ON equipment.id = lot.equipment_id
         LEFT JOIN public.lot_settlements AS settlement ON settlement.lot_id = lot.id
         WHERE ledger.auction_id = $1
         ORDER BY ledger.entry_sequence
         LIMIT $2 OFFSET $3"
    ))
    .bind(auction_id)
    .bind(request.limit)
    .bind(request.offset)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取公开补偿账本条目失败"))?;

    let entries = rows
        .into_iter()
        .map(|row| {
            let kind: String =
                row.try_get("movement_kind").map_err(|_| storage_error("读取补偿账本类型失败"))?;
            let kind = PublicCompensationMovementKind::parse(&kind)
                .ok_or_else(|| storage_error("补偿账本类型不在公开闭集内"))?;
            let created_at: DateTime<_> =
                row.try_get("created_at").map_err(|_| storage_error("读取补偿账本时间失败"))?;
            Ok(PublicCompensationMovement {
                auction_id: row
                    .try_get("auction_id")
                    .map_err(|_| storage_error("读取补偿账本条目拍卖标识失败"))?,
                sequence: row
                    .try_get("entry_sequence")
                    .map_err(|_| storage_error("读取补偿账本序号失败"))?,
                lot_id: row
                    .try_get("lot_id")
                    .map_err(|_| storage_error("读取补偿账本 lot 标识失败"))?,
                lot_number: row
                    .try_get("lot_number")
                    .map_err(|_| storage_error("读取补偿账本 lot 序号失败"))?,
                equipment_name: row
                    .try_get("equipment_name")
                    .map_err(|_| storage_error("读取补偿账本装备名称失败"))?,
                equipment_category: row
                    .try_get("equipment_category")
                    .map_err(|_| storage_error("读取补偿账本装备分类失败"))?,
                equipment_id: row
                    .try_get::<Option<Uuid>, _>("equipment_id")
                    .map_err(|_| storage_error("读取补偿账本装备标识失败"))?
                    .map(auction_domain::EquipmentId::from_uuid),
                projected_game_equipment_url: {
                    let world: Option<String> = row
                        .try_get("equipment_world_id")
                        .map_err(|_| storage_error("读取补偿账本装备世界失败"))?;
                    let external_id: Option<String> = row
                        .try_get("external_equipment_id")
                        .map_err(|_| storage_error("读取补偿账本装备外部标识失败"))?;
                    match (world.as_deref(), external_id.as_deref()) {
                        (Some("main"), Some(id)) => {
                            Some(format!("https://hentaiverse.org/equip/{id}"))
                        }
                        (Some("isekai"), Some(id)) => {
                            Some(format!("https://hentaiverse.org/isekai/equip/{id}"))
                        }
                        _ => None,
                    }
                },
                hold_ppm: row
                    .try_get::<Option<i64>, _>("hold_ppm")
                    .map_err(|_| storage_error("读取补偿账本持有比例失败"))?
                    .map(|value| {
                        i32::try_from(value).map_err(|_| storage_error("补偿账本持有比例超出范围"))
                    })
                    .transpose()?,
                adjustment_ppm: row
                    .try_get::<Option<i64>, _>("adjustment_ppm")
                    .map_err(|_| storage_error("读取补偿账本调整比例失败"))?
                    .map(|value| {
                        i32::try_from(value).map_err(|_| storage_error("补偿账本调整比例超出范围"))
                    })
                    .transpose()?,
                leader_held_nanos: row
                    .try_get::<Option<i64>, _>("leader_held_nanos")
                    .map_err(|_| storage_error("读取补偿账本累计领先时长失败"))?,
                kind,
                amount: row.try_get("amount").map_err(|_| storage_error("读取补偿账本金额失败"))?,
                balance_after: row
                    .try_get("balance_after")
                    .map_err(|_| storage_error("读取补偿账本余额失败"))?,
                created_at,
            })
        })
        .collect::<Result<Vec<_>, ApplicationError>>()?;

    let page = PublicCompensationMovementPage {
        total: round.movement_count,
        round,
        entries,
        limit: request.limit,
        offset: request.offset,
    };
    transaction.commit().await.map_err(|_| storage_error("提交公开补偿账本读取事务失败"))?;
    page.validate().map_err(storage_error)?;
    Ok(Some(page))
}
