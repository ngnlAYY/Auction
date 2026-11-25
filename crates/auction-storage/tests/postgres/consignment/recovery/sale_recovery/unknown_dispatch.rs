//! 撤回与重列的“未知派发”判据：只有已起写（有 `delivery_send_attempts`）而没有 Sent 绑定的派发才算未知。
//!
//! 保护的不变量：4（外部结果未知不放行相反外写）、6（失败关闭）。确定没有起写的失败派发
//! 一定没有发出，不阻断撤回与重列。

use auction_application::consignment::{
    ConsignmentError, CustodyControlStore, CustodyRelistStore, DepositBusinessRejection,
    RelistCommand, WithdrawDepositCommand,
};
use auction_application::core::RequestHash;
use auction_domain::consignment::ListingVersion;

use super::super::super::contracts::fixture::OBLIGATION_3;
use super::super::super::deposit::support::{LISTING, succeeds};
use super::super::super::ledger136::fixture::{
    PURCHASED_AT, REFUND_ATTEMPT, REFUND_DELIVERY, REFUND_ROUND, round_sql,
};
use super::{Closure, World, command_id, id, seller, unknown_refund_sql};

/// 买家退款腿：投递已失败、从未起写（没有 `delivery_send_attempts` 与资产腿尝试），也没有 Sent 绑定。
fn failed_unstarted_refund_sql() -> String {
    let command = format!("c71{}", &REFUND_DELIVERY[3..]);
    format!(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, failure_code, result_metadata, fencing_version, attempt_count,
             completed_at, created_at, updated_at)
         VALUES ('{REFUND_DELIVERY}', '{command}', decode(repeat('c1', 32), 'hex'), 'settlement',
                 '{OBLIGATION_3}', 'FakeSellerTwo', '{{}}'::jsonb, 'failed', 'invalid_payload',
                 '{{\"stage\":\"preflight_blocked\",\"write_started\":false,\"outcome_unknown\":false,\"failure_code\":\"invalid_payload\"}}'::jsonb,
                 0, 0, '{PURCHASED_AT}', '{PURCHASED_AT}', '{PURCHASED_AT}');"
    ) + round_only(&round_sql(REFUND_ROUND, REFUND_DELIVERY, OBLIGATION_3, REFUND_ATTEMPT, "cash_refund"))
}

/// 只保留派发轮语句：资产腿尝试行以发送尝试为外键，从未起写的派发不会有它。
fn round_only(sql: &str) -> &str {
    sql.split("INSERT INTO public.consignment_dispatch_attempts").next().unwrap_or(sql)
}

/// 已起写（有发送尝试）、没有 Sent 绑定，投递随后记为 failed 但没有可证明未写出的证据。
fn failed_started_refund_sql() -> String {
    unknown_refund_sql()
        + &format!(
            "UPDATE public.deliveries SET status = 'failed', failure_code = 'gateway_outcome_unknown',
                 completed_at = '{PURCHASED_AT}'
             WHERE id = '{REFUND_DELIVERY}';"
        )
}

fn withdraw(seed: u128, hash: u8) -> WithdrawDepositCommand {
    WithdrawDepositCommand {
        command_id: command_id(seed),
        seller_user_id: seller(),
        listing_id: id(LISTING),
        expected_version: ListingVersion::new(6).expect("版本"),
        request_hash: RequestHash::new([hash; 32]),
        request_id: None,
    }
}

fn relist(seed: u128, hash: u8) -> RelistCommand {
    RelistCommand {
        command_id: command_id(seed),
        seller_user_id: seller(),
        listing_id: id(LISTING),
        expected_version: ListingVersion::new(6).expect("版本"),
        request_hash: RequestHash::new([hash; 32]),
        request_id: None,
    }
}

/// 正例：失败但从未起写的退款派发不阻断撤回，也不阻断重列。
#[tokio::test]
async fn postgres_failed_unstarted_dispatch_does_not_block_withdrawal_or_relist() {
    let withdrawing = World::sold(Closure::CancelledUnpaid).await;
    succeeds(&withdrawing.owner, &failed_unstarted_refund_sql()).await;
    let receipt = withdrawing.deposit.request_withdrawal(withdraw(1, 0x61)).await;
    assert!(receipt.is_ok(), "{receipt:?}");

    let relisting = World::sold(Closure::CancelledUnpaid).await;
    succeeds(&relisting.owner, &failed_unstarted_refund_sql()).await;
    let receipt = relisting.deposit.relist(relist(2, 0x62)).await;
    assert!(receipt.is_ok(), "{receipt:?}");
}

/// 反例：已起写而没有终局证据（无 Sent 绑定），即使投递已记 failed，撤回与重列仍被拒，零痕迹。
#[tokio::test]
async fn postgres_failed_started_dispatch_without_evidence_still_blocks_withdrawal_and_relist() {
    let withdrawing = World::sold(Closure::CancelledUnpaid).await;
    succeeds(&withdrawing.owner, &failed_started_refund_sql()).await;
    let before = (withdrawing.position().await, withdrawing.listing().await);
    let refused = withdrawing.deposit.request_withdrawal(withdraw(3, 0x63)).await;
    assert!(
        matches!(
            refused,
            Err(ConsignmentError::Deposit(DepositBusinessRejection::CustodyUnproven))
        ),
        "{refused:?}"
    );
    assert_eq!((withdrawing.position().await, withdrawing.listing().await), before);
    assert_eq!(withdrawing.generations().await.len(), 1);

    let relisting = World::sold(Closure::CancelledUnpaid).await;
    succeeds(&relisting.owner, &failed_started_refund_sql()).await;
    let before = (relisting.position().await, relisting.listing().await);
    let refused = relisting.deposit.relist(relist(4, 0x64)).await;
    assert!(
        matches!(refused, Err(ConsignmentError::Deposit(DepositBusinessRejection::EquipmentBusy))),
        "{refused:?}"
    );
    assert_eq!((relisting.position().await, relisting.listing().await), before);
    assert_eq!(relisting.generations().await.len(), 1);
}
