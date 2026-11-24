//! 回执逐件 L 必须与同事务暂存事件一致；不一致时整体回滚且零事件、零回执、零审计。

use auction_application::consignment::{DepositActor, DepositOperation, ListingStateChange};
use auction_domain::SettlementWorld;
use auction_domain::consignment::{ListingEquipmentIdentity, ListingState, OriginalMailKey};
use auction_storage::{
    TestDepositLocks, TestDepositTarget, test_deposit_audit_count, test_deposit_command_exists,
    test_deposit_event_count, test_deposit_listing_state,
};
use uuid::Uuid;

use super::support::{LISTING, USER_1, execute, listing_awaiting_deposit, users_and_key};
use crate::support::TestDatabase;

fn locks(command_id: Uuid) -> TestDepositLocks {
    TestDepositLocks {
        command_id,
        sellers: vec![Uuid::parse_str(USER_1).expect("卖家")],
        equipment: vec![ListingEquipmentIdentity::new("main", "42").expect("装备")],
        mail_keys: vec![mail_key()],
    }
}

fn mail_key() -> OriginalMailKey {
    OriginalMailKey::new("12345", SettlementWorld::Main, "555").expect("原信键")
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

#[tokio::test]
async fn postgres_deposit_finish_rejects_receipt_changes_that_do_not_match_staged_events() {
    let database = seed().await;
    let store = database.store().await.expect("连接运行账号");
    let matching = ListingStateChange {
        listing_id: listing_id(),
        listing_version: 3,
        state: ListingState::DepositAccepted,
    };
    let extra_listing = Uuid::from_u128(0x4c00_0000_0000_4000_8000_0000_0000_0001);
    // 触发器只按被更新的挂单核对回执；多余的不一致 L 只能由 `finish` 拒绝。
    let mismatches = [
        vec![
            matching,
            ListingStateChange {
                listing_id: extra_listing,
                listing_version: 4,
                state: ListingState::DepositAccepted,
            },
        ],
        vec![
            matching,
            ListingStateChange {
                listing_id: extra_listing,
                listing_version: 3,
                state: ListingState::Closed,
            },
        ],
    ];
    for (index, changes) in mismatches.into_iter().enumerate() {
        // Worker 命令必须是应用层派生的 UUIDv5，000132 的 actor/uuid 版本约束也这样要求。
        let command_id =
            Uuid::new_v5(&Uuid::NAMESPACE_URL, format!("p4k-receipt-guard-{index}").as_bytes());
        let result = store
            .test_deposit_finish_with_mismatched_listing_change(
                locks(command_id),
                DepositActor::Worker,
                DepositOperation::DecideEnvelope,
                TestDepositTarget::MailKey(mail_key()),
                [0xb1 + index as u8; 32],
                listing_id(),
                3,
                changes,
            )
            .await;
        assert!(result.is_err(), "回执 L 与暂存事件不一致时必须拒绝提交");
        assert!(!test_deposit_command_exists(&store, command_id).await, "失败时不得留下命令回执");
        assert_eq!(
            test_deposit_event_count(&store, command_id).await.expect("事件数"),
            0,
            "失败时不得留下事件"
        );
        assert_eq!(
            test_deposit_audit_count(&store, command_id).await.expect("审计数"),
            0,
            "失败时不得留下审计"
        );
        assert_eq!(
            test_deposit_listing_state(&store, listing_id()).await.expect("挂单状态"),
            ("awaiting_deposit".into(), 2),
            "失败时挂单更新也必须回滚"
        );
    }
}
