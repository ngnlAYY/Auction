//! 追加式资格裁决与提交入口：保存事实、裁决、合格时写拒收并判阈值，全部在同一事务内。
//!
//! 保护不变量 3、6、8：裁决只追加（待判可随新观察重裁，合格与不计为终局）；合格才写拒收；
//! 任一步失败整笔回滚，不留“有事实无处罚”的半态；矛盾输入（种类与合同世界不符）失败关闭。

use auction_domain::consignment::refusal::{
    NotCounted, PendingReason, Qualification, QualifiedRefusal, ReturnFactInput, qualify_return,
};
use auction_time::Timestamp;
use sqlx::PgConnection;
use uuid::Uuid;

use super::buyer_lock::ensure_buyer_lock_held;
use super::cycle::cycle_for_occurrence;
use super::fact::{
    IntakeContext, StoredFact, fact_id_of, insert_fact, kind_text, load_intake_context,
    load_stored_fact,
};
use super::sanction::{Scope, decide, persist, sanction_of_fact};
use super::{RefusalOutcome, ReturnFactSubmission};
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::deposit::primitives::database_now;

/// 提交一条退回事实或对其重新裁决（买家锁与 Serializable 事务由调用方持有）。
///
/// - 新事实：归属发生时周期并保存，追加首个裁决；合格则写拒收并判阈值，达标写处罚；
/// - 已有终局裁决（合格或不计）：重放，不追加任何行，合格时带回已有处罚；
/// - 已有待判：以本次证据重裁，裁决变化才追加；转为合格则按上面处理。
///
/// 阈值判定时刻 `t0` 是锁后的数据库时刻，不取 Take、补证或 Worker 启动时间（D9 §5.2）。
pub(crate) async fn submit_return_fact(
    conn: &mut PgConnection,
    submission: &ReturnFactSubmission,
) -> Result<RefusalOutcome, WriteError> {
    let context = load_intake_context(conn, submission.intake_id).await?;
    if submission.kind != context.world_kind {
        return Err(WriteError::invariant("consignment_refusal_kind_world"));
    }
    ensure_buyer_lock_held(conn, context.buyer).await?;
    let now = database_now(conn).await?;

    let stored = load_stored_fact(conn, &context, submission.intake_id, submission.kind).await?;
    let fact = match stored {
        Some(fact) => fact,
        None => {
            let cycle = cycle_for_occurrence(conn, context.buyer, context.returned_at, now).await?;
            insert_fact(
                conn,
                &context,
                submission.intake_id,
                submission.kind,
                cycle.cycle_id,
                submission.proven_at,
                submission.delivery_close_at,
            )
            .await?
        }
    };

    let history = verdict_history(conn, fact.return_fact_id).await?;
    if let Some(terminal) = history.iter().copied().find(|verdict| !is_pending(*verdict)) {
        let sanction = match terminal {
            Qualification::Qualified => sanction_of_fact(conn, fact.return_fact_id).await?,
            Qualification::NotCounted(_) | Qualification::Pending(_) => None,
        };
        // 重放返回原结果（不变量 1）：迟到证据首次得 cycle_closed，重放仍得 cycle_closed。
        let cycle_closed = matches!(terminal, Qualification::Qualified)
            && refusal_recorded_after_close(conn, fact.return_fact_id).await?;
        return Ok(RefusalOutcome {
            return_fact_id: fact.return_fact_id,
            qualification: terminal,
            replayed: true,
            cycle_closed,
            sanction,
        });
    }

    let input = fact_input(&context, &fact, submission);
    let qualification = qualify_return(&input);
    let unchanged = history.first().is_some_and(|latest| *latest == qualification);
    if unchanged {
        return Ok(RefusalOutcome {
            return_fact_id: fact.return_fact_id,
            qualification,
            replayed: true,
            cycle_closed: false,
            sanction: None,
        });
    }
    let verdict_id = Uuid::new_v4();
    append_verdict(conn, verdict_id, fact.return_fact_id, qualification, now).await?;
    let Qualification::Qualified = qualification else {
        return Ok(RefusalOutcome {
            return_fact_id: fact.return_fact_id,
            qualification,
            replayed: false,
            cycle_closed: false,
            sanction: None,
        });
    };

    insert_refusal(conn, verdict_id, &context, &fact, submission, now).await?;
    // 周期已被解禁关闭：迟到的合格证据只留记录，不凑数、不处罚（决定 23）。
    if cycle_is_closed(conn, fact.cycle_id).await? {
        return Ok(RefusalOutcome {
            return_fact_id: fact.return_fact_id,
            qualification,
            replayed: false,
            cycle_closed: true,
            sanction: None,
        });
    }
    let trigger = QualifiedRefusal::from_fact(&input, false)
        .ok_or_else(|| WriteError::invariant("consignment_refusal_trigger_unqualified"))?;
    let scope = Scope {
        buyer: context.buyer,
        cycle_id: fact.cycle_id,
        kind: submission.kind,
        trigger_fact_id: fact.return_fact_id,
        threshold: context.threshold,
    };
    let decision = decide(conn, &trigger, &scope, now).await?;
    let sanction = match decision {
        Some(decision) => {
            Some(persist(conn, context.buyer, fact.cycle_id, fact.return_fact_id, &decision).await?)
        }
        None => None,
    };
    Ok(RefusalOutcome {
        return_fact_id: fact.return_fact_id,
        qualification,
        replayed: false,
        cycle_closed: false,
        sanction,
    })
}

