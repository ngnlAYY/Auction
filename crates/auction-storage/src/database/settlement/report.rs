//! 从 PostgreSQL 结算事实链读取报告；摘要与 lot 明细共用同一可重复读只读快照。金额与比例
//! 读冻结的结算事实；累计领先时长没有冻结列，按结算锁内的同一公式从只追加的竞价事实重放。

use auction_application::core::ApplicationError;
use auction_application::core::ApplicationFuture;
use auction_application::settlement::{
    SettlementAdjustment, SettlementLotReport, SettlementSummary,
};
use auction_application::settlement::{SettlementReport, SettlementReportReader};
use auction_domain::AuctionId;

use crate::database::DatabaseStore;
use crate::database::auction::snapshot::{auction_snapshot_columns, decode_auction_snapshot};
use crate::database::settlement::holding::leader_held_nanos;
use crate::database::storage_error;
use crate::database::storage_error_from;

impl SettlementReportReader for DatabaseStore {
    fn settlement_report(
        &self,
        auction_id: AuctionId,
    ) -> ApplicationFuture<'_, Option<SettlementReport>> {
        Box::pin(async move {
            if auction_id.is_nil() {
                return Err(storage_error("结算报告拍卖标识无效"));
            }

            postgres_report(self, auction_id).await
        })
    }
}

