//! 结算事实、lot 关闭与补偿账本写入；事务边界由父模块统一装配。

use super::*;

struct CompensationLedgerEntry<'a> {
    command_id: CommandId,
    sequence: i32,
    movement_kind: &'a str,
    amount: i64,
    balance_after: i128,
    lot_id: Option<auction_domain::LotId>,
    occurred_at: DateTime<Utc>,
}

/// 把仍为 live 的 lot 按结算明细关闭；价格、领先者和状态必须与锁内计划保持一致。
pub(super) async fn close_settlement_lots(
    transaction: &mut PostgresCompetitionTransaction,
    auction_id: AuctionId,
    plan: &SettlementPlan,
    occurred_at: DateTime<Utc>,
) -> Result<(), SettlementTransactionError> {
    for line in &plan.lines {
        let next_status = if line.end_price.value() == 0 { "unsold" } else { "sold" };
        let transaction = transaction.transaction_mut().map_err(map_competition_error)?;
        let updated = sqlx::query(
            r#"UPDATE public.lots
               SET status = $3, slow_authorization_id = NULL, slow_due_at = NULL,
                   state_version = state_version + 1, updated_at = $5
               WHERE id = $1 AND auction_id = $2 AND status = 'live'
                 AND (($3 = 'sold' AND leader_user_id IS NOT NULL AND current_price = $4)
                   OR ($3 = 'unsold' AND leader_user_id IS NULL))"#,
        )
        .bind(line.lot_id.as_uuid())
        .bind(auction_id.as_uuid())
        .bind(next_status)
        .bind(line.end_price.value())
        .bind(occurred_at)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
        if updated.rows_affected() != 1 {
            return Err(invalid("结算 lot 状态、领先者或价格与领域计划不一致"));
        }
    }

    let transaction = transaction.transaction_mut().map_err(map_competition_error)?;
    // 撤回记录不属于结算明细，但它们残留的活动代理同样必须阻止结算提交。
    // 只关联仍活动的授权，沿用其部分索引；终态历史不能放大 lot 汇总的中间结果。
    let counts = sqlx::query(
        r#"SELECT
               COUNT(DISTINCT l.id)
                   FILTER (WHERE l.status <> 'withdrawn')::bigint AS lot_count,
               COUNT(DISTINCT l.id) FILTER (WHERE l.status = 'live')::bigint AS live_count,
               COUNT(DISTINCT p.authorization_id)::bigint AS active_proxy_count
           FROM public.lots AS l
           LEFT JOIN public.bid_proxy_authorizations AS p
             ON p.lot_id = l.id AND p.status IN ('active', 'final_fast')
           WHERE l.auction_id = $1"#,
    )
    .bind(auction_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let lot_count: i64 = counts.try_get("lot_count").map_err(map_sql_error)?;
    let live_count: i64 = counts.try_get("live_count").map_err(map_sql_error)?;
    let active_proxy_count: i64 = counts.try_get("active_proxy_count").map_err(map_sql_error)?;
    let planned_lot_count =
        i64::try_from(plan.lines.len()).map_err(|_| invalid("结算 lot 数量超出数据库范围"))?;
    if planned_lot_count == 0
        || lot_count != planned_lot_count
        || live_count != 0
        || active_proxy_count != 0
    {
        return Err(invalid("结算后仍有未关闭 lot 或活动代理授权"));
    }
    Ok(())
}

pub(super) async fn insert_lot_settlements(
    transaction: &mut PostgresCompetitionTransaction,
    plan: &SettlementPlan,
) -> Result<(), SettlementTransactionError> {
    for line in &plan.lines {
        let transaction = transaction.transaction_mut().map_err(map_competition_error)?;
        sqlx::query(
            r#"INSERT INTO public.lot_settlements
                   (lot_id, end_price, listed_price, buyer_amount, seller_amount,
                    tax_amount, compensation_amount, hold_ppm, adjustment_ppm, adjustment_metadata)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)"#,
        )
        .bind(line.lot_id.as_uuid())
        .bind(line.end_price.value())
        .bind(line.listed_price.value())
        .bind(line.buyer_amount.value())
        .bind(line.seller_amount.value())
        .bind(line.tax_amount.value())
        .bind(line.compensation_amount.value())
        .bind(line.hold_ppm)
        .bind(line.adjustment_ppm)
        // 旧版 adjustment metadata 是只允许 `policy` 的严格闭集；策略版本属于结算摘要，
        // 不能扩散进 lot metadata，否则旧版/新版严格报告解码器都会拒绝正常结算事实。
        .bind(json!({ "policy": plan.policy_mode.as_str() }))
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
    }
    Ok(())
}

