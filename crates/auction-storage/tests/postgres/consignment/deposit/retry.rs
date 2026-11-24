//! v2 路径的 40001/40P01 有界重试与非重试错误短路。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use auction_application::consignment::{
    ConsignmentError, DepositActor, DepositBusinessRejection, DepositOperation, WithdrawDisposition,
};
use auction_domain::CommandId;
use auction_domain::consignment::{ListingEquipmentIdentity, ListingState, ListingVersion};
use auction_storage::{
    TestDepositLocks, TestDepositOutcome, TestDepositReceipt, TestDepositTarget,
    test_deposit_audit_count, test_deposit_command_exists, test_deposit_event_count,
    test_deposit_listing_state,
};
use uuid::Uuid;

use super::support::{LISTING, REPLAY, USER_1, execute, listing_awaiting_deposit, users_and_key};
use crate::support::TestDatabase;

fn locks(command_id: Uuid) -> TestDepositLocks {
    TestDepositLocks {
        command_id,
        sellers: vec![Uuid::parse_str(USER_1).expect("卖家")],
        equipment: vec![ListingEquipmentIdentity::new("main", "42").expect("装备")],
        mail_keys: vec![
            auction_domain::consignment::OriginalMailKey::new(
                "12345",
                auction_domain::SettlementWorld::Main,
                "555",
            )
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
    DepositActor::PublicUser(auction_domain::UserId::from_uuid(
        Uuid::parse_str(USER_1).expect("卖家 UUID"),
    ))
}

/// 第一次尝试确定序列化失败，第二次以同一输入成功，且事实与回执只写一份。
#[tokio::test]
async fn postgres_deposit_v2_serialization_conflict_retries_once_with_the_same_input() {
    let database = seed().await;
    let store = database.store().await.expect("连接运行账号");
    let command_id = Uuid::from_u128(0x4800_0000_0000_4000_8000_0000_0000_9001);
    let attempts = Arc::new(AtomicUsize::new(0));
    let receipt = auction_application::consignment::WithdrawDepositReceipt::new(
        CommandId::from_uuid(command_id),
        WithdrawDisposition::Closed,
        listing_id(),
        ListingVersion::new(3).expect("版本"),
        ListingState::Closed,
        None,
    );
    let outcome = store
        .test_deposit_serialization_conflict_retries(
            locks(command_id),
            public_user(),
            DepositOperation::Withdraw,
            TestDepositTarget::Listing(listing_id()),
            [0x91; 32],
            TestDepositReceipt::Withdraw(receipt),
            Uuid::parse_str(REPLAY).expect("回放钥 UUID"),
            Arc::clone(&attempts),
        )
        .await
        .expect("第二次尝试必须成功");
    assert_eq!(attempts.load(Ordering::SeqCst), 2, "闭包必须恰好执行两次");
    let TestDepositOutcome::First(stored) = outcome else {
        panic!("重试成功后应首次提交");
    };
    assert_eq!(stored.status, 200);
    assert!(test_deposit_command_exists(&store, command_id).await);
    assert_eq!(test_deposit_event_count(&store, command_id).await.expect("事件数"), 1);
    assert_eq!(test_deposit_audit_count(&store, command_id).await.expect("审计数"), 1);
    assert_eq!(
        test_deposit_listing_state(&store, listing_id()).await.expect("挂单状态"),
        ("closed".into(), 3)
    );
}

/// 非 40001/40P01 的错误不得重试。
#[tokio::test]
async fn postgres_deposit_v2_non_retryable_errors_run_the_closure_once() {
    let database = seed().await;
    let store = database.store().await.expect("连接运行账号");
    let cases = [
        ConsignmentError::OutcomeUnknown,
        ConsignmentError::from(DepositBusinessRejection::StaleVersion),
    ];
    for (index, error) in cases.into_iter().enumerate() {
        let command_id = Uuid::from_u128(0x4800_0000_0000_4000_8000_0000_0000_a000 + index as u128);
        let attempts = Arc::new(AtomicUsize::new(0));
        let result = store
            .test_deposit_single_attempt_error(locks(command_id), Arc::clone(&attempts), error)
            .await;
        assert!(result.is_err(), "非重试错误必须原样返回");
        assert_eq!(attempts.load(Ordering::SeqCst), 1, "非重试错误只能执行一次");
        assert!(!test_deposit_command_exists(&store, command_id).await, "非重试错误不得写命令事实");
    }
}