/// 摘要、账本笔数与 lot 明细共用一个可重复读只读快照：结算事实单事务提交，单快照足以保证
/// 一致；不持有 SSI 谓词锁，不会与并发结算或竞价写事务形成读写依赖，自身也不会因 40001 失败。
async fn postgres_report(
    store: &crate::database::DatabaseStore,
    auction_id: AuctionId,
) -> Result<Option<SettlementReport>, ApplicationError> {
    use auction_time::Timestamp;
    use sqlx::Row;
    use uuid::Uuid;

    let mut transaction = store
        .pool()
        .begin_read_snapshot()
        .await
        .map_err(|error| storage_error_from(error, "打开结算报告快照失败"))?;

    let row = sqlx::query(concat!(
        "SELECT ",
        auction_snapshot_columns!(),
        r#",
               s.auction_id AS settlement_auction_id,
               s.opening_pool_balance,
               s.tax_total,
               s.discount_demand,
               s.compensation_total,
               s.closing_pool_balance,
               s.settled_at AS settlement_settled_at,
               (
                   SELECT COUNT(*)::bigint
                   FROM public.compensation_ledger AS ledger
                   WHERE ledger.auction_id = a.id
               ) AS movement_count
           FROM public.auctions AS a
           LEFT JOIN public.auction_settlements AS s ON s.auction_id = a.id
           WHERE a.id = $1"#
    ))
    .bind(auction_id.as_uuid())
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取结算报告拍卖失败"))?;

    let Some(row) = row else {
        transaction
            .commit()
            .await
            .map_err(|error| storage_error_from(error, "提交结算报告快照失败"))?;
        return Ok(None);
    };

    let auction = decode_auction_snapshot(&row)
        .map_err(|(context, error)| storage_error_from(error, context))?;
    let stored_auction_id = auction.id.as_uuid();
    if stored_auction_id != auction_id.as_uuid() {
        return Err(storage_error("结算报告拍卖标识与请求不一致"));
    }

    let settlement_id: Option<Uuid> = row
        .try_get("settlement_auction_id")
        .map_err(|error| storage_error_from(error, "读取结算报告结算标识失败"))?;
    let settlement = match settlement_id {
        None => None,
        Some(value) if value == stored_auction_id => {
            let settled_at: Timestamp = row
                .try_get("settlement_settled_at")
                .map_err(|error| storage_error_from(error, "读取结算报告完成时间失败"))?;
            Some(SettlementSummary {
                opening_balance: row
                    .try_get("opening_pool_balance")
                    .map_err(|error| storage_error_from(error, "读取结算报告期初余额失败"))?,
                tax_income: row
                    .try_get("tax_total")
                    .map_err(|error| storage_error_from(error, "读取结算报告税收失败"))?,
                discount_demand: row
                    .try_get("discount_demand")
                    .map_err(|error| storage_error_from(error, "读取结算报告折扣需求失败"))?,
                compensation_out: row
                    .try_get("compensation_total")
                    .map_err(|error| storage_error_from(error, "读取结算报告补偿支出失败"))?,
                closing_balance: row
                    .try_get("closing_pool_balance")
                    .map_err(|error| storage_error_from(error, "读取结算报告期末余额失败"))?,
                movement_count: row
                    .try_get("movement_count")
                    .map_err(|error| storage_error_from(error, "读取结算报告账本笔数失败"))?,
                settled_at,
            })
        }
        Some(_) => return Err(storage_error("结算报告结算标识与拍卖不一致")),
    };

    let lots = if settlement.is_some() {
        // 撤回标的不参与结算金额；其余 lot 保留 LEFT JOIN，缺少结算事实时仍失败关闭。
        let rows = sqlx::query(concat!(
            r#"SELECT
                   l.id,
                   l.category_number AS lot_number,
                   l.status,
                   equipment.name AS equipment_name,
                   equipment.category AS equipment_category,
                   equipment.external_equipment_id AS external_equipment_id,
                   submission.seller,
                   winner.username AS winner,
                   settlement.lot_id AS settlement_lot_id,
                   COALESCE(l.current_price, 0)::bigint AS final_price,
                   settlement.listed_price AS listed_price,
                   settlement.buyer_amount AS buyer_amount,
                   settlement.seller_amount AS seller_amount,
                   settlement.tax_amount AS tax_amount,
                   settlement.compensation_amount AS compensation_amount,
                   settlement.hold_ppm AS hold_ppm,
                   settlement.adjustment_ppm AS adjustment_ppm,
                   "#,
            leader_held_nanos!("l"),
            r#" AS leader_held_nanos,
                   settlement.adjustment_metadata AS adjustment_metadata
               FROM public.lots AS l
               INNER JOIN public.equipment AS equipment ON equipment.id = l.equipment_id
               LEFT JOIN public.auction_submissions AS submission
                   ON submission.id = l.submission_id
               LEFT JOIN public.users AS winner ON winner.id = l.leader_user_id
               LEFT JOIN public.lot_settlements AS settlement ON settlement.lot_id = l.id
               WHERE l.auction_id = $1 AND l.status <> 'withdrawn'
               ORDER BY l.lot_number, l.id"#
        ))
        .bind(stored_auction_id)
        .fetch_all(&mut *transaction)
        .await
        .map_err(|error| storage_error_from(error, "读取结算报告 lot 失败"))?;

        rows.into_iter()
            .map(|row| {
                let lot_id: Uuid = row
                    .try_get("id")
                    .map_err(|error| storage_error_from(error, "读取结算 lot 标识失败"))?;
                let settlement_lot_id: Option<Uuid> = row
                    .try_get("settlement_lot_id")
                    .map_err(|error| storage_error_from(error, "读取结算 lot 结算标识失败"))?;
                if settlement_lot_id != Some(lot_id) {
                    return Err(storage_error("结算报告缺少 lot 结算事实"));
                }
                let buyer_amount: Option<i64> = row
                    .try_get("buyer_amount")
                    .map_err(|error| storage_error_from(error, "读取结算买方金额失败"))?;
                let listed_price: Option<i64> = row
                    .try_get("listed_price")
                    .map_err(|error| storage_error_from(error, "读取结算挂牌金额失败"))?;
                let seller_amount: Option<i64> = row
                    .try_get("seller_amount")
                    .map_err(|error| storage_error_from(error, "读取结算卖方金额失败"))?;
                let tax_amount: Option<i64> = row
                    .try_get("tax_amount")
                    .map_err(|error| storage_error_from(error, "读取结算税额失败"))?;
                let compensation_amount: Option<i64> = row
                    .try_get("compensation_amount")
                    .map_err(|error| storage_error_from(error, "读取结算补偿金额失败"))?;
                let hold_ppm: Option<i64> = row
                    .try_get("hold_ppm")
                    .map_err(|error| storage_error_from(error, "读取结算持有比例失败"))?;
                let adjustment_ppm: Option<i64> = row
                    .try_get("adjustment_ppm")
                    .map_err(|error| storage_error_from(error, "读取结算调整比例失败"))?;
                let (
                    Some(listed_price),
                    Some(buyer_amount),
                    Some(seller_amount),
                    Some(tax_amount),
                    Some(compensation_amount),
                    Some(hold_ppm),
                    Some(adjustment_ppm),
                ) = (
                    listed_price,
                    buyer_amount,
                    seller_amount,
                    tax_amount,
                    compensation_amount,
                    hold_ppm,
                    adjustment_ppm,
                )
                else {
                    return Err(storage_error("结算报告缺少 lot 金额或调整事实"));
                };
                let hold_ppm = i32::try_from(hold_ppm)
                    .map_err(|_| storage_error("结算报告持有比例超出支持范围"))?;
                let adjustment_ppm = i32::try_from(adjustment_ppm)
                    .map_err(|_| storage_error("结算报告调整比例超出支持范围"))?;
                let leader_held_nanos: i64 = row
                    .try_get("leader_held_nanos")
                    .map_err(|error| storage_error_from(error, "读取结算报告累计领先时长失败"))?;
                if leader_held_nanos < 0 {
                    return Err(storage_error("结算报告累计领先时长无效"));
                }
                let final_price: i64 = row
                    .try_get("final_price")
                    .map_err(|error| storage_error_from(error, "读取结算成交金额失败"))?;
                let adjustment_metadata: serde_json::Value =
                    row.try_get("adjustment_metadata").map_err(|error| {
                        storage_error_from(error, "读取结算 adjustment metadata 失败")
                    })?;
                Ok(SettlementLotReport {
                    id: auction_domain::LotId::from_uuid(lot_id),
                    lot_number: row
                        .try_get("lot_number")
                        .map_err(|error| storage_error_from(error, "读取结算 lot 序号失败"))?,
                    status: row
                        .try_get("status")
                        .map_err(|error| storage_error_from(error, "读取结算 lot 状态失败"))?,
                    equipment_name: row
                        .try_get("equipment_name")
                        .map_err(|error| storage_error_from(error, "读取结算装备名称失败"))?,
                    equipment_category: row
                        .try_get("equipment_category")
                        .map_err(|error| storage_error_from(error, "读取结算装备分类失败"))?,
                    external_equipment_id: row
                        .try_get("external_equipment_id")
                        .map_err(|error| storage_error_from(error, "读取结算装备外部标识失败"))?,
                    seller: row
                        .try_get("seller")
                        .map_err(|error| storage_error_from(error, "读取结算卖方失败"))?,
                    winner: row
                        .try_get("winner")
                        .map_err(|error| storage_error_from(error, "读取结算买方失败"))?,
                    final_price,
                    listed_price,
                    buyer_amount,
                    seller_amount,
                    tax_amount,
                    compensation_amount,
                    hold_ppm,
                    adjustment_ppm,
                    leader_held_nanos,
                    adjustment: decode_settlement_adjustment(adjustment_metadata)?,
                })
            })
            .collect::<Result<Vec<_>, ApplicationError>>()?
    } else {
        Vec::new()
    };

    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交结算报告快照失败"))?;

    Ok(Some(SettlementReport { auction, settlement, lots }))
}

fn decode_settlement_adjustment(
    value: serde_json::Value,
) -> Result<SettlementAdjustment, ApplicationError> {
    let serde_json::Value::Object(mut fields) = value else {
        return if value.is_null() {
            Ok(SettlementAdjustment { policy: None })
        } else {
            Err(storage_error("结算 adjustment metadata 结构无效"))
        };
    };
    if fields.keys().any(|key| key != "policy") {
        return Err(storage_error("结算 adjustment metadata 含未知字段"));
    }
    let policy = match fields.remove("policy") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(policy))
            if !policy.is_empty() && policy == policy.trim() =>
        {
            Some(policy)
        }
        Some(_) => return Err(storage_error("结算 adjustment policy 无效")),
    };
    Ok(SettlementAdjustment { policy })
}