pub(super) async fn insert_compensation_ledger(
    transaction: &mut PostgresCompetitionTransaction,
    command_id: CommandId,
    plan: &SettlementPlan,
    occurred_at: DateTime<Utc>,
) -> Result<(), SettlementTransactionError> {
    let mut sequence: i32 = 1;
    let mut balance = plan.opening_pool.value() as i128;
    insert_ledger_entry(
        transaction,
        CompensationLedgerEntry {
            command_id,
            sequence,
            movement_kind: "opening",
            amount: plan.opening_pool.value(),
            balance_after: balance,
            lot_id: None,
            occurred_at,
        },
    )
    .await?;
    sequence += 1;

    if plan.compensation_pool_enabled {
        // 旧版账本先追加全部税费，再追加全部补偿；顺序本身属于可审计事实。
        for line in &plan.lines {
            if line.tax_amount.value() > 0 {
                balance = balance
                    .checked_add(i128::from(line.tax_amount.value()))
                    .ok_or_else(|| invalid("补偿池账本余额溢出"))?;
                insert_ledger_entry(
                    transaction,
                    CompensationLedgerEntry {
                        command_id,
                        sequence,
                        movement_kind: "tax",
                        amount: line.tax_amount.value(),
                        balance_after: balance,
                        lot_id: Some(line.lot_id),
                        occurred_at,
                    },
                )
                .await?;
                sequence += 1;
            }
        }
        for line in &plan.lines {
            if line.compensation_amount.value() > 0 {
                balance = balance
                    .checked_sub(i128::from(line.compensation_amount.value()))
                    .ok_or_else(|| invalid("补偿池账本余额下溢"))?;
                insert_ledger_entry(
                    transaction,
                    CompensationLedgerEntry {
                        command_id,
                        sequence,
                        movement_kind: "compensation",
                        amount: -line.compensation_amount.value(),
                        balance_after: balance,
                        lot_id: Some(line.lot_id),
                        occurred_at,
                    },
                )
                .await?;
                sequence += 1;
            }
        }
    }

    let closing = i128::from(plan.closing_pool.value());
    if balance != closing {
        return Err(invalid("补偿池账本与结算计划不一致"));
    }
    insert_ledger_entry(
        transaction,
        CompensationLedgerEntry {
            command_id,
            sequence,
            movement_kind: "closing",
            amount: 0,
            balance_after: balance,
            lot_id: None,
            occurred_at,
        },
    )
    .await
}

async fn insert_ledger_entry(
    transaction: &mut PostgresCompetitionTransaction,
    entry: CompensationLedgerEntry<'_>,
) -> Result<(), SettlementTransactionError> {
    let balance_after =
        i64::try_from(entry.balance_after).map_err(|_| invalid("补偿池账本余额溢出"))?;
    if balance_after < 0 {
        return Err(invalid("补偿池账本余额不能为负"));
    }
    let id =
        deterministic_uuid(entry.command_id, &format!("settlement-ledger:{}", entry.sequence), 1);
    let transaction = transaction.transaction_mut().map_err(map_competition_error)?;
    sqlx::query(
        r#"INSERT INTO public.compensation_ledger
               (id, auction_id, lot_id, entry_sequence, movement_kind, amount, balance_after,
                created_at)
           VALUES ($1, (SELECT aggregate_id FROM public.auction_commands WHERE command_id = $2),
                   $3, $4, $5, $6, $7, $8)"#,
    )
    .bind(id)
    .bind(entry.command_id.as_uuid())
    .bind(entry.lot_id.map(|id| id.as_uuid()))
    .bind(entry.sequence)
    .bind(entry.movement_kind)
    .bind(entry.amount)
    .bind(balance_after)
    .bind(entry.occurred_at)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}
