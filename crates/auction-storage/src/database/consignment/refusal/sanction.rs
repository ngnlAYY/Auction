//! 处罚：读取同买家同周期同种类的合格拒收池，按领域规则判阈值，达标时写处罚与构成项。
//!
//! 保护不变量 2、3：判阈值与写处罚在同一买家锁、同一 Serializable 事务内完成，
//! `UNIQUE(buyer_user_id, cycle_id)` 与构成项全局唯一把“同周期至多一项处罚、构成项只用一次”
//! 落到数据库；处罚、恰阈值个构成项与唯一账号工作由延迟触发器在 COMMIT 前整体核对。

use auction_domain::UserId;
use auction_domain::consignment::OriginalMailKey;
use auction_domain::consignment::refusal::{
    Deliverability, QualifiedRefusal, RefusalKind, ReturnFactInput, SanctionDecision, SanctionRule,
    evaluate_cycle_sanction, sanction_id,
};
use auction_time::Timestamp;
use sqlx::PgConnection;
use uuid::Uuid;

use super::SanctionRecord;
use super::account_work::insert_account_work;
use super::fact::kind_text;
use crate::database::consignment::commands::errors::WriteError;

/// 处罚规则对应的拒收种类：main 规则只数 main 事实，isekai 规则只数 isekai 事实。
pub(super) const fn rule_for(kind: RefusalKind) -> SanctionRule {
    match kind {
        RefusalKind::MainCodReturn => SanctionRule::MainCodReturns,
        RefusalKind::IsekaiUnpaidCarrierReturn => SanctionRule::IsekaiUnpaidCarrierReturns,
    }
}

/// 处罚规则的持久文本（表约束值，不同于账号理由码）。
const fn rule_text(rule: SanctionRule) -> &'static str {
    match rule {
        SanctionRule::MainCodReturns => "main_cod_returns",
        SanctionRule::IsekaiUnpaidCarrierReturns => "isekai_unpaid_carrier_returns",
    }
}

fn parse_rule(text: &str) -> Result<SanctionRule, WriteError> {
    match text {
        "main_cod_returns" => Ok(SanctionRule::MainCodReturns),
        "isekai_unpaid_carrier_returns" => Ok(SanctionRule::IsekaiUnpaidCarrierReturns),
        _ => Err(WriteError::invariant("consignment_refusal_sanction_rule")),
    }
}

/// 该买家在该周期是否已有处罚（含已执行与已因周期关闭而作废的）。
pub(super) async fn cycle_has_sanction(
    conn: &mut PgConnection,
    buyer: UserId,
    cycle_id: Uuid,
) -> Result<bool, WriteError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM public.consignment_sanctions WHERE buyer_user_id = $1 AND cycle_id = $2)",
    )
    .bind(buyer.as_uuid())
    .bind(cycle_id)
    .fetch_one(&mut *conn)
    .await?)
}

type PoolRow = (
    Uuid,
    Uuid,
    Uuid,
    i32,
    Timestamp,
    Timestamp,
    Timestamp,
    Option<Timestamp>,
    String,
    String,
    String,
    bool,
);