const fn is_pending(verdict: Qualification) -> bool {
    matches!(verdict, Qualification::Pending(_))
}

/// 本次观察对应的领域输入：事实身份来自存储，证据结论来自调用方。
fn fact_input(
    context: &IntakeContext,
    fact: &StoredFact,
    submission: &ReturnFactSubmission,
) -> ReturnFactInput {
    ReturnFactInput {
        return_fact_id: fact_id_of(context),
        k: context.key.clone(),
        contract_id: context.contract_id,
        obligation_id: context.obligation_id,
        original_round: context.original_round,
        kind: submission.kind,
        buyer_user_id: context.buyer,
        returned_at: fact.returned_at,
        returned_minute_end: fact.returned_minute_end,
        proven_at: fact.proven_at,
        delivery_close_at: fact.delivery_close_at,
        cycle_id: fact.cycle_id,
        deliverability: submission.evidence.deliverability,
        paid_or_opposite: submission.evidence.paid_or_opposite,
        asset_matches: submission.evidence.asset_matches,
        duplicate: submission.evidence.duplicate,
        ownership_proven: submission.evidence.ownership_proven,
    }
}

/// 事实的全部裁决，最新的在前。
async fn verdict_history(
    conn: &mut PgConnection,
    return_fact_id: Uuid,
) -> Result<Vec<Qualification>, WriteError> {
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT verdict, reason FROM public.consignment_return_fact_verdicts
         WHERE return_fact_id = $1 ORDER BY evaluated_at DESC, id DESC",
    )
    .bind(return_fact_id)
    .fetch_all(&mut *conn)
    .await?;
    rows.iter().map(|(verdict, reason)| parse_verdict(verdict, reason.as_deref())).collect()
}

