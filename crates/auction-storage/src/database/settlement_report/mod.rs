//! 从 PostgreSQL 结算事实链读取报告；摘要与 lot 明细保持同一快照，不重算历史结果。

use auction_application::{
    ApplicationError, ApplicationFuture, SettlementReport, SettlementReportReader,
};
use auction_domain::AuctionId;

#[cfg(feature = "postgres")]
use auction_application::{
    SettlementAdjustment, SettlementAuctionSnapshot, SettlementLotReport, SettlementSummary,
};

use super::DatabaseStore;

impl SettlementReportReader for DatabaseStore {
    fn settlement_report(
        &self,
        auction_id: AuctionId,
    ) -> ApplicationFuture<'_, Option<SettlementReport>> {
        Box::pin(async move {
            if auction_id.is_nil() {
                return Err(storage_error("结算报告拍卖标识无效"));
            }

            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => postgres_report(store, auction_id).await,
            }
        })
    }
}

#[cfg(feature = "postgres")]
async fn postgres_report(
    store: &super::PostgresStore,
    auction_id: AuctionId,
) -> Result<Option<SettlementReport>, ApplicationError> {
    for attempt in 0..3_u8 {
        match postgres_report_once(store, auction_id).await {
            Err(error) if error.retryable() && attempt < 2 => continue,
            result => return result.map_err(PostgresReportError::into_application),
        }
    }
    Err(storage_error("结算报告可序列化快照重试次数耗尽"))
}

#[cfg(feature = "postgres")]
enum PostgresReportError {
    Database { source: sqlx::Error, safe_message: &'static str },
    Application(ApplicationError),
}

#[cfg(feature = "postgres")]
impl PostgresReportError {
    fn database(source: sqlx::Error, safe_message: &'static str) -> Self {
        Self::Database { source, safe_message }
    }

    fn retryable(&self) -> bool {
        matches!(
            self,
            Self::Database {
                source: sqlx::Error::Database(database),
                ..
            } if matches!(database.code().as_deref(), Some("40001" | "40P01"))
        )
    }

    fn into_application(self) -> ApplicationError {
        match self {
            Self::Database { safe_message, .. } => storage_error(safe_message),
            Self::Application(error) => error,
        }
    }
}

#[cfg(feature = "postgres")]
impl From<ApplicationError> for PostgresReportError {
    fn from(error: ApplicationError) -> Self {
        Self::Application(error)
    }
}

#[cfg(feature = "postgres")]
async fn postgres_report_once(
    store: &super::PostgresStore,
    auction_id: AuctionId,
) -> Result<Option<SettlementReport>, PostgresReportError> {
    use chrono::{DateTime, Utc};
    use sqlx::Row;
    use uuid::Uuid;

    let mut transaction = store
        .pool()
        .begin()
        .await
        .map_err(|error| PostgresReportError::database(error, "打开结算报告快照失败"))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await
        .map_err(|error| PostgresReportError::database(error, "设置结算报告快照失败"))?;

    let row = sqlx::query(
        r#"SELECT
               a.id AS auction_id,
               a.world_id,
               a.sequence,
               a.status,
               a.preview_scheduled_at,
               a.scheduled_open_at,
               a.preview_opened_at,
               a.preview_manual_open,
               a.preview_revision,
               a.opened_at,
               a.final_phase_at,
               a.settled_at,
               a.cancelled_at,
               a.cancelled_reason,
               a.cancelled_by,
               a.state_version,
               a.scheduled_end_at,
               a.effective_end_at,
               a.created_at,
               a.updated_at,
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
           WHERE a.id = $1"#,
    )
    .bind(auction_id.as_uuid())
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|error| PostgresReportError::database(error, "读取结算报告拍卖失败"))?;

    let Some(row) = row else {
        transaction
            .commit()
            .await
            .map_err(|error| PostgresReportError::database(error, "提交结算报告快照失败"))?;
        return Ok(None);
    };

    let stored_auction_id: Uuid =
        row.try_get("auction_id").map_err(|_| storage_error("读取结算报告拍卖标识失败"))?;
    if stored_auction_id != auction_id.as_uuid() {
        return Err(storage_error("结算报告拍卖标识与请求不一致").into());
    }

