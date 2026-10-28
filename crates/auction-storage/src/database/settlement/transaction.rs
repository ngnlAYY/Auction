//! 结算事务主流程与锁内计划应用；事务边界由父模块统一装配。

use std::collections::HashSet;

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettlementSnapshotMode {
    ForUpdate,
    ReadOnlySettled,
}

impl PostgresCompetitionTransaction {
    pub(super) async fn insert_settlement_command(
        &mut self,
        command: StoredCommand,
        occurred_at: DateTime<Utc>,
    ) -> Result<(), SettlementTransactionError> {
        command.validate().map_err(SettlementTransactionError::from)?;
        let payload: Value = serde_json::from_slice(&command.result_payload)
            .map_err(|_| invalid("结算命令结果不是 JSON 对象"))?;
        let inserted = {
            let transaction = self.transaction_mut().map_err(map_competition_error)?;
            sqlx::query(
                r#"INSERT INTO public.auction_commands
                       (command_id, command_kind, aggregate_id, request_hash, result_code,
                        result_payload, created_at)
                   VALUES ($1, $2, $3, $4, $5, $6, $7)
                   ON CONFLICT (command_id) DO NOTHING"#,
            )
            .bind(command.command_id.as_uuid())
            .bind(&command.command_kind)
            .bind(command.aggregate_id)
            .bind(command.request_hash.as_bytes().as_slice())
            .bind(&command.result_code)
            .bind(payload)
            .bind(occurred_at)
            .execute(&mut **transaction)
            .await
            .map_err(map_sql_error)?
        };
        if inserted.rows_affected() == 1 {
            return Ok(());
        }
        let existing = self
            .find_command_inner(command.command_id)
            .await
            .map_err(map_competition_error)?
            .ok_or_else(|| invalid("结算命令冲突后无法读取账本"))?;
        if existing == command { Ok(()) } else { Err(CommandLedgerError::Conflict.into()) }
    }

    pub(super) async fn load_settlement_for_update(
        &mut self,
        auction_id: AuctionId,
    ) -> Result<SettlementSnapshot, SettlementTransactionError> {
        self.load_settlement_snapshot(auction_id, SettlementSnapshotMode::ForUpdate).await
    }

    /// 在调用方已经建立的只读事务中加载 settled 权威快照，不获取行锁也不允许 settling 写路径。
    pub(crate) async fn load_settled_snapshot_read_only(
        &mut self,
        auction_id: AuctionId,
    ) -> Result<SettlementSnapshot, SettlementTransactionError> {
        self.load_settlement_snapshot(auction_id, SettlementSnapshotMode::ReadOnlySettled).await
    }

