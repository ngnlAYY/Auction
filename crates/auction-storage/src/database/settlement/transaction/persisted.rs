//! 已提交结算的持久态装载：重放命令时按冻结事实重建结算结果。

use crate::database::PostgresCompetitionTransaction;
use crate::database::settlement::{
    invalid, map_competition_error, map_sql_error, parse_obligation_kind, parse_settlement_mode,
    positive_u64, settlement_amount,
};
use auction_application::settlement::{
    PersistedSettlement, PersistedSettlementObligationCount, SettlementTransactionError,
};
use auction_domain::{AuctionId, CommandId, SettlementLine};
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

impl PostgresCompetitionTransaction {
    pub(super) async fn load_persisted_settlement(
        &mut self,
        auction_id: AuctionId,
        auction_state_version: u64,
        auction_settled_at: Option<DateTime<Utc>>,
        workflow_complete_command_id: Option<CommandId>,
    ) -> Result<PersistedSettlement, SettlementTransactionError> {
        // 已结算事实只追加（触发器拒绝 UPDATE/DELETE，运行账号也只有 INSERT），且本事务已在
        // `load_for_update` 开头以 `FOR UPDATE OF a, p` 锁住场次与补偿池；对这两张表再加行锁
        // 不增加保护，反而需要运行账号没有的 UPDATE 权限（42501）。这里只做普通快照读取。
        let summary = {
            let transaction = self.transaction_mut().map_err(map_competition_error)?;
            let query = r#"SELECT command_id, policy_version, policy_mode, pool_enabled,
                          opening_pool_balance, closing_pool_balance, opening_pool_version,
                          closing_pool_version, tax_total, discount_demand, compensation_total,
                          settled_at
                   FROM public.auction_settlements
                   WHERE auction_id = $1"#;
            sqlx::query(query)
                .bind(auction_id.as_uuid())
                .fetch_optional(&mut **transaction)
                .await
                .map_err(map_sql_error)?
                .ok_or_else(|| invalid("已结算拍卖缺少结算摘要"))?
        };
        let command_id = summary
            .try_get::<Uuid, _>("command_id")
            .map(CommandId::from_uuid)
            .map_err(map_sql_error)?;
        let settled_at: DateTime<Utc> = summary.try_get("settled_at").map_err(map_sql_error)?;
        if auction_settled_at != Some(settled_at) {
            return Err(invalid("拍卖终态时间与结算摘要不一致"));
        }
        let policy_version =
            u32::try_from(summary.try_get::<i32, _>("policy_version").map_err(map_sql_error)?)
                .map_err(|_| invalid("结算摘要策略版本无效"))?;
        let policy_mode = parse_settlement_mode(
            summary.try_get::<String, _>("policy_mode").map_err(map_sql_error)?,
        )?;
        let opening_pool = settlement_amount(
            summary.try_get("opening_pool_balance").map_err(map_sql_error)?,
            "结算摘要期初余额",
        )?;
        let closing_pool = settlement_amount(
            summary.try_get("closing_pool_balance").map_err(map_sql_error)?,
            "结算摘要期末余额",
        )?;
        let tax_total = settlement_amount(
            summary.try_get("tax_total").map_err(map_sql_error)?,
            "结算摘要税费总额",
        )?;
        let discount_demand = settlement_amount(
            summary.try_get("discount_demand").map_err(map_sql_error)?,
            "结算摘要折扣需求",
        )?;
        let compensation_total = settlement_amount(
            summary.try_get("compensation_total").map_err(map_sql_error)?,
            "结算摘要补偿总额",
        )?;

        let line_rows = {
            let transaction = self.transaction_mut().map_err(map_competition_error)?;
            let query = r#"SELECT settlement.lot_id, settlement.end_price, settlement.listed_price,
                          settlement.buyer_amount, settlement.seller_amount,
                          settlement.tax_amount, settlement.compensation_amount,
                          settlement.hold_ppm, settlement.adjustment_ppm,
                          lot.status
                   FROM public.lot_settlements AS settlement
                   INNER JOIN public.lots AS lot ON lot.id = settlement.lot_id
                   WHERE lot.auction_id = $1
                   ORDER BY lot.lot_number, lot.id"#;
            sqlx::query(query)
                .bind(auction_id.as_uuid())
                .fetch_all(&mut **transaction)
                .await
                .map_err(map_sql_error)?
        };
        if line_rows.is_empty() {
            return Err(invalid("已结算拍卖缺少 lot 结算事实"));
        }
        let mut lines = Vec::with_capacity(line_rows.len());
        let mut sold_count = 0_u64;
        let mut unsold_count = 0_u64;
        for row in line_rows {
            match row.try_get::<String, _>("status").map_err(map_sql_error)?.as_str() {
                "sold" => {
                    sold_count =
                        sold_count.checked_add(1).ok_or_else(|| invalid("成交数量溢出"))?;
                }
                "unsold" => {
                    unsold_count =
                        unsold_count.checked_add(1).ok_or_else(|| invalid("流拍数量溢出"))?;
                }
                _ => return Err(invalid("已结算 lot 状态无效")),
            }
            let line = SettlementLine {
                lot_id: row
                    .try_get::<Uuid, _>("lot_id")
                    .map(auction_domain::LotId::from_uuid)
                    .map_err(map_sql_error)?,
                end_price: settlement_amount(
                    row.try_get("end_price").map_err(map_sql_error)?,
                    "lot 结算成交价",
                )?,
                listed_price: settlement_amount(
                    row.try_get("listed_price").map_err(map_sql_error)?,
                    "lot 结算列示价",
                )?,
                buyer_amount: settlement_amount(
                    row.try_get("buyer_amount").map_err(map_sql_error)?,
                    "lot 结算买方金额",
                )?,
                seller_amount: settlement_amount(
                    row.try_get("seller_amount").map_err(map_sql_error)?,
                    "lot 结算卖方金额",
                )?,
                tax_amount: settlement_amount(
                    row.try_get("tax_amount").map_err(map_sql_error)?,
                    "lot 结算税费",
                )?,
                compensation_amount: settlement_amount(
                    row.try_get("compensation_amount").map_err(map_sql_error)?,
                    "lot 结算补偿",
                )?,
                hold_ppm: row.try_get("hold_ppm").map_err(map_sql_error)?,
                adjustment_ppm: row.try_get("adjustment_ppm").map_err(map_sql_error)?,
            };
            line.validate().map_err(|_| invalid("lot 结算事实无效"))?;
            lines.push(line);
        }

        let obligation_rows = {
            let transaction = self.transaction_mut().map_err(map_competition_error)?;
            sqlx::query(
                r#"SELECT obligation_kind, COUNT(*)::bigint AS obligation_count
                   FROM public.settlement_obligations
                   WHERE auction_id = $1
                   GROUP BY obligation_kind
                   ORDER BY obligation_kind"#,
            )
            .bind(auction_id.as_uuid())
            .fetch_all(&mut **transaction)
            .await
            .map_err(map_sql_error)?
        };
        if obligation_rows.is_empty() {
            return Err(invalid("已结算拍卖缺少结算义务"));
        }
        let mut obligation_counts = Vec::with_capacity(obligation_rows.len());
        for row in obligation_rows {
            obligation_counts.push(PersistedSettlementObligationCount {
                kind: parse_obligation_kind(
                    row.try_get::<String, _>("obligation_kind").map_err(map_sql_error)?,
                )?,
                count: positive_u64(
                    row.try_get("obligation_count").map_err(map_sql_error)?,
                    "结算义务数量",
                )?,
            });
        }

        Ok(PersistedSettlement {
            command_id,
            workflow_complete_command_id,
            policy_version,
            policy_mode,
            pool_enabled: summary.try_get("pool_enabled").map_err(map_sql_error)?,
            opening_pool,
            closing_pool,
            opening_pool_version: positive_u64(
                summary.try_get("opening_pool_version").map_err(map_sql_error)?,
                "结算摘要期初池版本",
            )?,
            closing_pool_version: positive_u64(
                summary.try_get("closing_pool_version").map_err(map_sql_error)?,
                "结算摘要期末池版本",
            )?,
            tax_total,
            discount_demand,
            compensation_total,
            sold_count,
            unsold_count,
            obligation_counts,
            state_version: auction_state_version,
            settled_at,
            lines,
        })
    }
}
