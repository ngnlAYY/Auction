//! 结算事务主流程与锁内计划应用。
//!
//! 该文件只承载 PostgreSQL 结算事务的单一职责，事务边界仍由父模块统一装配。

use super::*;

impl PostgresCompetitionTransaction {
    pub(super) async fn load_settlement_for_update(
        &mut self,
        auction_id: AuctionId,
    ) -> Result<SettlementSnapshot, SettlementTransactionError> {
        let auction_row = {
            let transaction = self.transaction_mut().map_err(map_competition_error)?;
            sqlx::query(
                r#"SELECT
                       a.world_id,
                       a.status,
                       a.state_version AS auction_state_version,
                       a.scheduled_open_at,
                       a.scheduled_end_at,
                       a.policy_snapshot,
                       p.balance AS opening_pool,
                       p.state_version AS opening_pool_version
                   FROM public.auctions AS a
                   INNER JOIN public.settlement_pools AS p ON p.world_id = a.world_id
                   WHERE a.id = $1 AND a.status IN ('settling', 'settled')
                   FOR UPDATE OF a, p"#,
            )
            .bind(auction_id.as_uuid())
            .fetch_optional(&mut **transaction)
            .await
            .map_err(map_sql_error)?
        };
        let Some(auction_row) = auction_row else {
            return Err(SettlementTransactionError::NotFound);
        };

        let auction_status: String = auction_row.try_get("status").map_err(map_sql_error)?;
        let world =
            settlement_world(auction_row.try_get::<String, _>("world_id").map_err(map_sql_error)?)?;
        let policy_snapshot: Value =
            auction_row.try_get("policy_snapshot").map_err(map_sql_error)?;
        let (policy, recovery_policy) = parse_settlement_policy(&policy_snapshot)?;
        let scheduled_open_at: DateTime<Utc> =
            auction_row.try_get("scheduled_open_at").map_err(map_sql_error)?;
        let scheduled_end_at: DateTime<Utc> =
            auction_row.try_get("scheduled_end_at").map_err(map_sql_error)?;
        let auction_state_version = positive_u64(
            auction_row.try_get::<i64, _>("auction_state_version").map_err(map_sql_error)?,
            "拍卖状态版本",
        )?;
        let opening_pool = SettlementAmount::new(
            auction_row.try_get::<i64, _>("opening_pool").map_err(map_sql_error)?,
        )
        .map_err(|_| invalid("结算池余额无效"))?;
        let opening_pool_version = positive_u64(
            auction_row.try_get::<i64, _>("opening_pool_version").map_err(map_sql_error)?,
            "结算池状态版本",
        )?;

        let rows = {
            let transaction = self.transaction_mut().map_err(map_competition_error)?;
            sqlx::query(
                r#"SELECT
                       l.id,
                       l.current_price,
                       l.leader_user_id,
                       l.leader_since,
                       l.effective_end_at,
                       l.equipment_id,
                       l.submission_id,
                       l.effective_end_at AS fact_end_at,
                       submission.seller
                   FROM public.lots AS l
                   INNER JOIN public.auctions AS a ON a.id = l.auction_id
                   LEFT JOIN public.auction_submissions AS submission
                       ON submission.id = l.submission_id
                   WHERE l.auction_id = $1
                     AND (($2 = 'settling' AND l.status = 'live')
                       OR ($2 = 'settled' AND l.status IN ('sold', 'unsold')))
                   ORDER BY l.lot_number, l.id
                   FOR UPDATE OF l"#,
            )
            .bind(auction_id.as_uuid())
            .bind(&auction_status)
            .fetch_all(&mut **transaction)
            .await
            .map_err(map_sql_error)?
        };

        let mut facts = Vec::with_capacity(rows.len());
        let mut lot_ids = Vec::with_capacity(rows.len());
        for row in rows {
            let lot_id = row
                .try_get::<Uuid, _>("id")
                .map(auction_domain::LotId::from_uuid)
                .map_err(map_sql_error)?;
            let leader_id = row
                .try_get::<Option<Uuid>, _>("leader_user_id")
                .map_err(map_sql_error)?
                .map(auction_domain::UserId::from_uuid);
            let end_price = row
                .try_get::<Option<i64>, _>("current_price")
                .map_err(map_sql_error)?
                .map(|value| SettlementAmount::new(value).map_err(|_| invalid("成交金额无效")))
                .transpose()?;
            let seller_identity: String = row
                .try_get::<Option<String>, _>("seller")
                .map_err(map_sql_error)?
                .ok_or_else(|| invalid("结算 lot 缺少卖方身份"))?;
            let fact_end_at: DateTime<Utc> = row.try_get("fact_end_at").map_err(map_sql_error)?;
            let fact = SettlementFact {
                lot_id,
                world,
                end_price: leader_id.and(end_price),
                leader_id,
                leader_since: row.try_get("leader_since").map_err(map_sql_error)?,
                effective_end_at: fact_end_at,
                seller_identity,
                equipment_id: row
                    .try_get::<Uuid, _>("equipment_id")
                    .map(auction_domain::EquipmentId::from_uuid)
                    .map_err(map_sql_error)?,
                submission_id: row
                    .try_get::<Option<Uuid>, _>("submission_id")
                    .map_err(map_sql_error)?
                    .map(auction_domain::SubmissionId::from_uuid),
            };
            lot_ids.push(lot_id);
            facts.push(fact);
        }

        let mut lot_competitions = Vec::new();
        if auction_status == "settling" {
            lot_competitions.reserve(lot_ids.len());
            for lot_id in lot_ids {
                lot_competitions
                    .push(self.load_for_update_inner(lot_id).await.map_err(map_competition_error)?);
            }
        }

        Ok(SettlementSnapshot {
            auction_id,
            auction_state_version,
            policy,
            recovery_policy,
            scheduled_open_at,
            scheduled_end_at,
            opening_pool,
            opening_pool_version,
            facts,
            lot_competitions,
        })
    }

