//! Public 业务请求的指标记录与竞价错误结果分类。

use std::time::Instant;

use auction_application::core::{
    ApplicationError, CommandLedgerError, CompetitionTransactionError,
};
use auction_domain::DomainError;
use auction_observability::{BusinessEvent, BusinessOutcome};

use crate::state::PublicApiState;

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
        // 只把映射为客户端规则/状态拒绝的域错误计入拒绝；状态损坏与内部失败保留为 Error。
        ApplicationError::Domain(
            DomainError::InvalidAmount { .. }
            | DomainError::InvalidRules(_)
            | DomainError::InvalidSchedule(_)
            | DomainError::BidTooLow { .. }
            | DomainError::AuctionClosed
            | DomainError::AlreadyLeading
            | DomainError::ActiveProxyExists
            | DomainError::InvalidProxyTransition { .. }
            | DomainError::InvalidProxyMaximum
            | DomainError::ProxyAuthorizationNotActive
            | DomainError::ProxyMaximumNotLower
            | DomainError::ProxyMaximumBelowBinding
            | DomainError::ProxyReentryMaximumNotHigher
            | DomainError::ProxyDisabled
            | DomainError::ProxyOutbidLimit
            | DomainError::ProxySlowFinal,
        )
        | ApplicationError::CommandLedger(CommandLedgerError::Conflict)
        | ApplicationError::CompetitionTransaction(
            CompetitionTransactionError::NotFound
            | CompetitionTransactionError::Conflict
            | CompetitionTransactionError::StaleState
            | CompetitionTransactionError::InvalidState
            | CompetitionTransactionError::BidderIneligible
            | CompetitionTransactionError::CommandLedger(CommandLedgerError::Conflict),
        ) => BusinessOutcome::Rejected,
        _ => BusinessOutcome::Error,
    }
}
