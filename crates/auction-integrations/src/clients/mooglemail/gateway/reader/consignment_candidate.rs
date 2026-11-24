//! 收件详情的旧信/寄售候选分流，以及同一详情文档上的 4D1 会话绑定。
//!
//! 门控关闭时本模块不参与任何判定；打开后仍先按列表投影挑选候选，并在详情完整主题上
//! 排除既有拍卖可识别的系统结算退件，避免寄售接管旧 owner 的邮件。

use std::time::Instant;

use auction_application::settlement::{
    InternalEvidenceError, system_returned_settlement_obligation_id,
};
use auction_domain::consignment::is_consign_subject;
use auction_time::Timestamp;

use super::super::MoogleMailCredentialSession;
use super::MoogleMailGateway;
use super::consignment::{
    BoundConsignmentObservation, ConsignmentReadPending, remote_observation_id,
};
use crate::clients::mooglemail::error::{MoogleMailError, MoogleMailErrorCategory};
use crate::clients::mooglemail::parser::{
    ConsignmentListRow, MoogleMailSummary, parse_consignment_detail,
};
use crate::clients::mooglemail::protocol::{MoogleMailOperation, MoogleMailWorld};

/// 详情文档应走的解析路径。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DetailRoute {
    /// 既有通用解析；门控关闭时恒为该值。
    Legacy,
    /// 4D1 候选观察，同时保留通用解析结果供本段透传。
    Consignment,
}

/// 列表投影是否为寄售候选。
pub(crate) fn list_is_candidate(sender: &str, subject: &str) -> bool {
    is_consign_subject(subject)
        || (is_system_rts(sender, subject)
            && system_returned_settlement_obligation_id(sender, subject).is_none())
}

/// 把列表投影转成 4D1 观察所需的原始行；不读取详情。
pub(crate) fn row_from_summary(summary: &MoogleMailSummary) -> ConsignmentListRow {
    ConsignmentListRow {
        external_mail_id: summary.external_mail_id.clone(),
        world: summary.world,
        world_marker: summary.world_marker,
        sender: summary.sender.clone(),
        subject: summary.subject.clone(),
        received_minute_raw: summary.received_minute_raw.clone(),
        received_at: summary.received_at,
    }
}

/// 详情完整主题的最终分流；可识别的旧结算退件始终走旧解析。
pub(crate) fn detail_route(
    sender: &str,
    subject: &str,
    consignment_candidates: bool,
) -> DetailRoute {
    if !consignment_candidates {
        return DetailRoute::Legacy;
    }
    if system_returned_settlement_obligation_id(sender, subject).is_some() {
        return DetailRoute::Legacy;
    }
    if is_consign_subject(subject) || is_system_rts(sender, subject) {
        DetailRoute::Consignment
    } else {
        DetailRoute::Legacy
    }
}

/// 在同一详情文档上建立认证会话绑定；不重复 GET，也不写任何持久事实。
pub(crate) async fn bind_candidate(
    gateway: &MoogleMailGateway,
    session: &MoogleMailCredentialSession,
    world: MoogleMailWorld,
    deadline: tokio::time::Instant,
    row: &ConsignmentListRow,
    document: &[u8],
) -> Result<BoundConsignmentObservation, ConsignmentReadPending> {
    let standard_deadline: Instant = deadline.into_std();
    let binding = gateway
        .read_identity_in_session(session, world, standard_deadline)
        .await
        .map_err(map_identity_error)?;
    let observation = parse_consignment_detail(document, world, row);
    if !observation.sender.is_empty() && !gateway.allowed_sender(&observation.sender) {
        return Err(ConsignmentReadPending::AllowlistDenied);
    }
    session.check_internal_current(standard_deadline).await.map_err(map_identity_error)?;
    let observed_at = Timestamp::now();
    let remote_observation_id = remote_observation_id(&binding, &observation);
    Ok(BoundConsignmentObservation { observation, binding, remote_observation_id, observed_at })
}

