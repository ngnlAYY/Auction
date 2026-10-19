//! Public 单场补偿账本查询。

use auction_application::{
    ApplicationError, PublicCompensationMovement, PublicCompensationMovementKind,
    PublicCompensationMovementPage, PublicCompensationMovementRequest, PublicCompensationRound,
};
use chrono::DateTime;
use sqlx::Row;
use uuid::Uuid;

use super::super::storage_error;
use crate::database::PostgresStore;

/// 在同一 Serializable 快照中读取批次摘要和该场补偿账本。
#[cfg(feature = "postgres")]
pub(crate) async fn movements(
    store: &PostgresStore,
    auction_id: Uuid,
    request: PublicCompensationMovementRequest,
) -> Result<Option<PublicCompensationMovementPage>, ApplicationError> {
    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启公开补偿账本读取事务失败"))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("设置公开补偿账本读取隔离级别失败"))?;

    let round_row = sqlx::query(
        "SELECT
            a.id AS auction_id,
            a.world_id,
            a.sequence AS auction_sequence,
            settlement.opening_pool_balance AS opening_balance,
            totals.tax_income,
            settlement.discount_demand,
            totals.compensation_out,
            settlement.closing_pool_balance AS closing_balance,
            totals.movement_count,
            settlement.settled_at
         FROM public.auction_settlements AS settlement
         JOIN public.auctions AS a
           ON a.id = settlement.auction_id AND a.status = 'settled'
         JOIN public.worlds AS world ON world.id = a.world_id
         JOIN LATERAL (
             SELECT
                 count(*)::bigint AS movement_count,
                 coalesce(sum(ledger.amount) FILTER (WHERE ledger.movement_kind = 'tax'), 0)::bigint
                     AS tax_income,
                 coalesce(
                     -(sum(ledger.amount) FILTER (WHERE ledger.movement_kind = 'compensation')),
                     0
                 )::bigint AS compensation_out
             FROM public.compensation_ledger AS ledger
             WHERE ledger.auction_id = settlement.auction_id
         ) AS totals ON true
         WHERE settlement.auction_id = $1",
    )
    .bind(auction_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取公开补偿池批次失败"))?;

    let Some(round_row) = round_row else {
        transaction.commit().await.map_err(|_| storage_error("提交公开补偿账本空结果事务失败"))?;
        return Ok(None);
    };

    let round = PublicCompensationRound {
        auction_id: round_row
            .try_get("auction_id")
            .map_err(|_| storage_error("读取补偿账本拍卖标识失败"))?,
        world_id: round_row
            .try_get("world_id")
            .map_err(|_| storage_error("读取补偿账本世界标识失败"))?,
        auction_sequence: round_row
            .try_get("auction_sequence")
            .map_err(|_| storage_error("读取补偿账本拍卖序号失败"))?,
        opening_balance: round_row
            .try_get("opening_balance")
            .map_err(|_| storage_error("读取补偿账本期初余额失败"))?,
        tax_income: round_row
            .try_get("tax_income")
            .map_err(|_| storage_error("读取补偿账本税收失败"))?,
        discount_demand: round_row
            .try_get("discount_demand")
            .map_err(|_| storage_error("读取补偿账本折扣需求失败"))?,
        compensation_out: round_row
            .try_get("compensation_out")
            .map_err(|_| storage_error("读取补偿账本支出失败"))?,
        closing_balance: round_row
            .try_get("closing_balance")
            .map_err(|_| storage_error("读取补偿账本期末余额失败"))?,
        movement_count: round_row
            .try_get("movement_count")
            .map_err(|_| storage_error("读取补偿账本笔数失败"))?,
        settled_at: round_row
            .try_get("settled_at")
            .map_err(|_| storage_error("读取补偿账本结算时间失败"))?,
    };

    let rows = sqlx::query(
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
            CASE WHEN ledger.lot_id IS NULL THEN NULL ELSE COALESCE(
                (SELECT SUM(GREATEST(0, LEAST(
                    EXTRACT(EPOCH FROM (COALESCE(b.next_at, lot.effective_end_at) - b.created_at)) * 1000000000,
                    EXTRACT(EPOCH FROM (lot.effective_end_at - b.created_at)) * 1000000000
                )))
                FROM (
                    SELECT bid.bidder_user_id, bid.created_at,
                           LEAD(bid.created_at) OVER (
                               ORDER BY bid.accepted_state_version, bid.fact_index, bid.id
                           ) AS next_at
                    FROM public.bids AS bid
                    WHERE bid.lot_id = lot.id
                ) AS b
                WHERE b.bidder_user_id = lot.leader_user_id)::bigint,
                0
            ) END AS leader_held_nanos
         FROM public.compensation_ledger AS ledger
         LEFT JOIN public.lots AS lot ON lot.id = ledger.lot_id
         LEFT JOIN public.equipment AS equipment ON equipment.id = lot.equipment_id
         LEFT JOIN public.lot_settlements AS settlement ON settlement.lot_id = lot.id
         WHERE ledger.auction_id = $1
         ORDER BY ledger.entry_sequence
         LIMIT $2 OFFSET $3",
    )
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
