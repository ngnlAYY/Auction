use super::*;
use chrono::TimeDelta;

fn expectation() -> ExternalRecoveryDeliveryExpectation {
    ExternalRecoveryDeliveryExpectation {
        delivery_id: Uuid::from_u128(7),
        fencing_version: 3,
        evidence: ExternalRecoverySendEvidence::Frozen,
        world_id: "main".to_owned(),
        recipient: "Restia﹒Ashdoll".to_owned(),
        subject: Some(
            "[Auction] settlement payment 11111111-2222-3333-4444-555555555555".to_owned(),
        ),
        credits: 0,
        cod: 0,
        equipment_ids: Some(vec!["317632875".to_owned()]),
    }
}

/// 恢复库只有 payload 时的身份：没有渲染标题，装备身份可能未知。
fn restored_payload_expectation() -> ExternalRecoveryDeliveryExpectation {
    ExternalRecoveryDeliveryExpectation {
        delivery_id: Uuid::from_u128(8),
        fencing_version: 4,
        evidence: ExternalRecoverySendEvidence::RestoredPayload,
        world_id: "main".to_owned(),
        recipient: "Restia﹒Ashdoll".to_owned(),
        subject: None,
        credits: 300_000,
        cod: 0,
        equipment_ids: None,
    }
}

#[test]
fn subject_projection_accepts_truncation_only() {
    let expected = expectation().subject.unwrap();
    assert!(subject_is_list_projection(&expected, &expected));
    assert!(subject_is_list_projection("[Auction] settlement pay", &expected));
    assert!(subject_is_list_projection("[Auction] settlement pay\u{2026}", &expected));
    assert!(!subject_is_list_projection("[Auction] other", &expected));
    assert!(!subject_is_list_projection("", &expected));
}

#[test]
fn list_projection_requires_world_and_cutoff() {
    let expectation = expectation();
    let cutoff = Utc::now() - TimeDelta::hours(1);
    assert!(sent_list_projection_may_match(
        &expectation,
        "main",
        "Restia",
        "[Auction] settlement",
        Utc::now() - TimeDelta::minutes(10),
        cutoff,
    ));
    assert!(!sent_list_projection_may_match(
        &expectation,
        "isekai",
        "Restia",
        "[Auction] settlement",
        Utc::now() - TimeDelta::minutes(10),
        cutoff,
    ));
    assert!(!sent_list_projection_may_match(
        &expectation,
        "main",
        "Restia",
        "[Auction] settlement",
        cutoff - TimeDelta::seconds(1),
        cutoff,
    ));
}

#[test]
fn detail_match_requires_full_identity_and_assets() {
    let expectation = expectation();
    let subject = expectation.subject.clone().unwrap();
    let cutoff = Utc::now() - TimeDelta::hours(1);
    let sent_at = Utc::now() - TimeDelta::minutes(10);
    assert_eq!(
        sent_detail_match(
            &expectation,
            "main",
            "restia﹒ashdoll",
            &subject,
            sent_at,
            cutoff,
            false,
            0,
            0,
            &["317632875".to_owned()],
        ),
        SentDetailMatch::Exact
    );
    // 已领取：剩余资产为零，只按完整身份与标题判断。
    assert_eq!(
        sent_detail_match(
            &expectation,
            "main",
            &expectation.recipient,
            &subject,
            sent_at,
            cutoff,
            true,
            0,
            0,
            &[],
        ),
        SentDetailMatch::Exact
    );
    // 未领取但资产不一致：不得隔离。
    assert_eq!(
        sent_detail_match(
            &expectation,
            "main",
            &expectation.recipient,
            &subject,
            sent_at,
            cutoff,
            false,
            0,
            0,
            &["317632876".to_owned()],
        ),
        SentDetailMatch::AssetMismatch
    );
    assert_eq!(
        sent_detail_match(
            &expectation,
            "main",
            "Other",
            &subject,
            sent_at,
            cutoff,
            false,
            0,
            0,
            &["317632875".to_owned()],
        ),
        SentDetailMatch::IdentityMismatch
    );
    // 冻结身份必须要求标题一致：同收件人同资产的其它标题不是这次发送。
    assert_eq!(
        sent_detail_match(
            &expectation,
            "main",
            &expectation.recipient,
            "[Auction] something else",
            sent_at,
            cutoff,
            false,
            0,
            0,
            &["317632875".to_owned()],
        ),
        SentDetailMatch::IdentityMismatch
    );
}

