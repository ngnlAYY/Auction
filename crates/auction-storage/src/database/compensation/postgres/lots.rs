//! Public 单场逐件结算明细与成交撤销冲正查询。

use crate::database::storage_error_from;

use auction_application::core::ApplicationError;
use auction_application::public::{
    PublicCompensationLot, PublicCompensationLotPage, PublicCompensationLotRequest,
    PublicCompensationLotReversal,
};
use sqlx::Row;
use sqlx::postgres::PgRow;
use uuid::Uuid;

use super::super::storage_error;
use super::load_round;
use crate::database::DatabaseStore;

/// 在同一只读快照中读取批次摘要、该场已售 lot 的冻结结算金额与冲正事实。
pub(crate) async fn lots(
    store: &DatabaseStore,
    auction_id: Uuid,
    request: PublicCompensationLotRequest,
) -> Result<Option<PublicCompensationLotPage>, ApplicationError> {
    let mut transaction = store
        .pool()
        .begin_read_snapshot()
        .await
        .map_err(|error| storage_error_from(error, "开启公开逐件结算读取事务失败"))?;

    let Some(round) = load_round(&mut transaction, auction_id).await? else {
        transaction
            .commit()
            .await
            .map_err(|error| storage_error_from(error, "提交公开逐件结算空结果事务失败"))?;
        return Ok(None);
    };

    let total = sqlx::query_scalar::<_, i64>(
        "SELECT count(*)::bigint
         FROM public.lots AS lot
         JOIN public.lot_settlements AS settlement ON settlement.lot_id = lot.id
         WHERE lot.auction_id = $1 AND settlement.end_price > 0",
    )
    .bind(auction_id)
    .fetch_one(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "统计公开逐件结算明细失败"))?;

    // 只公开已售 lot；流拍 lot 的全部金额为零，不参与附加费或补偿计算。
    let entries = sqlx::query(concat!(
        "SELECT
            lot.id AS lot_id,
            lot.category_number AS lot_number,
            equipment.id AS equipment_id,
            equipment.name AS equipment_name,
            equipment.category AS equipment_category,
            equipment.world_id AS equipment_world_id,
            equipment.external_equipment_id,
            settlement.end_price,
            settlement.end_price - settlement.listed_price AS discount_amount,
            settlement.hold_ppm,
            settlement.adjustment_ppm,
            settlement.tax_amount,
            settlement.compensation_amount,
            settlement.buyer_amount,
            settlement.seller_amount,
            COALESCE(buyer.username, '—')::text AS buyer_name,
            COALESCE(submission.seller, '—')::text AS seller_name,
            ",
        leader_held_nanos!(),
        " AS leader_held_nanos,
            recovery.resolution IS NOT NULL AS voided,
            reversal.compensation_amount - reversal.tax_reversed AS reversal_amount,
            reversal.tax_amount - reversal.tax_reversed AS reversal_tax_shortfall,
            reversal.closing_balance AS reversal_balance_after,
            reversal.created_at AS reversed_at
         FROM public.lots AS lot
         JOIN public.lot_settlements AS settlement ON settlement.lot_id = lot.id
         JOIN public.equipment AS equipment ON equipment.id = lot.equipment_id
         LEFT JOIN public.users AS buyer ON buyer.id = lot.leader_user_id
         LEFT JOIN public.auction_submissions AS submission ON submission.id = lot.submission_id
         LEFT JOIN public.settlement_recoveries AS recovery
           ON recovery.auction_id = lot.auction_id AND recovery.lot_id = lot.id
         LEFT JOIN public.compensation_pool_reversals AS reversal ON reversal.lot_id = lot.id
         WHERE lot.auction_id = $1 AND settlement.end_price > 0
         ORDER BY lot.lot_number, lot.id
         LIMIT $2 OFFSET $3"
    ))
    .bind(auction_id)
    .bind(request.limit)
    .bind(request.offset)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取公开逐件结算明细失败"))?
    .iter()
    .map(map_lot)
    .collect::<Result<Vec<_>, ApplicationError>>()?;

    let page = PublicCompensationLotPage {
        round,
        entries,
        total,
        limit: request.limit,
        offset: request.offset,
    };
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交公开逐件结算读取事务失败"))?;
    page.validate().map_err(storage_error)?;
    Ok(Some(page))
}

