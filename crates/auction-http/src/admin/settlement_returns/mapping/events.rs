//! 映射结算退件的追加事件，并验证每种事件的闭集细节。

use auction_application::settlement::SettlementReturnEvent;
use auction_contracts::{AdminSettlementReturnEventKind, AdminSettlementReturnEventResponse};
use auction_domain::valid_text;

use super::validation::valid_event_details;
use crate::mapping::timestamp;

pub(in crate::admin::settlement_returns) fn map_event(
    event: SettlementReturnEvent,
) -> Option<AdminSettlementReturnEventResponse> {
    let kind = event_kind(&event.event_kind)?;
    if !valid_event_details(kind, &event.details)
        || event.command_id.is_some() != event.actor_id.is_some()
        || event.actor_id.as_deref().is_some_and(|actor| !valid_text(actor, 128))
        || event.command_id.is_some_and(|command| command.as_uuid().is_nil())
    {
        return None;
    }
    let command_event = matches!(
        kind,
        AdminSettlementReturnEventKind::Acknowledged
            | AdminSettlementReturnEventKind::RecheckRequested
            | AdminSettlementReturnEventKind::TakeRetryRequested
            | AdminSettlementReturnEventKind::ResendAuthorized
            | AdminSettlementReturnEventKind::InboxClosed
    ) || (kind == AdminSettlementReturnEventKind::TakeRequested
        && event.details.get("plan").is_some());
    if command_event != event.command_id.is_some() {
        return None;
    }
    Some(AdminSettlementReturnEventResponse {
        case_version: event.case_version,
        event_kind: kind,
        occurred_at: timestamp(event.occurred_at),
        actor_id: event.actor_id,
        command_id: event.command_id.map(|command_id| command_id.as_uuid().to_string()),
        details: event.details,
    })
}

fn event_kind(value: &str) -> Option<AdminSettlementReturnEventKind> {
    use AdminSettlementReturnEventKind as Kind;
    Some(match value {
        "opened" => Kind::Opened,
        "acknowledged" => Kind::Acknowledged,
        "recheck_requested" => Kind::RecheckRequested,
        "recheck_rebound" => Kind::RecheckRebound,
        "recheck_unchanged" => Kind::RecheckUnchanged,
        "take_requested" => Kind::TakeRequested,
        "take_activated" => Kind::TakeActivated,
        "take_proven" => Kind::TakeProven,
        "take_unknown" => Kind::TakeUnknown,
        "take_observed" => Kind::TakeObserved,
        "take_retry_requested" => Kind::TakeRetryRequested,
        "held" => Kind::Held,
        "resend_authorized" => Kind::ResendAuthorized,
        "resent" => Kind::Resent,
        "inbox_closed" => Kind::InboxClosed,
        _ => return None,
    })
}
