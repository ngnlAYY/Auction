//! 统一处置事实的领域规则回归：闭集、来源唯一性、解除失败计数口径与确认准入。

use std::collections::BTreeSet;

use auction_domain::{
    DeliveryAcknowledgementFacts, DeliveryDispositionActorSource, DeliveryDispositionError,
    DeliveryDispositionKind, DeliveryDispositionSource, PREFLIGHT_LOT_WATCH_INVALIDATED,
    PREFLIGHT_RETIREMENT_CODES, validate_acknowledgement, validate_disposition_actor,
    validate_disposition_reason, validate_disposition_reason_code,
};
use uuid::Uuid;

fn source_with_admin() -> DeliveryDispositionSource {
    DeliveryDispositionSource { admin_command_id: Some(Uuid::from_u128(11)), ..Default::default() }
}

fn facts() -> DeliveryAcknowledgementFacts<'static> {
    DeliveryAcknowledgementFacts {
        status: "failed",
        origin_kind: "lot_watch",
        credits: Some(0),
        cod: Some(0),
        equipment_count: 0,
        has_fulfillment_obligation: false,
    }
}

#[test]
fn kind_names_are_unique_and_round_trip() {
    let mut seen = BTreeSet::new();
    for kind in DeliveryDispositionKind::ALL {
        let name = kind.as_str();
        assert!(seen.insert(name), "种类名称必须唯一: {name}");
        assert_eq!(DeliveryDispositionKind::parse(name), Ok(kind));
        // 只接受闭集内的稳定名称，大小写与未知取值都失败关闭。
        assert_eq!(
            DeliveryDispositionKind::parse(&name.to_uppercase()),
            Err(DeliveryDispositionError::UnknownKind)
        );
    }
    assert_eq!(
        DeliveryDispositionKind::parse("operator_acknowledgement"),
        Err(DeliveryDispositionError::UnknownKind)
    );
    assert_eq!(seen.len(), 7);
}

#[test]
fn actor_sources_are_closed() {
    let mut seen = BTreeSet::new();
    for source in DeliveryDispositionActorSource::ALL {
        assert!(seen.insert(source.as_str()));
        assert_eq!(DeliveryDispositionActorSource::parse(source.as_str()), Ok(source));
    }
    assert_eq!(seen.len(), 4);
    assert_eq!(
        DeliveryDispositionActorSource::parse("operator"),
        Err(DeliveryDispositionError::InvalidActor)
    );
}

#[test]
fn only_lot_watch_invalidated_preflight_retirement_resolves_failures() {
    assert!(
        DeliveryDispositionKind::PreflightRetired
            .resolves_alert(Some(PREFLIGHT_LOT_WATCH_INVALIDATED))
    );
    for code in ["recipient_renamed", "auction_notice_invalidated", "season_notice_invalidated"] {
        assert!(
            !DeliveryDispositionKind::PreflightRetired.resolves_alert(Some(code)),
            "改名与赛季退役当前仍计入通知失败: {code}"
        );
    }
    // 缺少失败码的预检退役不解除计数，避免把未知终态当作已处置。
    assert!(!DeliveryDispositionKind::PreflightRetired.resolves_alert(None));

    for kind in [
        DeliveryDispositionKind::AutoSentProof,
        DeliveryDispositionKind::OperatorSentConfirmation,
        DeliveryDispositionKind::AdminFailed,
        DeliveryDispositionKind::EquipmentLoss,
        DeliveryDispositionKind::LegacySelf,
        DeliveryDispositionKind::OperatorAcknowledged,
    ] {
        assert!(kind.resolves_alert(None), "该种类必须解除失败计数: {}", kind.as_str());
    }
}

#[test]
fn source_requires_exactly_one_reference_matching_the_kind() {
    assert_eq!(source_with_admin().validate(DeliveryDispositionKind::AdminFailed), Ok(()));

    let mismatched = source_with_admin();
    assert_eq!(
        mismatched.validate(DeliveryDispositionKind::EquipmentLoss),
        Err(DeliveryDispositionError::InvalidSource)
    );

    let empty = DeliveryDispositionSource::default();
    assert_eq!(
        empty.validate(DeliveryDispositionKind::AutoSentProof),
        Err(DeliveryDispositionError::InvalidSource)
    );

    let two = DeliveryDispositionSource {
        admin_command_id: Some(Uuid::from_u128(11)),
        audit_id: Some(Uuid::from_u128(12)),
        ..Default::default()
    };
    assert_eq!(
        two.validate(DeliveryDispositionKind::AdminFailed),
        Err(DeliveryDispositionError::InvalidSource)
    );

    // 预检退役以投递行自身为证：任何外部引用都拒绝。
    assert_eq!(empty.validate(DeliveryDispositionKind::PreflightRetired), Ok(()));
    assert_eq!(
        source_with_admin().validate(DeliveryDispositionKind::PreflightRetired),
        Err(DeliveryDispositionError::InvalidSource)
    );
}