/// 把读取 pending 映射为可重试或失败关闭的 gateway 错误；不携带远端原文。
pub(crate) fn pending_error(
    world: MoogleMailWorld,
    reason: ConsignmentReadPending,
) -> MoogleMailError {
    let (category, temporary) = match reason {
        ConsignmentReadPending::RateLimited => (MoogleMailErrorCategory::RateLimited, true),
        ConsignmentReadPending::AccountBusy => (MoogleMailErrorCategory::AccountBusy, true),
        ConsignmentReadPending::AllowlistDenied => (MoogleMailErrorCategory::Configuration, false),
        ConsignmentReadPending::AuthenticatedUidUnavailable => {
            (MoogleMailErrorCategory::IdentityUnproven, false)
        }
        ConsignmentReadPending::TransientReadUnavailable => {
            (MoogleMailErrorCategory::CredentialUnavailable, true)
        }
    };
    MoogleMailError::new(MoogleMailOperation::Detail, world, category, temporary)
}

fn map_identity_error(error: InternalEvidenceError) -> ConsignmentReadPending {
    match error {
        InternalEvidenceError::Unavailable
        | InternalEvidenceError::Expired
        | InternalEvidenceError::Revoked => ConsignmentReadPending::TransientReadUnavailable,
        InternalEvidenceError::Unsupported
        | InternalEvidenceError::Unproven
        | InternalEvidenceError::InvalidRequest => {
            ConsignmentReadPending::AuthenticatedUidUnavailable
        }
    }
}

fn is_system_rts(sender: &str, subject: &str) -> bool {
    sender == "MoogleMail"
        && subject.get(..6).is_some_and(|prefix| prefix.eq_ignore_ascii_case("[RTS] "))
}

#[cfg(test)]
mod tests {
    use auction_time::Timestamp;

    use super::DetailRoute;
    use super::detail_route;
    use super::list_is_candidate;
    use crate::clients::mooglemail::MoogleMailWorld;
    use crate::clients::mooglemail::parser::{ConsignmentListRow, parse_consignment_detail};

    const CONSIGN_DETAIL: &str =
        include_str!("../../../../../tests/fixtures/mooglemail/consign-detail-main-single.html");

    #[test]
    fn disabled_gate_never_marks_a_candidate() {
        let settlement = "[RTS] Auction buyer refund [5b1c2a9e-1f0d-4c6b-9a7e-3d2f1e0c9b8a]";
        for (sender, subject) in [
            ("FixtureSeller", "consign"),
            ("MoogleMail", "[RTS] consign"),
            ("MoogleMail", settlement),
        ] {
            assert_eq!(detail_route(sender, subject, false), DetailRoute::Legacy);
        }
    }

    #[test]
    fn settlement_return_is_never_taken_from_the_legacy_owner() {
        let obligation = "5b1c2a9e-1f0d-4c6b-9a7e-3d2f1e0c9b8a";
        let settlement = format!("[RTS] Auction buyer refund [{obligation}]");
        assert!(!list_is_candidate("MoogleMail", &settlement));
        assert_eq!(detail_route("MoogleMail", &settlement, true), DetailRoute::Legacy);
        assert_eq!(
            detail_route("MoogleMail", "[RTS] Auction payment overdue", true),
            DetailRoute::Consignment
        );
        assert_eq!(detail_route("FixtureSeller", "consign", true), DetailRoute::Consignment);
        assert_eq!(detail_route("FixtureSeller", "hello", true), DetailRoute::Legacy);
        let row = ConsignmentListRow {
            external_mail_id: "900001".to_owned(),
            world: MoogleMailWorld::Main,
            world_marker: Some(MoogleMailWorld::Main),
            sender: "FixtureSeller".to_owned(),
            subject: "consign".to_owned(),
            received_minute_raw: "2026-11-02 03:45".to_owned(),
            received_at: Timestamp::UNIX_EPOCH,
        };
        let observation = parse_consignment_detail(CONSIGN_DETAIL.as_bytes(), row.world, &row);
        assert!(observation.subject_is_consign());
        assert!(!observation.duplicate_raw_eid);
    }
}