    pub(super) async fn apply_settlement_inner(
        &mut self,
        auction_id: AuctionId,
        command_id: CommandId,
        plan: SettlementPlan,
        proxy_closure_plans: Vec<CompetitionPlan>,
        recovery_policy: RecoveryPolicy,
    ) -> Result<(), SettlementTransactionError> {
        recovery_policy.validate().map_err(|_| invalid("恢复策略快照校验失败"))?;
        for line in &plan.lines {
            line.validate().map_err(|_| invalid("结算明细校验失败"))?;
        }
        for obligation in &plan.obligations {
            obligation.validate().map_err(|_| invalid("结算义务校验失败"))?;
        }
        validate_proxy_closure_plans(command_id, &plan, &proxy_closure_plans)?;

        let (world_id, auction_state_version) = {
            let transaction = self.transaction_mut().map_err(map_competition_error)?;
            let row = sqlx::query(
                r#"SELECT world_id, state_version
                   FROM public.auctions
                   WHERE id = $1 AND status = 'settling'
                   FOR UPDATE"#,
            )
            .bind(auction_id.as_uuid())
            .fetch_optional(&mut **transaction)
            .await
            .map_err(map_sql_error)?
            .ok_or(SettlementTransactionError::NotFound)?;
            (
                row.try_get::<String, _>("world_id").map_err(map_sql_error)?,
                positive_u64(
                    row.try_get::<i64, _>("state_version").map_err(map_sql_error)?,
                    "拍卖状态版本",
                )?,
            )
        };
        let current_pool_version = {
            let transaction = self.transaction_mut().map_err(map_competition_error)?;
            let row = sqlx::query(
                r#"SELECT balance, state_version
                   FROM public.settlement_pools
                   WHERE world_id = $1
                   FOR UPDATE"#,
            )
            .bind(&world_id)
            .fetch_one(&mut **transaction)
            .await
            .map_err(map_sql_error)?;
            let current_balance: i64 = row.try_get("balance").map_err(map_sql_error)?;
            if current_balance != plan.opening_pool.value() {
                return Err(invalid("结算池余额与结算计划不一致"));
            }
            positive_u64(row.try_get("state_version").map_err(map_sql_error)?, "结算池状态版本")?
        };
        let closing_pool_version =
            current_pool_version.checked_add(1).ok_or_else(|| invalid("结算池状态版本溢出"))?;

        // 代理终态、lot 成交/流拍状态与金额事实必须在同一 Serializable 事务内提交。
        for closure_plan in proxy_closure_plans {
            self.apply_plan_inner(closure_plan, None).await.map_err(map_competition_error)?;
        }
        close_settlement_lots(self, auction_id, &plan).await?;

        {
            let transaction = self.transaction_mut().map_err(map_competition_error)?;
            let result = sqlx::query(
                r#"UPDATE public.settlement_pools
                   SET balance = $2, state_version = $3, updated_at = CURRENT_TIMESTAMP
                   WHERE world_id = $1 AND balance = $4 AND state_version = $5"#,
            )
            .bind(&world_id)
            .bind(plan.closing_pool.value())
            .bind(i64_version(closing_pool_version, "结算池状态版本")?)
            .bind(plan.opening_pool.value())
            .bind(i64_version(current_pool_version, "结算池状态版本")?)
            .execute(&mut **transaction)
            .await
            .map_err(map_sql_error)?;
            if result.rows_affected() != 1 {
                return Err(invalid("结算池版本与数据库不一致"));
            }

            sqlx::query(
                r#"INSERT INTO public.auction_settlements
                       (auction_id, command_id, policy_version, policy_mode, pool_enabled,
                        opening_pool_balance, closing_pool_balance, tax_total, discount_demand,
                        compensation_total, opening_pool_version, closing_pool_version, settled_at)
                   VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, CURRENT_TIMESTAMP)"#,
            )
            .bind(auction_id.as_uuid())
            .bind(command_id.as_uuid())
            .bind(i32_version(plan.policy_version, "结算策略版本")?)
            .bind(settlement_mode(plan.policy_mode))
            .bind(plan.compensation_pool_enabled)
            .bind(plan.opening_pool.value())
            .bind(plan.closing_pool.value())
            .bind(plan.tax_total.value())
            .bind(plan.discount_demand.value())
            .bind(plan.compensation_total.value())
            .bind(i64_version(current_pool_version, "结算池状态版本")?)
            .bind(i64_version(closing_pool_version, "结算池状态版本")?)
            .execute(&mut **transaction)
            .await
            .map_err(map_sql_error)?;
        }