    let settlement_id: Option<Uuid> = row
        .try_get("settlement_auction_id")
        .map_err(|_| storage_error("读取结算报告结算标识失败"))?;
    let settlement = match settlement_id {
        None => None,
        Some(value) if value == stored_auction_id => {
            let settled_at: DateTime<Utc> = row
                .try_get("settlement_settled_at")
                .map_err(|_| storage_error("读取结算报告完成时间失败"))?;
            Some(SettlementSummary {
                opening_balance: row
                    .try_get("opening_pool_balance")
                    .map_err(|_| storage_error("读取结算报告期初余额失败"))?,
                tax_income: row
                    .try_get("tax_total")
                    .map_err(|_| storage_error("读取结算报告税收失败"))?,
                discount_demand: row
                    .try_get("discount_demand")
                    .map_err(|_| storage_error("读取结算报告折扣需求失败"))?,
                compensation_out: row
                    .try_get("compensation_total")
                    .map_err(|_| storage_error("读取结算报告补偿支出失败"))?,
                closing_balance: row
                    .try_get("closing_pool_balance")
                    .map_err(|_| storage_error("读取结算报告期末余额失败"))?,
                movement_count: row
                    .try_get("movement_count")
                    .map_err(|_| storage_error("读取结算报告账本笔数失败"))?,
                settled_at,
            })
        }
        Some(_) => return Err(storage_error("结算报告结算标识与拍卖不一致").into()),
    };

