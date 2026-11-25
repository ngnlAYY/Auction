//! 起写：`write_started` 先于 POST、每投递至多一个首次许可；崩溃重启与并发都不产生第二次 POST。

use auction_application::consignment::{
    ConsignmentError, DepositBusinessRejection, DispatchStore, MarkWriteStarted, WriteStart,
    WriteStartOutcome,
};
use auction_application::core::RequestHash;
use auction_domain::CommandId;
use auction_domain::consignment::dispatch::{AttemptKey, dispatch_attempt_id};
use sqlx::PgPool;
use uuid::Uuid;

use super::super::contracts::fixture::{CONTRACT, OBLIGATION_1};
use super::super::deposit::support::{POSITION, fails};
use super::fixture::{PRICE, Seeded, binding, frozen, round_key};

/// 冻结 Send 尝试与寄售尝试行数。
async fn attempt_rows(pool: &PgPool, delivery: Uuid) -> (i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.delivery_send_attempts WHERE delivery_id = $1),
                (SELECT count(*) FROM public.consignment_dispatch_attempts WHERE delivery_id = $1)",
    )
    .bind(delivery)
    .fetch_one(pool)
    .await
    .expect("读取尝试行数")
}

#[tokio::test]
async fn postgres_consignment_dispatch_write_started_precedes_post_and_attempt_is_unique() {
    let seeded = Seeded::purchased().await;
    let pool = seeded.pool();
    let token = Uuid::new_v4();
    seeded.claim(token, 1).await;
    let attempt = AttemptKey { delivery_id: seeded.delivery_id, fence: 1 };
    let service = seeded.service();
    service.start_attempt(round_key(), attempt, token, frozen(PRICE)).await.expect("登记尝试");
    // 起写之前没有冻结身份与尝试：此时不存在任何可 POST 的持久事实。
    assert_eq!(attempt_rows(pool, seeded.delivery_id).await, (0, 0));

    let started =
        service.mark_write_started(attempt, token, Some(binding())).await.expect("提交起写");
    let WriteStart::FirstCommitted { receipt, permit } = started else {
        panic!("首次起写必须携带许可");
    };
    // 拿到许可时，起写事实已在另一连接可见：投递 sending，冻结尝试与寄售尝试各一行。
    assert_eq!((permit.delivery_id(), permit.fence()), (seeded.delivery_id, 1));
    assert_eq!(receipt.attempt_id, dispatch_attempt_id(&attempt));
    let status: String = sqlx::query_scalar("SELECT status FROM public.deliveries WHERE id = $1")
        .bind(seeded.delivery_id)
        .fetch_one(pool)
        .await
        .expect("投递状态");
    assert_eq!(status, "sending");
    assert_eq!(attempt_rows(pool, seeded.delivery_id).await, (1, 1));
    // 原件腿的首次起写同事务把 position 推进到 in_transit 并追加同版本事件。
    let position: (String, i64) = sqlx::query_as(
        "SELECT state, version FROM public.consignment_custody_positions WHERE id = $1::uuid",
    )
    .bind(POSITION)
    .fetch_one(pool)
    .await
    .expect("position");
    assert_eq!(position, ("in_transit".to_owned(), 4));
    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_custody_events
         WHERE position_id = $1::uuid AND version = 4 AND event_kind = 'in_transit'",
    )
    .bind(POSITION)
    .fetch_one(pool)
    .await
    .expect("事件");
    assert_eq!(events, 1);
    // 反例：同 (投递, fence) 不能有第二条尝试。
    fails(
        pool,
        &format!(
            "INSERT INTO public.consignment_dispatch_attempts
                (delivery_id, fence, attempt_id, contract_id, obligation_id, round, leg, started_at)
             VALUES ('{}', 1, gen_random_uuid(), '{CONTRACT}', '{OBLIGATION_1}', 1, 'original',
                     statement_timestamp());",
            seeded.delivery_id
        ),
        "23505",
        Some("c6_attempt_pk"),
    )
    .await;
}

#[tokio::test]
async fn postgres_consignment_dispatch_crash_before_start_gives_the_new_fence_the_only_post() {
    let seeded = Seeded::purchased().await;
    let service = seeded.service();
    // 第一个执行者领取（fence 1）后在起写前崩溃；租约过期后新执行者领取（fence 2）。
    let stale_token = Uuid::new_v4();
    seeded.claim(stale_token, 1).await;
    let new_token = Uuid::new_v4();
    seeded.reclaim(new_token, 2).await;
    let stale = AttemptKey { delivery_id: seeded.delivery_id, fence: 1 };
    let current = AttemptKey { delivery_id: seeded.delivery_id, fence: 2 };
    // 迟到的旧执行者不得起写：确定拒绝，零许可、零尝试。
    let denied = service
        .mark_write_started(stale, stale_token, Some(binding()))
        .await
        .expect_err("旧 fence");
    assert_eq!(denied, ConsignmentError::Deposit(DepositBusinessRejection::FenceMismatch));
    assert_eq!(attempt_rows(seeded.pool(), seeded.delivery_id).await, (0, 0));
    // 新执行者恰得一个许可。
    let started = service
        .mark_write_started(current, new_token, Some(binding()))
        .await
        .expect("新 fence 起写");
    assert!(matches!(started, WriteStart::FirstCommitted { .. }));
    assert_eq!(attempt_rows(seeded.pool(), seeded.delivery_id).await, (1, 1));
}