        insert_lot_settlements(self, &plan).await?;
        insert_compensation_ledger(self, command_id, &plan).await?;
        insert_obligations(self, &plan).await?;
        insert_recoveries(self, auction_id, &plan, recovery_policy).await?;
        insert_settlement_audit(self, auction_id, command_id, auction_state_version).await?;
        let next_version =
            auction_state_version.checked_add(1).ok_or_else(|| invalid("拍卖状态版本溢出"))?;

        {
            let transaction = self.transaction_mut().map_err(map_competition_error)?;
            let result = sqlx::query(
                r#"UPDATE public.auctions
                   SET status = 'settled', settled_at = CURRENT_TIMESTAMP,
                       state_version = $2, updated_at = CURRENT_TIMESTAMP
                   WHERE id = $1 AND status = 'settling' AND state_version = $3"#,
            )
            .bind(auction_id.as_uuid())
            .bind(i64_version(next_version, "拍卖状态版本")?)
            .bind(i64_version(auction_state_version, "拍卖状态版本")?)
            .execute(&mut **transaction)
            .await
            .map_err(map_sql_error)?;
            if result.rows_affected() != 1 {
                return Err(invalid("拍卖状态版本与数据库不一致"));
            }
        }

        insert_settlement_realtime(self, auction_id, command_id, next_version).await
    }
}

pub(super) fn validate_proxy_closure_plans(
    command_id: CommandId,
    settlement_plan: &SettlementPlan,
    closure_plans: &[CompetitionPlan],
) -> Result<(), SettlementTransactionError> {
    for (index, plan) in closure_plans.iter().enumerate() {
        plan.validate().map_err(|_| invalid("结算代理关闭计划校验失败"))?;
        let transition = match (plan.proxy_transitions.as_slice(), plan.proxy_states.as_slice()) {
            ([transition], [state]) => (transition, state),
            _ => return Err(invalid("结算代理关闭计划必须只包含一个终态转换")),
        };
        if !plan.public_bids.is_empty()
            || plan.active_proxy_id.is_some()
            || transition.0.command_id != command_id
            || transition.0.new_status != ProxyStatus::Closed
            || transition.1.status != ProxyStatus::Closed
            || transition.1.lot_id != plan.final_lot.lot_id
            || !settlement_plan.lines.iter().any(|line| line.lot_id == plan.final_lot.lot_id)
            || closure_plans[..index]
                .iter()
                .any(|previous| previous.final_lot.lot_id == plan.final_lot.lot_id)
        {
            return Err(invalid("结算代理关闭计划与结算命令或 lot 不一致"));
        }
    }
    Ok(())
}