/// 同买家、同周期、同种类的已合格拒收（不含触发事实自身），连同“是否已被处罚占用”。
///
/// 池成员由已存的合格裁决构造：用保存的事实重走 `from_fact`，若领域不再判其合格就是持久数据
/// 与规则矛盾，失败关闭而不是悄悄少数（不变量 6）。
pub(super) async fn load_pool(
    conn: &mut PgConnection,
    buyer: UserId,
    cycle_id: Uuid,
    kind: RefusalKind,
    trigger_fact_id: Uuid,
) -> Result<Vec<QualifiedRefusal>, WriteError> {
    let rows: Vec<PoolRow> = sqlx::query_as(
        "SELECT fact.return_fact_id, fact.contract_id, fact.obligation_id, fact.original_round,
                fact.returned_at, fact.returned_minute_end, fact.proven_at, fact.delivery_close_at,
                intake.bot_uid, intake.world_id, intake.external_mid,
                EXISTS (SELECT 1 FROM public.consignment_sanction_members AS member
                        WHERE member.return_fact_id = fact.return_fact_id) AS used
         FROM public.consignment_refusals AS refusal
         JOIN public.consignment_return_facts AS fact
           ON fact.return_fact_id = refusal.return_fact_id
         JOIN public.consignment_return_intakes AS intake ON intake.id = fact.intake_id
         WHERE refusal.buyer_user_id = $1 AND refusal.cycle_id = $2 AND refusal.kind = $3
           AND refusal.return_fact_id <> $4",
    )
    .bind(buyer.as_uuid())
    .bind(cycle_id)
    .bind(kind_text(kind))
    .bind(trigger_fact_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut pool = Vec::with_capacity(rows.len());
    for (
        fact_id,
        contract_id,
        obligation_id,
        round,
        returned_at,
        minute_end,
        proven_at,
        close_at,
        bot,
        world,
        mid,
        used,
    ) in rows
    {
        let world = auction_domain::consignment::parse_settlement_world(&world)
            .ok_or_else(|| WriteError::invariant("consignment_refusal_world"))?;
        let input = ReturnFactInput {
            return_fact_id: fact_id,
            k: OriginalMailKey::new(&bot, world, &mid).map_err(|error| {
                tracing::warn!(%error, "拒收池成员的原信键无效");
                WriteError::invariant("consignment_refusal_mail_key")
            })?,
            contract_id,
            obligation_id,
            original_round: u32::try_from(round)?,
            kind,
            buyer_user_id: buyer,
            returned_at,
            returned_minute_end: minute_end,
            proven_at,
            delivery_close_at: close_at,
            cycle_id,
            // 池里只有已存合格裁决的事实：证据结论取其合格时的形状。
            deliverability: Deliverability::Deliverable,
            paid_or_opposite: false,
            asset_matches: true,
            duplicate: false,
            ownership_proven: true,
        };
        let member = QualifiedRefusal::from_fact(&input, used)
            .ok_or_else(|| WriteError::invariant("consignment_refusal_pool_unqualified"))?;
        pool.push(member);
    }
    Ok(pool)
}

/// 一次判阈值的范围：谁、哪个周期、哪种规则，以及触发事实与冻结阈值。
pub(super) struct Scope {
    pub(super) buyer: UserId,
    pub(super) cycle_id: Uuid,
    pub(super) kind: RefusalKind,
    pub(super) trigger_fact_id: Uuid,
    pub(super) threshold: u8,
}

/// 在买家锁内判定本次合格拒收是否使买家达到阈值；周期已有处罚时一律不再判。
pub(super) async fn decide(
    conn: &mut PgConnection,
    trigger: &QualifiedRefusal,
    scope: &Scope,
    t0: Timestamp,
) -> Result<Option<SanctionDecision>, WriteError> {
    // 同周期至多一项处罚：已有（含已执行、已因周期关闭作废）就不再判，构成项照 6D1 规则留在池里。
    let already = cycle_has_sanction(conn, scope.buyer, scope.cycle_id).await?;
    let pool =
        load_pool(conn, scope.buyer, scope.cycle_id, scope.kind, scope.trigger_fact_id).await?;
    Ok(evaluate_cycle_sanction(already, rule_for(scope.kind), trigger, &pool, scope.threshold, t0))
}

/// 写处罚、恰阈值个构成项与唯一账号工作（pending 处罚即新购阻断）。
pub(super) async fn persist(
    conn: &mut PgConnection,
    buyer: UserId,
    cycle_id: Uuid,
    trigger_fact_id: Uuid,
    decision: &SanctionDecision,
) -> Result<SanctionRecord, WriteError> {
    let id = sanction_id(buyer, cycle_id);
    sqlx::query(
        "INSERT INTO public.consignment_sanctions
            (id, buyer_user_id, cycle_id, rule, t0, threshold, window_from, window_to,
             trigger_return_fact_id, state, version)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'pending', 1)",
    )
    .bind(id)
    .bind(buyer.as_uuid())
    .bind(cycle_id)
    .bind(rule_text(decision.rule))
    .bind(decision.t0)
    .bind(i16::from(decision.threshold))
    .bind(decision.window.map(|window| window.from_exclusive))
    .bind(decision.window.map(|window| window.to_inclusive))
    .bind(trigger_fact_id)
    .execute(&mut *conn)
    .await?;
    for member in &decision.members {
        sqlx::query(
            "INSERT INTO public.consignment_sanction_members (sanction_id, return_fact_id)
             VALUES ($1, $2)",
        )
        .bind(id)
        .bind(*member)
        .execute(&mut *conn)
        .await?;
    }
    let account_work_id = insert_account_work(conn, id, buyer).await?;
    Ok(SanctionRecord {
        sanction_id: id,
        cycle_id,
        account_work_id,
        rule: decision.rule,
        t0: decision.t0,
    })
}

/// 事实已是某项处罚的构成项时，返回那项处罚（重放时用）。
pub(super) async fn sanction_of_fact(
    conn: &mut PgConnection,
    return_fact_id: Uuid,
) -> Result<Option<SanctionRecord>, WriteError> {
    let row: Option<(Uuid, Uuid, String, Timestamp, Uuid)> = sqlx::query_as(
        "SELECT sanction.id, sanction.cycle_id, sanction.rule, sanction.t0, work.id
         FROM public.consignment_sanction_members AS member
         JOIN public.consignment_sanctions AS sanction ON sanction.id = member.sanction_id
         JOIN public.consignment_account_work AS work ON work.sanction_id = sanction.id
         WHERE member.return_fact_id = $1",
    )
    .bind(return_fact_id)
    .fetch_optional(&mut *conn)
    .await?;
    row.map(|(sanction_id, cycle_id, rule, t0, account_work_id)| {
        Ok(SanctionRecord { sanction_id, cycle_id, account_work_id, rule: parse_rule(&rule)?, t0 })
    })
    .transpose()
}
