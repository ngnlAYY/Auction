//! 结算事实、lot 关闭与补偿账本写入；事务边界由父模块统一装配。

use super::*;

/// 已校验的补偿池账本条目；ID、序号和余额在任何写入前全部算好。
struct CompensationLedgerEntry {
    id: Uuid,
    sequence: i32,
    movement_kind: &'static str,
    amount: i64,
    balance_after: i64,
    lot_id: Option<auction_domain::LotId>,
}

/// 把仍为 live 的 lot 按结算明细关闭；价格、领先者和状态必须与锁内计划保持一致。
pub(super) async fn close_settlement_lots(
    transaction: &mut PostgresCompetitionTransaction,
    auction_id: AuctionId,
    plan: &SettlementPlan,
    occurred_at: DateTime<Utc>,
) -> Result<(), SettlementTransactionError> {
    let lot_ids: Vec<Uuid> = plan.lines.iter().map(|line| line.lot_id.as_uuid()).collect();
    let next_statuses: Vec<&str> = plan
        .lines
        .iter()
        .map(|line| if line.end_price.value() == 0 { "unsold" } else { "sold" })
        .collect();
    let end_prices: Vec<i64> = plan.lines.iter().map(|line| line.end_price.value()).collect();
    let transaction = transaction.transaction_mut().map_err(map_competition_error)?;
    // 结算快照已按 lot_number 锁定全部 lot，单条语句不改变锁序；任一行的状态、领先者或价格
    // 与计划不符都会使更新行数小于明细数，整笔事务失败，计划中的重复 lot 同样无法凑足行数。
    let updated = sqlx::query(
        r#"UPDATE public.lots AS lot
           SET status = planned.next_status, slow_authorization_id = NULL, slow_due_at = NULL,
               state_version = lot.state_version + 1, updated_at = $5
           FROM UNNEST($1::uuid[], $2::text[], $3::bigint[])
               AS planned(lot_id, next_status, end_price)
           WHERE lot.id = planned.lot_id AND lot.auction_id = $4 AND lot.status = 'live'
             AND ((planned.next_status = 'sold' AND lot.leader_user_id IS NOT NULL
                   AND lot.current_price = planned.end_price)
               OR (planned.next_status = 'unsold' AND lot.leader_user_id IS NULL))"#,
    )
    .bind(&lot_ids)
    .bind(&next_statuses)
    .bind(&end_prices)
    .bind(auction_id.as_uuid())
    .bind(occurred_at)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    if updated.rows_affected() != plan.lines.len() as u64 {
        return Err(invalid("结算 lot 状态、领先者或价格与领域计划不一致"));
    }

    // 撤回记录不属于结算明细，但它们残留的活动代理同样必须阻止结算提交。
    // 活动授权的部分唯一索引保证每个 lot 至多匹配一行，计数无需 DISTINCT 排序或去重。
    let counts = sqlx::query(
        r#"SELECT
               COUNT(*)
                   FILTER (WHERE l.status <> 'withdrawn')::bigint AS lot_count,
               COUNT(*) FILTER (WHERE l.status = 'live')::bigint AS live_count,
               COUNT(p.authorization_id)::bigint AS active_proxy_count
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
    // 每批最多 2560 个参数；有界批量减少持有结算锁期间的往返，不拆分原 Serializable 事务。
    for lines in plan.lines.chunks(SETTLEMENT_BATCH_SIZE) {
        let transaction = transaction.transaction_mut().map_err(map_competition_error)?;
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new(
            r#"INSERT INTO public.lot_settlements
                   (lot_id, end_price, listed_price, buyer_amount, seller_amount,
                    tax_amount, compensation_amount, hold_ppm, adjustment_ppm, adjustment_metadata) "#,
        );
        // metadata 沿用只含 policy 的历史闭集；所有业务值只通过参数绑定进入 SQL。
        query.push_values(lines, |mut values, line| {
            values
                .push_bind(line.lot_id.as_uuid())
                .push_bind(line.end_price.value())
                .push_bind(line.listed_price.value())
                .push_bind(line.buyer_amount.value())
                .push_bind(line.seller_amount.value())
                .push_bind(line.tax_amount.value())
                .push_bind(line.compensation_amount.value())
                .push_bind(line.hold_ppm)
                .push_bind(line.adjustment_ppm)
                .push_bind(json!({ "policy": plan.policy_mode.as_str() }));
        });
        execute_settlement_batch(transaction, query, lines.len()).await?;
    }
    Ok(())
}

pub(super) async fn insert_compensation_ledger(
    transaction: &mut PostgresCompetitionTransaction,
    command_id: CommandId,
    plan: &SettlementPlan,
    occurred_at: DateTime<Utc>,
) -> Result<(), SettlementTransactionError> {
    let entries = compensation_ledger_entries(command_id, plan)?;
    // 每批最多 2048 个参数；auction_id 仍取自同事务命令账本，缺失账本时由 NOT NULL 约束拒绝。
    for entries in entries.chunks(SETTLEMENT_BATCH_SIZE) {
        let transaction = transaction.transaction_mut().map_err(map_competition_error)?;
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new(
            r#"INSERT INTO public.compensation_ledger
                   (id, auction_id, lot_id, entry_sequence, movement_kind, amount, balance_after,
                    created_at) "#,
        );
        query.push_values(entries, |mut values, entry| {
            values
                .push_bind(entry.id)
                .push("(SELECT aggregate_id FROM public.auction_commands WHERE command_id = ")
                .push_bind_unseparated(command_id.as_uuid())
                .push_unseparated(")")
                .push_bind(entry.lot_id.map(|id| id.as_uuid()))
                .push_bind(entry.sequence)
                .push_bind(entry.movement_kind)
                .push_bind(entry.amount)
                .push_bind(entry.balance_after)
                .push_bind(occurred_at);
        });
        execute_settlement_batch(transaction, query, entries.len()).await?;
    }
    Ok(())
}

/// 按旧版顺序构造期初、全部税费、全部补偿和期末条目；顺序与连续序号本身属于可审计事实。
fn compensation_ledger_entries(
    command_id: CommandId,
    plan: &SettlementPlan,
) -> Result<Vec<CompensationLedgerEntry>, SettlementTransactionError> {
    let movement_capacity = if plan.compensation_pool_enabled { plan.lines.len() * 2 } else { 0 };
    let mut entries = Vec::with_capacity(movement_capacity + 2);
    let mut balance = i128::from(plan.opening_pool.value());
    push_ledger_entry(
        &mut entries,
        command_id,
        "opening",
        plan.opening_pool.value(),
        balance,
        None,
    )?;

    if plan.compensation_pool_enabled {
        for line in plan.lines.iter().filter(|line| line.tax_amount.value() > 0) {
            balance = balance
                .checked_add(i128::from(line.tax_amount.value()))
                .ok_or_else(|| invalid("补偿池账本余额溢出"))?;
            let amount = line.tax_amount.value();
            push_ledger_entry(&mut entries, command_id, "tax", amount, balance, Some(line.lot_id))?;
        }
        for line in plan.lines.iter().filter(|line| line.compensation_amount.value() > 0) {
            balance = balance
                .checked_sub(i128::from(line.compensation_amount.value()))
                .ok_or_else(|| invalid("补偿池账本余额下溢"))?;
            let amount = -line.compensation_amount.value();
            let lot_id = Some(line.lot_id);
            push_ledger_entry(&mut entries, command_id, "compensation", amount, balance, lot_id)?;
        }
    }

    if balance != i128::from(plan.closing_pool.value()) {
        return Err(invalid("补偿池账本与结算计划不一致"));
    }
    push_ledger_entry(&mut entries, command_id, "closing", 0, balance, None)?;
    Ok(entries)
}

fn push_ledger_entry(
    entries: &mut Vec<CompensationLedgerEntry>,
    command_id: CommandId,
    movement_kind: &'static str,
    amount: i64,
    balance_after: i128,
    lot_id: Option<auction_domain::LotId>,
) -> Result<(), SettlementTransactionError> {
    let balance_after = i64::try_from(balance_after).map_err(|_| invalid("补偿池账本余额溢出"))?;
    if balance_after < 0 {
        return Err(invalid("补偿池账本余额不能为负"));
    }
    let sequence =
        i32::try_from(entries.len() + 1).map_err(|_| invalid("补偿池账本序号超出数据库范围"))?;
    entries.push(CompensationLedgerEntry {
        id: deterministic_uuid(command_id, &format!("settlement-ledger:{sequence}"), 1),
        sequence,
        movement_kind,
        amount,
        balance_after,
        lot_id,
    });
    Ok(())
}