async fn append_verdict(
    conn: &mut PgConnection,
    verdict_id: Uuid,
    return_fact_id: Uuid,
    qualification: Qualification,
    evaluated_at: Timestamp,
) -> Result<(), WriteError> {
    let (verdict, reason) = verdict_columns(qualification);
    sqlx::query(
        "INSERT INTO public.consignment_return_fact_verdicts
            (id, return_fact_id, verdict, reason, evaluated_at)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(verdict_id)
    .bind(return_fact_id)
    .bind(verdict)
    .bind(reason)
    .bind(evaluated_at)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// 合格裁决生效即写拒收；拒收 ID 复用该裁决 ID，使裁决与拒收一一可查。
async fn insert_refusal(
    conn: &mut PgConnection,
    verdict_id: Uuid,
    context: &IntakeContext,
    fact: &StoredFact,
    submission: &ReturnFactSubmission,
    qualified_at: Timestamp,
) -> Result<(), WriteError> {
    sqlx::query(
        "INSERT INTO public.consignment_refusals
            (id, return_fact_id, verdict_id, buyer_user_id, cycle_id, contract_id, kind, qualified_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(verdict_id)
    .bind(fact.return_fact_id)
    .bind(verdict_id)
    .bind(context.buyer.as_uuid())
    .bind(fact.cycle_id)
    .bind(context.contract_id)
    .bind(kind_text(submission.kind))
    .bind(qualified_at)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// 该事实的拒收是否在其周期关闭之后才记录（迟到证据）；买家锁内的数据库墙钟先后即写入先后。
async fn refusal_recorded_after_close(
    conn: &mut PgConnection,
    return_fact_id: Uuid,
) -> Result<bool, WriteError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM public.consignment_refusals AS refusal
             JOIN public.consignment_refusal_cycles AS cycle ON cycle.id = refusal.cycle_id
             WHERE refusal.return_fact_id = $1 AND cycle.state = 'closed'
               AND refusal.qualified_at >= cycle.closed_at)",
    )
    .bind(return_fact_id)
    .fetch_one(&mut *conn)
    .await?)
}

async fn cycle_is_closed(conn: &mut PgConnection, cycle_id: Uuid) -> Result<bool, WriteError> {
    let state: String =
        sqlx::query_scalar("SELECT state FROM public.consignment_refusal_cycles WHERE id = $1")
            .bind(cycle_id)
            .fetch_one(&mut *conn)
            .await?;
    Ok(state == "closed")
}

fn verdict_columns(qualification: Qualification) -> (&'static str, Option<&'static str>) {
    match qualification {
        Qualification::Qualified => ("qualified", None),
        Qualification::NotCounted(reason) => (
            "not_counted",
            Some(match reason {
                NotCounted::AfterDeliveryClosed => "after_delivery_closed",
                NotCounted::PaidOrOpposite => "paid_or_opposite",
                NotCounted::WrongAsset => "wrong_asset",
                NotCounted::Duplicate => "duplicate",
            }),
        ),
        Qualification::Pending(reason) => (
            "pending",
            Some(match reason {
                PendingReason::BoundaryMinuteAmbiguous => "boundary_minute_ambiguous",
                PendingReason::DeliverabilityDisputed => "deliverability_disputed",
                PendingReason::OwnershipUnproven => "ownership_unproven",
            }),
        ),
    }
}

/// 把持久的 (verdict, reason) 还原为领域裁决；未知值说明数据损坏，失败关闭。
fn parse_verdict(verdict: &str, reason: Option<&str>) -> Result<Qualification, WriteError> {
    let corrupt = || WriteError::invariant("consignment_refusal_verdict_shape");
    match (verdict, reason) {
        ("qualified", None) => Ok(Qualification::Qualified),
        ("not_counted", Some("after_delivery_closed")) => {
            Ok(Qualification::NotCounted(NotCounted::AfterDeliveryClosed))
        }
        ("not_counted", Some("paid_or_opposite")) => {
            Ok(Qualification::NotCounted(NotCounted::PaidOrOpposite))
        }
        ("not_counted", Some("wrong_asset")) => {
            Ok(Qualification::NotCounted(NotCounted::WrongAsset))
        }
        ("not_counted", Some("duplicate")) => Ok(Qualification::NotCounted(NotCounted::Duplicate)),
        ("pending", Some("boundary_minute_ambiguous")) => {
            Ok(Qualification::Pending(PendingReason::BoundaryMinuteAmbiguous))
        }
        ("pending", Some("deliverability_disputed")) => {
            Ok(Qualification::Pending(PendingReason::DeliverabilityDisputed))
        }
        ("pending", Some("ownership_unproven")) => {
            Ok(Qualification::Pending(PendingReason::OwnershipUnproven))
        }
        _ => Err(corrupt()),
    }
}