    let lots = if settlement.is_some() {
        // 撤回标的不参与结算金额；其余 lot 保留 LEFT JOIN，缺少结算事实时仍失败关闭。
        let rows = sqlx::query(
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
                   (SELECT COALESCE(SUM(GREATEST(0, LEAST(
                         EXTRACT(EPOCH FROM (COALESCE(b.next_at, l.effective_end_at) - b.created_at)) * 1000000000,
                         EXTRACT(EPOCH FROM (l.effective_end_at - b.created_at)) * 1000000000
                     )))::bigint, 0)
                     FROM (
                         SELECT bid.bidder_user_id, bid.created_at,
                                LEAD(bid.created_at) OVER (
                                    ORDER BY bid.accepted_state_version, bid.fact_index, bid.id
                                ) AS next_at
                         FROM public.bids AS bid
                         WHERE bid.lot_id = l.id
                     ) AS b
                     WHERE b.bidder_user_id = l.leader_user_id) AS leader_held_nanos,
                   settlement.adjustment_metadata AS adjustment_metadata
               FROM public.lots AS l
               INNER JOIN public.equipment AS equipment ON equipment.id = l.equipment_id
               LEFT JOIN public.auction_submissions AS submission
                   ON submission.id = l.submission_id
               LEFT JOIN public.users AS winner ON winner.id = l.leader_user_id
               LEFT JOIN public.lot_settlements AS settlement ON settlement.lot_id = l.id
               WHERE l.auction_id = $1 AND l.status <> 'withdrawn'
               ORDER BY l.lot_number, l.id"#,
        )
        .bind(stored_auction_id)
        .fetch_all(&mut *transaction)
        .await
        .map_err(|error| PostgresReportError::database(error, "读取结算报告 lot 失败"))?;

        rows.into_iter()
            .map(|row| {
                let lot_id: Uuid =
                    row.try_get("id").map_err(|_| storage_error("读取结算 lot 标识失败"))?;
                let settlement_lot_id: Option<Uuid> = row
                    .try_get("settlement_lot_id")
                    .map_err(|_| storage_error("读取结算 lot 结算标识失败"))?;
                if settlement_lot_id != Some(lot_id) {
                    return Err(storage_error("结算报告缺少 lot 结算事实"));
                }
                let buyer_amount: Option<i64> = row
                    .try_get("buyer_amount")
                    .map_err(|_| storage_error("读取结算买方金额失败"))?;
                let listed_price: Option<i64> = row
                    .try_get("listed_price")
                    .map_err(|_| storage_error("读取结算挂牌金额失败"))?;
                let seller_amount: Option<i64> = row
                    .try_get("seller_amount")
                    .map_err(|_| storage_error("读取结算卖方金额失败"))?;
                let tax_amount: Option<i64> =
                    row.try_get("tax_amount").map_err(|_| storage_error("读取结算税额失败"))?;
                let compensation_amount: Option<i64> = row
                    .try_get("compensation_amount")
                    .map_err(|_| storage_error("读取结算补偿金额失败"))?;
                let hold_ppm: Option<i64> =
                    row.try_get("hold_ppm").map_err(|_| storage_error("读取结算持有比例失败"))?;
                let adjustment_ppm: Option<i64> = row
                    .try_get("adjustment_ppm")
                    .map_err(|_| storage_error("读取结算调整比例失败"))?;
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
                    .map_err(|_| storage_error("读取结算报告累计领先时长失败"))?;
                if leader_held_nanos < 0 {
                    return Err(storage_error("结算报告累计领先时长无效"));
                }
                let final_price: i64 = row
                    .try_get("final_price")
                    .map_err(|_| storage_error("读取结算成交金额失败"))?;
                let adjustment_metadata: serde_json::Value = row
                    .try_get("adjustment_metadata")
                    .map_err(|_| storage_error("读取结算 adjustment metadata 失败"))?;
                Ok(SettlementLotReport {
                    id: auction_domain::LotId::from_uuid(lot_id),
                    lot_number: row
                        .try_get("lot_number")
                        .map_err(|_| storage_error("读取结算 lot 序号失败"))?,
                    status: row
                        .try_get("status")
                        .map_err(|_| storage_error("读取结算 lot 状态失败"))?,
                    equipment_name: row
                        .try_get("equipment_name")
                        .map_err(|_| storage_error("读取结算装备名称失败"))?,
                    equipment_category: row
                        .try_get("equipment_category")
                        .map_err(|_| storage_error("读取结算装备分类失败"))?,
                    external_equipment_id: row
                        .try_get("external_equipment_id")
                        .map_err(|_| storage_error("读取结算装备外部标识失败"))?,
                    seller: row.try_get("seller").map_err(|_| storage_error("读取结算卖方失败"))?,
                    winner: row.try_get("winner").map_err(|_| storage_error("读取结算买方失败"))?,
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
        .map_err(|error| PostgresReportError::database(error, "提交结算报告快照失败"))?;

    Ok(Some(SettlementReport {
        auction: SettlementAuctionSnapshot {
            id: auction_id,
            world_id: row.try_get("world_id").map_err(|_| storage_error("读取结算报告世界失败"))?,
            sequence: row.try_get("sequence").map_err(|_| storage_error("读取结算报告序号失败"))?,
            status: row.try_get("status").map_err(|_| storage_error("读取结算报告状态失败"))?,
            preview_scheduled_at: row
                .try_get("preview_scheduled_at")
                .map_err(|_| storage_error("读取结算报告预览时间失败"))?,
            scheduled_open_at: row
                .try_get("scheduled_open_at")
                .map_err(|_| storage_error("读取结算报告开拍时间失败"))?,
            preview_opened_at: row
                .try_get("preview_opened_at")
                .map_err(|_| storage_error("读取结算报告预览开放时间失败"))?,
            preview_manual_open: row
                .try_get("preview_manual_open")
                .map_err(|_| storage_error("读取结算报告预览方式失败"))?,
            preview_revision: row
                .try_get("preview_revision")
                .map_err(|_| storage_error("读取结算报告预览版本失败"))?,
            opened_at: row
                .try_get("opened_at")
                .map_err(|_| storage_error("读取结算报告开放时间失败"))?,
            final_phase_at: row
                .try_get("final_phase_at")
                .map_err(|_| storage_error("读取结算报告终阶段时间失败"))?,
            settled_at: row
                .try_get("settled_at")
                .map_err(|_| storage_error("读取结算报告拍卖结算时间失败"))?,
            cancelled_at: row
                .try_get("cancelled_at")
                .map_err(|_| storage_error("读取结算报告取消时间失败"))?,
            cancelled_reason: row
                .try_get("cancelled_reason")
                .map_err(|_| storage_error("读取结算报告取消原因失败"))?,
            cancelled_by: row
                .try_get("cancelled_by")
                .map_err(|_| storage_error("读取结算报告取消人失败"))?,
            state_version: row
                .try_get("state_version")
                .map_err(|_| storage_error("读取结算报告状态版本失败"))?,
            scheduled_end_at: row
                .try_get("scheduled_end_at")
                .map_err(|_| storage_error("读取结算报告名义结束时间失败"))?,
            effective_end_at: row
                .try_get("effective_end_at")
                .map_err(|_| storage_error("读取结算报告有效结束时间失败"))?,
            created_at: row
                .try_get("created_at")
                .map_err(|_| storage_error("读取结算报告创建时间失败"))?,
            updated_at: row
                .try_get("updated_at")
                .map_err(|_| storage_error("读取结算报告更新时间失败"))?,
        },
        settlement,
        lots,
    }))
}

#[cfg(feature = "postgres")]
fn decode_settlement_adjustment(
    value: serde_json::Value,
) -> Result<SettlementAdjustment, ApplicationError> {
    let serde_json::Value::Object(fields) = value else {
        return if value.is_null() {
            Ok(SettlementAdjustment { policy: None })
        } else {
            Err(storage_error("结算 adjustment metadata 结构无效"))
        };
    };
    if fields.keys().any(|key| key != "policy") {
        return Err(storage_error("结算 adjustment metadata 含未知字段"));
    }
    let policy = match fields.get("policy") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(policy))
            if !policy.is_empty() && policy == policy.trim() =>
        {
            Some(policy.clone())
        }
        Some(_) => return Err(storage_error("结算 adjustment policy 无效")),
    };
    Ok(SettlementAdjustment { policy })
}

fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(auction_application::RepositoryError::Failed(message.to_owned()))
}