/// 恢复库没有冻结尝试时只能按世界/收件人/资产核对；未知资产不得当作不匹配。
#[test]
fn restored_payload_identity_matches_without_subject() {
    let expectation = restored_payload_expectation();
    let cutoff = Utc::now() - TimeDelta::hours(1);
    let sent_at = Utc::now() - TimeDelta::minutes(10);
    // 没有标题也能进入详情候选；世界与时间仍然是硬条件。
    assert!(sent_list_projection_may_match(
        &expectation,
        "main",
        "Restia",
        "[Auction] settlement payout",
        sent_at,
        cutoff,
    ));
    assert!(!sent_list_projection_may_match(
        &expectation,
        "isekai",
        "Restia",
        "[Auction] settlement payout",
        sent_at,
        cutoff,
    ));
    assert!(!sent_list_projection_may_match(
        &expectation,
        "main",
        "Restia",
        "[Auction] settlement payout",
        cutoff - TimeDelta::seconds(1),
        cutoff,
    ));
    // 未领取：金额必须一致。
    assert_eq!(
        sent_detail_match(
            &expectation,
            "main",
            &expectation.recipient,
            "[Auction] settlement payout",
            sent_at,
            cutoff,
            false,
            300_000,
            0,
            &[],
        ),
        SentDetailMatch::Exact
    );
    assert_eq!(
        sent_detail_match(
            &expectation,
            "main",
            &expectation.recipient,
            "[Auction] settlement payout",
            sent_at,
            cutoff,
            false,
            299_999,
            0,
            &[],
        ),
        SentDetailMatch::AssetMismatch
    );
    // 未知装备身份不产生资产不匹配：结论由调用方作为未决阻塞处理。
    assert_eq!(
        sent_detail_match(
            &expectation,
            "main",
            &expectation.recipient,
            "[Auction] settlement payout",
            sent_at,
            cutoff,
            false,
            300_000,
            0,
            &["317632875".to_owned()],
        ),
        SentDetailMatch::Exact
    );
}

#[test]
fn restored_payload_collected_detail_is_unresolved() {
    let expectation = restored_payload_expectation();
    let cutoff = Utc::now() - TimeDelta::hours(1);
    let sent_at = Utc::now() - TimeDelta::minutes(10);
    // 已领取详情不再保留资产；payload 重建没有标题，不能把该 MID 归因到这条投递。
    assert_eq!(
        sent_detail_match(
            &expectation,
            "main",
            &expectation.recipient,
            "[Auction] any subject",
            sent_at,
            cutoff,
            true,
            0,
            0,
            &[],
        ),
        SentDetailMatch::Ambiguous
    );
}

#[test]
fn expectation_rejects_unsorted_equipment_and_non_world() {
    let mut expectation = expectation();
    expectation.equipment_ids = Some(vec!["2".to_owned(), "1".to_owned()]);
    assert!(!expectation.validate());
    expectation.equipment_ids = Some(vec!["1".to_owned()]);
    expectation.world_id = "shared".to_owned();
    assert!(!expectation.validate());
}

/// 冻结身份必须有标题与资产；payload 重建身份不能携带无法证明的标题。
#[test]
fn evidence_shape_is_closed() {
    let mut frozen = expectation();
    frozen.subject = None;
    assert!(!frozen.validate());
    let mut frozen_without_assets = expectation();
    frozen_without_assets.equipment_ids = None;
    assert!(!frozen_without_assets.validate());
    let mut restored = restored_payload_expectation();
    restored.subject = Some("[Auction] guessed".to_owned());
    assert!(!restored.validate());
    assert!(restored_payload_expectation().validate());
}

#[test]
fn unresolved_block_reason_is_closed() {
    let block = ExternalRecoveryUnresolvedBlock {
        delivery_id: Uuid::from_u128(9),
        fencing_version: 2,
        reason: EXTERNAL_RECOVERY_AMBIGUOUS_REASON.to_owned(),
        candidate_mail_ids: vec!["100".to_owned(), "101".to_owned()],
    };
    assert!(block.validate());
    let mut wrong_reason = block.clone();
    wrong_reason.reason = "guessed".to_owned();
    assert!(!wrong_reason.validate());
    let mut bad_mail_id = block;
    bad_mail_id.candidate_mail_ids = vec!["0".to_owned()];
    assert!(!bad_mail_id.validate());
}
