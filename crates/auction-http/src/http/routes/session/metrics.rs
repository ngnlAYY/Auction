//! Public 业务请求的指标记录与竞价错误结果分类。

use super::*;

pub(crate) fn observe_public_business(
    state: &PublicApiState,
    event: BusinessEvent,
    outcome: BusinessOutcome,
    started: Instant,
) {
    let Some(metrics) = state.metrics() else {
        return;
    };
    let recorder = metrics.business();
    recorder.observe(event, outcome);
    recorder.observe_duration(event, started.elapsed());
}

pub(crate) fn business_outcome_for_competition_error(error: &ApplicationError) -> BusinessOutcome {
    match error {
        ApplicationError::Domain(_)
        | ApplicationError::CommandLedger(CommandLedgerError::Conflict)
        | ApplicationError::CompetitionTransaction(
            CompetitionTransactionError::NotFound
            | CompetitionTransactionError::Conflict
            | CompetitionTransactionError::StaleState
            | CompetitionTransactionError::InvalidState
            | CompetitionTransactionError::CommandLedger(CommandLedgerError::Conflict),
        ) => BusinessOutcome::Rejected,
        _ => BusinessOutcome::Error,
    }
}
