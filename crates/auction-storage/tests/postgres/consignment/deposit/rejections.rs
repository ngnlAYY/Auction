//! v2 确定拒绝：每个闭集变体都经 000134 CHECK 落库、写拒绝审计、零业务事件，并支持重放。

use auction_application::consignment::{
    ConsignmentError, DepositActor, DepositBusinessRejection, DepositOperation,
};
use auction_domain::UserId;
use auction_domain::consignment::{ListingEquipmentIdentity, OriginalMailKey};
use auction_storage::{
    TestDepositLocks, TestDepositOutcome, TestDepositRejection, TestDepositTarget,
    test_deposit_audit_count, test_deposit_command_exists, test_deposit_event_count,
    test_deposit_rejected_audit_count,
};
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

use super::support::{LISTING, USER_1, execute, listing_awaiting_deposit, users_and_key};
use crate::support::TestDatabase;

fn locks(command_id: Uuid) -> TestDepositLocks {
    TestDepositLocks {
        command_id,
        sellers: vec![Uuid::parse_str(USER_1).expect("卖家")],
        equipment: vec![ListingEquipmentIdentity::new("main", "42").expect("装备")],
        mail_keys: vec![
            OriginalMailKey::new("12345", auction_domain::SettlementWorld::Main, "555")
                .expect("原信键"),
        ],
    }
}

async fn seed() -> TestDatabase {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接运行账号");
    execute(store.postgres_pool(), &users_and_key()).await.expect("写入用户");
    execute(store.postgres_pool(), &listing_awaiting_deposit()).await.expect("写入挂单");
    database
}

fn listing_id() -> Uuid {
    Uuid::parse_str(LISTING).expect("挂单 UUID")
}

fn public_user() -> DepositActor {
    DepositActor::PublicUser(UserId::from_uuid(Uuid::parse_str(USER_1).expect("卖家 UUID")))
}

fn cases() -> Vec<(&'static str, i16, TestDepositRejection)> {
    use DepositBusinessRejection as Business;
    use TestDepositRejection as Rejection;
    vec![
        ("stale_version", 409, Rejection::Business(Business::StaleVersion)),
        ("listing_not_active", 409, Rejection::Business(Business::ListingNotActive)),
        ("equipment_busy", 409, Rejection::Business(Business::EquipmentBusy)),
        ("participant_ineligible", 403, Rejection::Business(Business::ParticipantIneligible)),
        ("bot_party", 403, Rejection::Business(Business::BotParty)),
        ("season_closed", 409, Rejection::Business(Business::SeasonClosed)),
        ("mail_unavailable", 503, Rejection::Business(Business::MailUnavailable)),
        ("source_mismatch", 409, Rejection::Business(Business::SourceMismatch)),
        ("evidence_unproven", 409, Rejection::Business(Business::EvidenceUnproven)),
        ("evidence_conflict", 409, Rejection::Business(Business::EvidenceConflict)),
        ("disposition_conflict", 409, Rejection::Business(Business::DispositionConflict)),
        ("hold_mismatch", 409, Rejection::Business(Business::HoldMismatch)),
        ("action_not_allowed", 403, Rejection::Business(Business::ActionNotAllowed)),
        ("fence_mismatch", 409, Rejection::Business(Business::FenceMismatch)),
        ("listing_limit_reached", 429, Rejection::ListingLimitReached),
        ("equipment_committed", 409, Rejection::EquipmentCommitted),
        ("custody_unproven", 409, Rejection::CustodyUnproven),
        ("operations_hold", 409, Rejection::OperationsHold),
        ("consignment_disabled", 404, Rejection::Disabled),
    ]
}

#[tokio::test]
async fn postgres_deposit_rejections_persist_audit_and_replay_without_business_events() {
    let database = seed().await;
    let store = database.store().await.expect("连接运行账号");
    for (index, (code, status, rejection)) in cases().into_iter().enumerate() {
        let command_id = Uuid::from_u128(0x4a00_0000_0000_4000_8000_0000_0000_c000 + index as u128);
        let digest = [0xc0 + index as u8; 32];
        let first = store
            .test_deposit_submit_rejection(
                locks(command_id),
                public_user(),
                DepositOperation::IssueReservation,
                TestDepositTarget::Listing(listing_id()),
                digest,
                rejection,
            )
            .await
            .unwrap_or_else(|error| panic!("拒绝 {code} 落库失败：{error:?}"));
        let TestDepositOutcome::First(stored) = first else {
            panic!("拒绝 {code} 首次不应重放");
        };
        assert_eq!(stored.status, status, "{code} 状态码");
        assert_eq!(stored.code, code, "{code} 账本结果码");
        assert_eq!(stored.body["schema_version"], 2);
        assert_eq!(stored.body["disposition"], "rejected");
        assert_eq!(stored.body["code"], code);
        assert_eq!(stored.body["command_id"], command_id.to_string());

        let row = sqlx::query(
            "SELECT result_status, result_code, result_body
             FROM public.consignment_commands WHERE command_id = $1",
        )
        .bind(command_id)
        .fetch_one(store.postgres_pool())
        .await
        .expect("读回拒绝账本");
        let persisted_status: i16 = row.try_get("result_status").expect("状态码");
        let persisted_code: String = row.try_get("result_code").expect("结果码");
        let persisted_body: Value = row.try_get("result_body").expect("回执");
        assert_eq!(persisted_status, status, "{code} 落库状态码");
        assert_eq!(persisted_code, code, "{code} 落库结果码");
        assert_eq!(persisted_body, stored.body, "{code} 落库回执");

        assert_eq!(
            test_deposit_audit_count(&store, command_id).await.expect("审计数"),
            1,
            "{code} 必须写一条审计"
        );
        assert_eq!(
            test_deposit_rejected_audit_count(&store, command_id).await.expect("拒绝审计数"),
            1,
            "{code} 必须写一条拒绝审计"
        );
        assert_eq!(
            test_deposit_event_count(&store, command_id).await.expect("事件数"),
            0,
            "{code} 不得写业务事件"
        );

        let replay = store
            .test_deposit_submit_rejection(
                locks(command_id),
                public_user(),
                DepositOperation::IssueReservation,
                TestDepositTarget::Listing(listing_id()),
                digest,
                rejection,
            )
            .await
            .expect("同请求重放拒绝");
        let TestDepositOutcome::Replay(replayed) = replay else {
            panic!("拒绝 {code} 应重放首次回执");
        };
        assert_eq!(replayed, stored, "{code} 重放必须逐字段相同");

        let error = store
            .test_deposit_submit_rejection(
                locks(command_id),
                public_user(),
                DepositOperation::IssueReservation,
                TestDepositTarget::Listing(listing_id()),
                [0xdf; 32],
                rejection,
            )
            .await
            .expect_err("同 ID 异请求必须冲突");
        assert_eq!(error, ConsignmentError::CommandIdConflict, "{code} 异请求冲突");
        assert!(test_deposit_command_exists(&store, command_id).await);
        assert_eq!(
            test_deposit_audit_count(&store, command_id).await.expect("审计数"),
            1,
            "{code} 冲突不得追加审计"
        );
        assert_eq!(
            test_deposit_event_count(&store, command_id).await.expect("事件数"),
            0,
            "{code} 冲突不得追加事件"
        );
    }
}