#[tokio::test]
async fn postgres_consignment_dispatch_crash_after_start_never_issues_a_second_post() {
    let (seeded, started) = Seeded::first_dispatch_started().await;
    let service = seeded.service();
    // 起写后、网络前崩溃重启：租约过期，新 fence 领取（投递仍是 sending）。
    let new_token = Uuid::new_v4();
    seeded.reclaim(new_token, 2).await;
    let retry = AttemptKey { delivery_id: seeded.delivery_id, fence: 2 };
    let again =
        service.mark_write_started(retry, new_token, Some(binding())).await.expect("新 fence");
    assert!(matches!(again, WriteStart::AlreadyCommitted(_)), "已起写的投递不再发许可");
    // 登记尝试的回执与起写回执指向同一个确定性尝试 ID。
    assert_eq!(started.attempt_receipt.attempt_id, dispatch_attempt_id(&started.attempt));
    // 原命令重放同样只是“已提交”。
    let replay =
        service.mark_write_started(started.attempt, started.claim_token, Some(binding())).await;
    assert!(matches!(replay, Ok(WriteStart::AlreadyCommitted(_))));
    // 持久事实仍只有第一次起写：一条冻结尝试、一条寄售尝试，fence 为 1。
    assert_eq!(attempt_rows(seeded.pool(), seeded.delivery_id).await, (1, 1));
    let fence: i64 = sqlx::query_scalar(
        "SELECT fence FROM public.consignment_dispatch_attempts WHERE delivery_id = $1",
    )
    .bind(seeded.delivery_id)
    .fetch_one(seeded.pool())
    .await
    .expect("原 fence");
    assert_eq!(fence, 1);
}

#[tokio::test]
async fn postgres_consignment_dispatch_concurrent_write_starts_issue_exactly_one_permit() {
    let seeded = Seeded::purchased().await;
    let token = Uuid::new_v4();
    seeded.claim(token, 1).await;
    let attempt = AttemptKey { delivery_id: seeded.delivery_id, fence: 1 };
    // 两个执行者以不同命令 ID 同时起写同一 fence：只有一个得到首次提交。
    let command = |byte: u8| MarkWriteStarted {
        command_id: CommandId::from_uuid(Uuid::from_u128(
            0x5000_0000_0000_5000_8000_0000_0000_0000 | u128::from(byte),
        )),
        request_hash: RequestHash::new([byte; 32]),
        attempt,
        claim_token: token,
        credential: Some(binding()),
    };
    let (left, right) = tokio::join!(
        seeded.runtime.mark_write_started(command(1)),
        seeded.runtime.mark_write_started(command(2)),
    );
    let outcomes = [left.expect("起写 1"), right.expect("起写 2")];
    let firsts = outcomes
        .iter()
        .filter(|outcome| matches!(outcome, WriteStartOutcome::FirstCommitted(_)))
        .count();
    assert_eq!(firsts, 1, "{outcomes:?}");
    assert_eq!(attempt_rows(seeded.pool(), seeded.delivery_id).await, (1, 1));
}

#[tokio::test]
async fn postgres_consignment_dispatch_write_start_command_id_replays_and_conflicts() {
    let seeded = Seeded::purchased().await;
    let token = Uuid::new_v4();
    seeded.claim(token, 1).await;
    let attempt = AttemptKey { delivery_id: seeded.delivery_id, fence: 1 };
    let command = MarkWriteStarted {
        command_id: CommandId::from_uuid(Uuid::from_u128(
            0x5100_0000_0000_5000_8000_0000_0000_0001,
        )),
        request_hash: RequestHash::new([7; 32]),
        attempt,
        claim_token: token,
        credential: Some(binding()),
    };
    let first = seeded.runtime.mark_write_started(command.clone()).await.expect("首次起写");
    assert!(matches!(first, WriteStartOutcome::FirstCommitted(_)));
    // 同 ID 同请求：回放原结果，不再是首次提交。
    let replay = seeded.runtime.mark_write_started(command.clone()).await.expect("回放");
    assert!(matches!(replay, WriteStartOutcome::AlreadyCommitted(_)));
    // 同 ID 不同请求：冲突，不写任何事实。
    let other = MarkWriteStarted { request_hash: RequestHash::new([8; 32]), ..command };
    assert_eq!(
        seeded.runtime.mark_write_started(other).await.expect_err("异请求"),
        ConsignmentError::CommandIdConflict
    );
    assert_eq!(attempt_rows(seeded.pool(), seeded.delivery_id).await, (1, 1));
}