fn map_lot(row: &PgRow) -> Result<PublicCompensationLot, ApplicationError> {
    let world: String = row
        .try_get("equipment_world_id")
        .map_err(|error| storage_error_from(error, "读取逐件结算装备世界失败"))?;
    let external_id: String = row
        .try_get("external_equipment_id")
        .map_err(|error| storage_error_from(error, "读取逐件结算装备外部标识失败"))?;
    let projected_game_equipment_url = match world.as_str() {
        "main" => format!("https://hentaiverse.org/equip/{external_id}"),
        "isekai" => format!("https://hentaiverse.org/isekai/equip/{external_id}"),
        _ => return Err(storage_error("逐件结算装备世界不在公开闭集内")),
    };
    let reversed_at: Option<chrono::DateTime<chrono::Utc>> = row
        .try_get("reversed_at")
        .map_err(|error| storage_error_from(error, "读取成交撤销冲正时间失败"))?;
    let reversal = reversed_at
        .map(|created_at| {
            Ok::<_, ApplicationError>(PublicCompensationLotReversal {
                amount: row
                    .try_get("reversal_amount")
                    .map_err(|error| storage_error_from(error, "读取成交撤销冲正金额失败"))?,
                tax_shortfall: row
                    .try_get("reversal_tax_shortfall")
                    .map_err(|error| storage_error_from(error, "读取成交撤销未冲回附加费失败"))?,
                balance_after: row
                    .try_get("reversal_balance_after")
                    .map_err(|error| storage_error_from(error, "读取成交撤销冲正余额失败"))?,
                created_at,
            })
        })
        .transpose()?;
    Ok(PublicCompensationLot {
        lot_id: row
            .try_get("lot_id")
            .map_err(|error| storage_error_from(error, "读取逐件结算 lot 标识失败"))?,
        lot_number: row
            .try_get("lot_number")
            .map_err(|error| storage_error_from(error, "读取逐件结算 lot 序号失败"))?,
        equipment_name: row
            .try_get("equipment_name")
            .map_err(|error| storage_error_from(error, "读取逐件结算装备名称失败"))?,
        equipment_category: row
            .try_get("equipment_category")
            .map_err(|error| storage_error_from(error, "读取逐件结算装备分类失败"))?,
        equipment_id: row
            .try_get::<Uuid, _>("equipment_id")
            .map(auction_domain::EquipmentId::from_uuid)
            .map_err(|error| storage_error_from(error, "读取逐件结算装备标识失败"))?,
        projected_game_equipment_url,
        end_price: row
            .try_get("end_price")
            .map_err(|error| storage_error_from(error, "读取逐件结算成交价失败"))?,
        hold_ppm: ppm(row, "hold_ppm")?,
        adjustment_ppm: ppm(row, "adjustment_ppm")?,
        leader_held_nanos: row
            .try_get("leader_held_nanos")
            .map_err(|error| storage_error_from(error, "读取逐件结算累计领先时长失败"))?,
        tax_amount: row
            .try_get("tax_amount")
            .map_err(|error| storage_error_from(error, "读取逐件结算税收失败"))?,
        discount_amount: row
            .try_get("discount_amount")
            .map_err(|error| storage_error_from(error, "读取逐件结算折扣需求失败"))?,
        compensation_amount: row
            .try_get("compensation_amount")
            .map_err(|error| storage_error_from(error, "读取逐件结算补偿失败"))?,
        buyer_amount: row
            .try_get("buyer_amount")
            .map_err(|error| storage_error_from(error, "读取逐件结算买家实付失败"))?,
        seller_amount: row
            .try_get("seller_amount")
            .map_err(|error| storage_error_from(error, "读取逐件结算卖家所得失败"))?,
        buyer_name: row
            .try_get("buyer_name")
            .map_err(|error| storage_error_from(error, "读取逐件结算买家失败"))?,
        seller_name: row
            .try_get("seller_name")
            .map_err(|error| storage_error_from(error, "读取逐件结算卖家失败"))?,
        voided: row
            .try_get("voided")
            .map_err(|error| storage_error_from(error, "读取逐件结算撤销状态失败"))?,
        reversal,
    })
}

fn ppm(row: &PgRow, column: &str) -> Result<i32, ApplicationError> {
    let value: i64 =
        row.try_get(column).map_err(|error| storage_error_from(error, "读取逐件结算比例失败"))?;
    i32::try_from(value).map_err(|_| storage_error("逐件结算比例超出范围"))
}
