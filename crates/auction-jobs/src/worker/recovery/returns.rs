//! Recovery return evidence 阶段：先由恢复流程消费系统来源的付款/交付退件，再由结算退件消费方
//! 收口其余退件（货款、退款、流拍/撤拍寄回与来源无法证明的历史退件）。两边分区互斥，后者只
//! 留证与转人工对账，不触发任何外部动作。

use auction_application::core::Clock;
use auction_observability::WorkerResult;

use super::phase::run_phase;
use crate::worker::runtime::PersistentWorker;

pub(super) async fn record_returns<C>(worker: &PersistentWorker<C>, batch_result: &mut WorkerResult)
where
    C: Clock + 'static,
{
    let Some(store) = &worker.recovery_store else {
        return;
    };
    run_phase(
        worker,
        batch_result,
        ("record_return", "record_return_timeout"),
        |now| store.record_due_return(now),
        |recorded| {
            tracing::info!(
                recovery_id = %recorded.recovery_id,
                return_mail_id = %recorded.return_mail_id,
                stage = %recorded.stage,
                blocking_code = ?recorded.blocking_code,
                action_kinds = ?recorded.action_kinds,
                state_version = recorded.state_version,
                "recovery return evidence recorded"
            );
        },
    )
    .await;
    run_phase(
        worker,
        batch_result,
        ("consume_settlement_return", "consume_settlement_return_timeout"),
        |now| store.consume_due_settlement_return(now),
        |consumed| {
            tracing::info!(
                return_mail_id = %consumed.return_mail_id,
                obligation_id = ?consumed.obligation_id,
                delivery_id = ?consumed.delivery_id,
                match_state = consumed.decision.matched.as_str(),
                reason_code = consumed.decision.reason.as_str(),
                "结算退件已留证并消费"
            );
        },
    )
    .await;
}