    async fn load_settlement_snapshot(
        &mut self,
        auction_id: AuctionId,
        mode: SettlementSnapshotMode,
    ) -> Result<SettlementSnapshot, SettlementTransactionError> {
        let auction_row = {
            let transaction = self.transaction_mut().map_err(map_competition_error)?;
            let query = match mode {
                SettlementSnapshotMode::ForUpdate => {
                    r#"SELECT
                       a.world_id,
                       a.status,
                       a.state_version AS auction_state_version,
                       a.scheduled_open_at,
                       a.scheduled_end_at,
                       a.settled_at,
                       a.settlement_workflow_id,
                       a.settlement_claim_command_id,
                       a.settlement_due_at,
                       a.settlement_complete_command_id,
                       a.policy_snapshot,
                       p.balance AS opening_pool,
                       p.state_version AS opening_pool_version
                   FROM public.auctions AS a
                   INNER JOIN public.settlement_pools AS p ON p.world_id = a.world_id
                   WHERE a.id = $1 AND a.status IN ('settling', 'settled')
                   FOR UPDATE OF a, p"#
                }
                SettlementSnapshotMode::ReadOnlySettled => {
                    r#"SELECT
                       a.world_id,
                       a.status,
                       a.state_version AS auction_state_version,
                       a.scheduled_open_at,
                       a.scheduled_end_at,
                       a.settled_at,
                       a.settlement_workflow_id,
                       a.settlement_claim_command_id,
                       a.settlement_due_at,
                       a.settlement_complete_command_id,
                       a.policy_snapshot,
                       p.balance AS opening_pool,
                       p.state_version AS opening_pool_version
                   FROM public.auctions AS a
                   INNER JOIN public.settlement_pools AS p ON p.world_id = a.world_id
                   WHERE a.id = $1 AND a.status = 'settled'"#
                }
            };
            sqlx::query(query)
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
            let query = match mode {
                SettlementSnapshotMode::ForUpdate => {
                    r#"SELECT
                       l.id,
                       l.current_price,
                       l.leader_user_id,
                       l.leader_since,
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
                   FOR UPDATE OF l"#
                }
                SettlementSnapshotMode::ReadOnlySettled => {
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
                   ORDER BY l.lot_number, l.id"#
                }
            };
            sqlx::query(query)
                .bind(auction_id.as_uuid())
                .bind(&auction_status)
                .fetch_all(&mut **transaction)
                .await
                .map_err(map_sql_error)?
        };

        let mut facts = Vec::with_capacity(rows.len());
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
            // 新结算按追加式 bid 事实累计领先；旧场次只读核验沿用冻结时的最终连续区间，
            // 避免用新语义重算历史 hold_ppm 让既有结算证明失配。
            let leader_held_nanos = match leader_id {
                Some(_) if mode == SettlementSnapshotMode::ReadOnlySettled => {
                    let leader_since: Option<DateTime<Utc>> =
                        row.try_get("leader_since").map_err(map_sql_error)?;
                    leader_since
                        .and_then(|since| {
                            fact_end_at.signed_duration_since(since).num_nanoseconds()
                        })
                        .filter(|held| *held >= 0)
                }
                Some(_) => Some(
                    row.try_get::<Option<i64>, _>("leader_held_nanos")
                        .map_err(map_sql_error)?
                        .ok_or_else(|| invalid("累计领先时长无效"))?,
                ),
                None => None,
            };
            let fact = SettlementFact {
                lot_id,
                world,
                end_price: leader_id.and(end_price),
                leader_id,
                leader_since: row.try_get("leader_since").map_err(map_sql_error)?,
                leader_held_nanos,
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
            facts.push(fact);
        }

        let mut lot_competitions = Vec::new();
        if auction_status == "settling" {
            lot_competitions.reserve(facts.len());
            for fact in &facts {
                lot_competitions.push(
                    self.load_for_settlement_inner(fact.lot_id)
                        .await
                        .map_err(map_competition_error)?,
                );
            }
        }
        let persisted_settlement = if auction_status == "settled" {
            Some(
                self.load_persisted_settlement(
                    auction_id,
                    auction_state_version,
                    auction_row.try_get("settled_at").map_err(map_sql_error)?,
                    auction_row
                        .try_get::<Option<Uuid>, _>("settlement_complete_command_id")
                        .map_err(map_sql_error)?
                        .map(CommandId::from_uuid),
                    mode,
                )
                .await?,
            )
        } else {
            None
        };

        Ok(SettlementSnapshot {
            auction_id,
            workflow_id: auction_row.try_get("settlement_workflow_id").map_err(map_sql_error)?,
            claim_command_id: auction_row
                .try_get::<Option<Uuid>, _>("settlement_claim_command_id")
                .map_err(map_sql_error)?
                .map(CommandId::from_uuid),
            due_at: auction_row.try_get("settlement_due_at").map_err(map_sql_error)?,
            auction_state_version,
            policy,
            recovery_policy,
            scheduled_open_at,
            scheduled_end_at,
            opening_pool,
            opening_pool_version,
            facts,
            lot_competitions,
            persisted_settlement,
        })
    }

