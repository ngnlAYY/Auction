//! 结算事务主流程与锁内计划应用；事务边界由父模块统一装配。

use crate::database::PostgresCompetitionTransaction;
use crate::database::settlement::{
    invalid, map_competition_error, map_sql_error, parse_settlement_policy, positive_u64,
    settlement_world,
};
use auction_application::core::{CommandLedgerError, StoredCommand};
use auction_application::settlement::{SettlementSnapshot, SettlementTransactionError};
use auction_domain::{AuctionId, CommandId, SettlementAmount, SettlementFact};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

mod apply;
mod persisted;

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
        let auction_row = {
            let transaction = self.transaction_mut().map_err(map_competition_error)?;
            let query = r#"SELECT
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
                   CROSS JOIN public.compensation_pool AS p
                   WHERE a.id = $1 AND a.status IN ('settling', 'settled')
                   FOR UPDATE OF a, p"#;
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
            let query = r#"SELECT
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
                   FOR UPDATE OF l"#;
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
            let leader_held_nanos = leader_id
                .map(|_| {
                    row.try_get::<Option<i64>, _>("leader_held_nanos")
                        .map_err(map_sql_error)?
                        .ok_or_else(|| invalid("累计领先时长无效"))
                })
                .transpose()?;
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
}
