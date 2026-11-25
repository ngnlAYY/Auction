//! 接管同事务的返回事实提交：只对成交合同的未付腿，从锁内持久事实推导证据结论，再交 `submit_return_fact`。
//!
//! 保护不变量 3、6（D9 §2.2 / §5.1：“直接接管时已够证即随接管同提，先待判的随首次合格裁决同提”）：
//! - 接管写端 `decide_acquire_return` 已在锁内核过可信 RTS、原轮来源与冻结清单一致、K 的终局唯一，
//!   所以资产一致、归属已证、非重复三项结论取“成立”；
//! - 付款或相反交付以 Sale 已资金化（`consignment_funded_sales`）为准，与 7A-2 处置矩阵同一读口；
//! - isekai 的交割关闭界点取合同冻结季窗的季末（库约束要求必填）；“可信 RTS 发生时原合同仍可交付”没有可读的
//!   持久事实：取“有争议”，事实只待判、不凑数、不处罚，留给之后的补证以同一入口重裁（合同未写清，取最保守值）。

use auction_domain::SettlementWorld;
use auction_domain::consignment::parse_settlement_world;
use auction_domain::consignment::refusal::{Deliverability, RefusalKind};
use auction_time::Timestamp;
use sqlx::PgConnection;
use uuid::Uuid;

use super::{ReturnEvidence, ReturnFactSubmission, SanctionRecord, submit_return_fact};
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::deposit::primitives::database_now;

/// 本次提交新固定的处罚（重放或未达阈值时没有）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OpenedSanction {
    /// 被处罚的买家。
    pub(crate) buyer_user_id: Uuid,
    /// 处罚标识。
    pub(crate) record: SanctionRecord,
}

/// 接管行刚写入后、同一事务内提交返回事实；不是成交合同的未付腿时什么也不写。
///
/// 调用方必须已持有买家用户锁（`submit_return_fact` 失败关闭校验）。
pub(crate) async fn submit_acquired_return_fact(
    conn: &mut PgConnection,
    intake_id: Uuid,
) -> Result<Option<OpenedSanction>, WriteError> {
    type IntakeRow = (String, String, String, Option<Uuid>, bool, Option<Timestamp>);
    let row: Option<IntakeRow> = sqlx::query_as(
        "SELECT contract.kind, contract.world_id, intake.leg, sale.buyer_user_id,
                EXISTS (SELECT 1 FROM public.consignment_funded_sales AS funded
                        WHERE funded.contract_id = intake.contract_id),
                season.end_utc
         FROM public.consignment_return_intakes AS intake
         JOIN public.consignment_contracts AS contract ON contract.contract_id = intake.contract_id
         LEFT JOIN public.consignment_sales AS sale ON sale.contract_id = intake.contract_id
         LEFT JOIN public.consignment_season_windows AS season
           ON season.id = contract.season_window_id
         WHERE intake.id = $1",
    )
    .bind(intake_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((contract_kind, world, leg, buyer, funded, season_end)) = row else {
        return Err(WriteError::invariant("consignment_refusal_intake_missing"));
    };
    // 提款合同没有买家，已付交接与平台现金腿不是买家拒收。
    if contract_kind != "sale" {
        return Ok(None);
    }
    let world = parse_settlement_world(&world)
        .ok_or_else(|| WriteError::invariant("consignment_refusal_world"))?;
    let kind = match (world, leg.as_str()) {
        (SettlementWorld::Main, "original") => RefusalKind::MainCodReturn,
        (SettlementWorld::Isekai, "carrier" | "cash_principal") => {
            RefusalKind::IsekaiUnpaidCarrierReturn
        }
        _ => return Ok(None),
    };
    let buyer = buyer.ok_or_else(|| WriteError::invariant("consignment_refusal_sale_buyer"))?;
    // isekai 的有效交割关闭界点是合同冻结季窗的季末（决定 21、26；`c6_fact_close_shape_ck` 要求必填）。
    // 只取冻结值、不以接管时刻代替；isekai 合同缺季窗是持久事实损坏，失败关闭（不变量 6）。
    // 可交付性仍是待判：接管时没有可读的交付结论，资格不因填入界点而改变。
    let delivery_close_at = match kind {
        RefusalKind::MainCodReturn => None,
        RefusalKind::IsekaiUnpaidCarrierReturn => Some(
            season_end.ok_or_else(|| WriteError::invariant("consignment_refusal_isekai_close"))?,
        ),
    };
    let submission = ReturnFactSubmission {
        intake_id,
        kind,
        proven_at: database_now(conn).await?,
        delivery_close_at,
        evidence: ReturnEvidence {
            deliverability: match kind {
                RefusalKind::MainCodReturn => Deliverability::Deliverable,
                RefusalKind::IsekaiUnpaidCarrierReturn => Deliverability::Disputed,
            },
            paid_or_opposite: funded,
            asset_matches: true,
            duplicate: false,
            ownership_proven: true,
        },
    };
    let outcome = submit_return_fact(conn, &submission).await?;
    Ok(outcome
        .sanction
        .filter(|_| !outcome.replayed)
        .map(|record| OpenedSanction { buyer_user_id: buyer, record }))
}