#[test]
fn sent_binding_requires_known_world_and_positive_mid() {
    let binding = |world: &str, mid: &str| DeliveryDispositionSource {
        sent_binding: Some((world.to_owned(), mid.to_owned())),
        ..Default::default()
    };
    let kind = DeliveryDispositionKind::AutoSentProof;
    assert_eq!(binding("main", "3125345").validate(kind), Ok(()));
    assert_eq!(binding("isekai", "7").validate(kind), Ok(()));
    assert_eq!(binding("third", "7").validate(kind), Err(DeliveryDispositionError::InvalidSource));
    assert_eq!(binding("main", "0").validate(kind), Err(DeliveryDispositionError::InvalidSource));
    assert_eq!(
        binding("main", "03125345").validate(kind),
        Err(DeliveryDispositionError::InvalidSource)
    );
    assert_eq!(binding("main", "").validate(kind), Err(DeliveryDispositionError::InvalidSource));
    assert_eq!(binding("main", "abc").validate(kind), Err(DeliveryDispositionError::InvalidSource));
}

#[test]
fn preflight_reason_code_is_a_closed_set() {
    for code in PREFLIGHT_RETIREMENT_CODES {
        assert_eq!(
            validate_disposition_reason_code(DeliveryDispositionKind::PreflightRetired, Some(code)),
            Ok(())
        );
    }
    assert_eq!(
        validate_disposition_reason_code(DeliveryDispositionKind::PreflightRetired, None),
        Err(DeliveryDispositionError::InvalidReason)
    );
    assert_eq!(
        validate_disposition_reason_code(
            DeliveryDispositionKind::PreflightRetired,
            Some("mail_disposition_conflict")
        ),
        Err(DeliveryDispositionError::InvalidReason)
    );
    // 其它种类的原因码只做规范文本校验。
    assert_eq!(
        validate_disposition_reason_code(
            DeliveryDispositionKind::EquipmentLoss,
            Some("equipment_destroyed")
        ),
        Ok(())
    );
    assert_eq!(
        validate_disposition_reason_code(DeliveryDispositionKind::EquipmentLoss, None),
        Ok(())
    );
    assert_eq!(
        validate_disposition_reason_code(
            DeliveryDispositionKind::AdminFailed,
            Some("contains control\u{7}")
        ),
        Err(DeliveryDispositionError::InvalidReason)
    );
}

#[test]
fn operator_reason_is_required_and_other_reasons_are_optional() {
    let acknowledged = DeliveryDispositionKind::OperatorAcknowledged;
    assert_eq!(
        validate_disposition_reason(acknowledged, None),
        Err(DeliveryDispositionError::InvalidReason)
    );
    assert_eq!(
        validate_disposition_reason(acknowledged, Some("  ")),
        Err(DeliveryDispositionError::InvalidReason)
    );
    assert_eq!(validate_disposition_reason(acknowledged, Some("已核对，无资金影响")), Ok(()));
    assert_eq!(validate_disposition_reason(DeliveryDispositionKind::AutoSentProof, None), Ok(()));
    let long = "a".repeat(2049);
    assert_eq!(
        validate_disposition_reason(acknowledged, Some(&long)),
        Err(DeliveryDispositionError::InvalidReason)
    );
}

#[test]
fn actor_identifier_is_bounded_and_normalized() {
    assert_eq!(validate_disposition_actor("operator"), Ok(()));
    assert_eq!(
        validate_disposition_actor(" operator"),
        Err(DeliveryDispositionError::InvalidActor)
    );
    assert_eq!(validate_disposition_actor(""), Err(DeliveryDispositionError::InvalidActor));
    assert_eq!(
        validate_disposition_actor("bad\u{1}name"),
        Err(DeliveryDispositionError::InvalidActor)
    );
    let long = "a".repeat(129);
    assert_eq!(validate_disposition_actor(&long), Err(DeliveryDispositionError::InvalidActor));
}

#[test]
fn acknowledgement_accepts_only_history_without_fund_or_asset_impact() {
    assert_eq!(validate_acknowledgement(facts()), Ok(()));

    let cases = [
        DeliveryAcknowledgementFacts { status: "manual_reconcile", ..facts() },
        DeliveryAcknowledgementFacts { status: "succeeded", ..facts() },
        DeliveryAcknowledgementFacts { status: "pending", ..facts() },
        DeliveryAcknowledgementFacts { origin_kind: "settlement", ..facts() },
        DeliveryAcknowledgementFacts { origin_kind: "settlement_recovery_action", ..facts() },
        DeliveryAcknowledgementFacts { has_fulfillment_obligation: true, ..facts() },
        DeliveryAcknowledgementFacts { credits: Some(1), ..facts() },
        DeliveryAcknowledgementFacts { credits: Some(-1), ..facts() },
        DeliveryAcknowledgementFacts { credits: None, ..facts() },
        DeliveryAcknowledgementFacts { cod: Some(500), ..facts() },
        DeliveryAcknowledgementFacts { cod: None, ..facts() },
        DeliveryAcknowledgementFacts { equipment_count: 1, ..facts() },
    ];
    for case in cases {
        assert_eq!(
            validate_acknowledgement(case),
            Err(DeliveryDispositionError::NotAcknowledgeable),
            "必须拒绝仍有资金或资产影响的条目: {case:?}"
        );
    }
}