    async fn load_persisted_settlement(
        &mut self,
        auction_id: AuctionId,
        auction_state_version: u64,
        auction_settled_at: Option<DateTime<Utc>>,
        workflow_complete_command_id: Option<CommandId>,
        mode: SettlementSnapshotMode,
    ) -> Result<PersistedSettlement, SettlementTransactionError> {
        let summary = {
            let transaction = self.transaction_mut().map_err(map_competition_error)?;
            let query = match mode {
                SettlementSnapshotMode::ForUpdate => {
                    r#"SELECT command_id, policy_version, policy_mode, pool_enabled,
                          opening_pool_balance, closing_pool_balance, opening_pool_version,
                          closing_pool_version, tax_total, discount_demand, compensation_total,
                          settled_at
                   FROM public.auction_settlements
                   WHERE auction_id = $1
                   FOR UPDATE"#
                }
                SettlementSnapshotMode::ReadOnlySettled => {
                    r#"SELECT command_id, policy_version, policy_mode, pool_enabled,
                          opening_pool_balance, closing_pool_balance, opening_pool_version,
                          closing_pool_version, tax_total, discount_demand, compensation_total,
                          settled_at
                   FROM public.auction_settlements
                   WHERE auction_id = $1"#
                }
            };
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
            let query = match mode {
                SettlementSnapshotMode::ForUpdate => {
                    r#"SELECT settlement.lot_id, settlement.end_price, settlement.listed_price,
                          settlement.buyer_amount, settlement.seller_amount,
                          settlement.tax_amount, settlement.compensation_amount,
                          settlement.hold_ppm, settlement.adjustment_ppm,
                          lot.status
                   FROM public.lot_settlements AS settlement
                   INNER JOIN public.lots AS lot ON lot.id = settlement.lot_id
                   WHERE lot.auction_id = $1
                   ORDER BY lot.lot_number, lot.id
                   FOR UPDATE OF settlement"#
                }
                SettlementSnapshotMode::ReadOnlySettled => {
                    r#"SELECT settlement.lot_id, settlement.end_price, settlement.listed_price,
                          settlement.buyer_amount, settlement.seller_amount,
                          settlement.tax_amount, settlement.compensation_amount,
                          settlement.hold_ppm, settlement.adjustment_ppm,
                          lot.status
                   FROM public.lot_settlements AS settlement
                   INNER JOIN public.lots AS lot ON lot.id = settlement.lot_id
                   WHERE lot.auction_id = $1
                   ORDER BY lot.lot_number, lot.id"#
                }
            };
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
                    sold_count = sold_count.checked_add(1).ok_or_else(|| invalid("成交数量溢出"))?
                }
                "unsold" => {
                    unsold_count =
                        unsold_count.checked_add(1).ok_or_else(|| invalid("流拍数量溢出"))?
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

    pub(super) async fn apply_settlement_inner(
        &mut self,
        auction_id: AuctionId,
        command_id: CommandId,
        plan: &SettlementPlan,
        proxy_closure_plans: Vec<CompetitionPlan>,
        recovery_policy: RecoveryPolicy,
        occurred_at: DateTime<Utc>,
    ) -> Result<(), SettlementTransactionError> {
        recovery_policy.validate().map_err(|_| invalid("恢复策略快照校验失败"))?;
        for line in &plan.lines {
            line.validate().map_err(|_| invalid("结算明细校验失败"))?;
        }
        for obligation in &plan.obligations {
            obligation.validate().map_err(|_| invalid("结算义务校验失败"))?;
        }
        validate_proxy_closure_plans(command_id, plan, &proxy_closure_plans)?;

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
        let closing_pool_version = if plan.compensation_pool_enabled {
            current_pool_version.checked_add(1).ok_or_else(|| invalid("结算池状态版本溢出"))?
        } else {
            current_pool_version
        };

        // 代理终态、lot 成交/流拍状态与金额事实必须在同一 Serializable 事务内提交。
        for closure_plan in proxy_closure_plans {
            self.apply_plan_inner(&closure_plan, None, occurred_at)
                .await
                .map_err(map_competition_error)?;
        }
        close_settlement_lots(self, auction_id, plan, occurred_at).await?;

        {
            let transaction = self.transaction_mut().map_err(map_competition_error)?;
            if plan.compensation_pool_enabled {
                let result = sqlx::query(
                    r#"UPDATE public.settlement_pools
                       SET balance = $2, state_version = $3, updated_at = $6
                       WHERE world_id = $1 AND balance = $4 AND state_version = $5"#,
                )
                .bind(&world_id)
                .bind(plan.closing_pool.value())
                .bind(i64_version(closing_pool_version, "结算池状态版本")?)
                .bind(plan.opening_pool.value())
                .bind(i64_version(current_pool_version, "结算池状态版本")?)
                .bind(occurred_at)
                .execute(&mut **transaction)
                .await
                .map_err(map_sql_error)?;
                if result.rows_affected() != 1 {
                    return Err(invalid("结算池版本与数据库不一致"));
                }
            }

            sqlx::query(
                r#"INSERT INTO public.auction_settlements
                       (auction_id, command_id, policy_version, policy_mode, pool_enabled,
                        opening_pool_balance, closing_pool_balance, tax_total, discount_demand,
                        compensation_total, opening_pool_version, closing_pool_version, settled_at)
                   VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)"#,
            )
            .bind(auction_id.as_uuid())
            .bind(command_id.as_uuid())
            .bind(i32_version(plan.policy_version, "结算策略版本")?)
            .bind(plan.policy_mode.as_str())
            .bind(plan.compensation_pool_enabled)
            .bind(plan.opening_pool.value())
            .bind(plan.closing_pool.value())
            .bind(plan.tax_total.value())
            .bind(plan.discount_demand.value())
            .bind(plan.compensation_total.value())
            .bind(i64_version(current_pool_version, "结算池状态版本")?)
            .bind(i64_version(closing_pool_version, "结算池状态版本")?)
            .bind(occurred_at)
            .execute(&mut **transaction)
            .await
            .map_err(map_sql_error)?;
        }

        insert_lot_settlements(self, plan).await?;
        insert_compensation_ledger(self, command_id, plan, occurred_at).await?;
        insert_obligations(self, plan, occurred_at).await?;
        insert_recoveries(self, auction_id, plan, recovery_policy, occurred_at).await?;
        insert_settlement_audit(
            self,
            auction_id,
            command_id,
            current_pool_version,
            closing_pool_version,
            plan,
            occurred_at,
        )
        .await?;
        let next_version =
            auction_state_version.checked_add(1).ok_or_else(|| invalid("拍卖状态版本溢出"))?;

        {
            let transaction = self.transaction_mut().map_err(map_competition_error)?;
            let result = sqlx::query(
                r#"UPDATE public.auctions
                   SET status = 'settled', settled_at = $4,
                       settlement_complete_command_id = $5,
                       settlement_next_attempt_at = NULL,
                       settlement_last_error_code = NULL,
                       state_version = $2, updated_at = $4
                   WHERE id = $1 AND status = 'settling' AND state_version = $3"#,
            )
            .bind(auction_id.as_uuid())
            .bind(i64_version(next_version, "拍卖状态版本")?)
            .bind(i64_version(auction_state_version, "拍卖状态版本")?)
            .bind(occurred_at)
            .bind(command_id.as_uuid())
            .execute(&mut **transaction)
            .await
            .map_err(map_sql_error)?;
            if result.rows_affected() != 1 {
                return Err(invalid("拍卖状态版本与数据库不一致"));
            }
        }

        insert_settlement_realtime(self, auction_id, command_id, next_version, plan, occurred_at)
            .await
    }
}

pub(super) fn validate_proxy_closure_plans(
    command_id: CommandId,
    settlement_plan: &SettlementPlan,
    closure_plans: &[CompetitionPlan],
) -> Result<(), SettlementTransactionError> {
    if closure_plans.is_empty() {
        return Ok(());
    }
    let lot_ids: HashSet<_> = settlement_plan.lines.iter().map(|line| line.lot_id).collect();
    let mut closed_lots = HashSet::with_capacity(closure_plans.len());
    for plan in closure_plans {
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
            || !lot_ids.contains(&plan.final_lot.lot_id)
            || !closed_lots.insert(plan.final_lot.lot_id)
        {
            return Err(invalid("结算代理关闭计划与结算命令或 lot 不一致"));
        }
    }
    Ok(())
}
