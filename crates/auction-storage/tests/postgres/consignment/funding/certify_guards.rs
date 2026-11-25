//! COD 认证的复验守卫（D9 §4.2 第 1、3 步）：冻结身份、新鲜现金、开户边界与边界链、季末下游，
//! 以及与 6B3 本金结论写口的并发。每个反例都断言整笔回滚：资金、回执、审计与履约状态逐字不变。

use auction_application::settlement::FundingError;
use auction_domain::consignment::funding::{FundingMode, PrincipalConclusion};
use auction_time::{Duration, Timestamp};

use super::super::contracts::fixture::{CONTRACT, OBLIGATION_1};
use super::super::ledger136::fixture::PAY_DELIVERY;
use super::certify::Env;
use super::cod_fixture::{GROSS, OPENING_ENTRY, QUARANTINE, Shape, UID, cash, certify, id, ledger};

fn is_conflict(error: &FundingError) -> bool {
    matches!(error, FundingError::Conflict)
}

#[tokio::test]
async fn postgres_consignment_funding_certify_cod_rechecks_frozen_dispatch_identity() {
    let env = Env::new(Shape::STRICT).await;
    // 原 prepare 摘要、原 activate 摘要、正文摘要、冻结装备集合：逐项不符都整笔拒绝。
    let mut prepare = certify(2, None);
    prepare.freeze.prepare_digest = [0xaa; 32];
    env.assert_rejected(prepare, is_conflict).await;
    let mut activation = certify(3, None);
    activation.freeze.activation_digest = [0xaa; 32];
    env.assert_rejected(activation, is_conflict).await;
    let mut body = certify(4, None);
    body.freeze.body_digest = [0xaa; 32];
    env.assert_rejected(body, is_conflict).await;
    let mut equipment = certify(5, None);
    equipment.freeze.equipment_ids = vec!["4242".to_owned()];
    env.assert_rejected(equipment, is_conflict).await;

    // 正例：冻结身份全部相符才放行，且前面的拒绝没有占用任何命令 ID。
    let receipt = env.store.test_certify_cod(certify(2, None)).await.expect("冻结身份相符");
    assert!(!receipt.replayed);
}

#[tokio::test]
async fn postgres_consignment_funding_certify_cod_rechecks_send_expectation_against_round() {
    // 冻结的 Send 期望与派发轮、投递行必须一致：期望里的主题被改成另一份，整笔拒绝。
    let env = Env::new(Shape::STRICT).await;
    let mut conn = env.pool().acquire().await.expect("维护连接");
    sqlx::query("SET session_replication_role = replica")
        .execute(&mut *conn)
        .await
        .expect("绕过追加守卫");
    sqlx::query(
        "UPDATE public.delivery_send_attempts
         SET expectation = jsonb_set(expectation, '{subject}', '\"another subject\"')",
    )
    .execute(&mut *conn)
    .await
    .expect("篡改期望主题");
    drop(conn);
    env.assert_rejected(certify(2, None), is_conflict).await;
}

#[tokio::test]
async fn postgres_consignment_funding_certify_cod_rechecks_fresh_same_uid_cash() {
    let env = Env::new(Shape::STRICT).await;
    // 现有占用 300（隔离 200 + 补偿 100），全部未含时新发行 U = 102：现金少一分即已被花掉，不放行。
    let mut spent = certify(2, None);
    spent.cash = Some(cash(300 + GROSS - 1));
    env.assert_rejected(spent, |e| matches!(e, FundingError::Unavailable)).await;
    // 观察已过期，或有效窗口超过证据最长时效：不是新鲜现金。
    let mut expired = certify(3, None);
    let stale = cash(1_000_000);
    expired.cash = Some(auction_application::settlement::FreshCashObservation {
        observed_at: Timestamp::now() - Duration::seconds(200),
        expires_at: Timestamp::now() - Duration::seconds(100),
        ..stale.clone()
    });
    env.assert_rejected(expired, |e| matches!(e, FundingError::Unproven)).await;
    let mut too_long = certify(4, None);
    too_long.cash = Some(auction_application::settlement::FreshCashObservation {
        observed_at: Timestamp::now() - Duration::seconds(5),
        expires_at: Timestamp::now() + Duration::seconds(3_600),
        ..stale
    });
    env.assert_rejected(too_long, |e| matches!(e, FundingError::Unproven)).await;
    // 观察属于另一个 UID（或根本没有观察）在端口层就是无效输入，不进事务。
    let mut foreign = certify(5, None);
    foreign.cash.as_mut().expect("严格现金带观察").uid = "7002".to_owned();
    env.assert_rejected(foreign, |e| matches!(e, FundingError::Invalid)).await;
    let mut missing = certify(6, None);
    missing.cash = None;
    env.assert_rejected(missing, |e| matches!(e, FundingError::Invalid)).await;

    // 正例：现金恰好覆盖现有占用加新发行。
    let mut exact = certify(7, None);
    exact.cash = Some(cash(300 + GROSS));
    env.store.test_certify_cod(exact).await.expect("现金恰好覆盖");
}

#[tokio::test]
async fn postgres_consignment_funding_certify_cod_requires_send_strictly_after_boundary() {
    // 原 Send 不晚于开户边界：金额可能已含于 opening，不能再当作新发行。
    let env = Env::new(Shape { send_before_boundary: true, ..Shape::STRICT }).await;
    env.assert_rejected(certify(2, None), |e| matches!(e, FundingError::Unproven)).await;
}

#[tokio::test]
async fn postgres_consignment_funding_certify_cod_rejects_changed_boundary_chain() {
    let env = Env::new(Shape::STRICT).await;
    // 边界之后又出现另一次 opening（调整、结转、重开）：原边界不再能证明这次 Send，整笔待核。
    sqlx::query(
        "INSERT INTO public.settlement_funding_sources
            (id, uid, kind, source_key, amount, proof_digest, created_at)
         VALUES ('c6a10000-0000-4000-8000-0000000000f1', $1, 'reconciliation',
                 'c6a10000-0000-4000-8000-0000000000f1', 0,
                 decode(repeat('02', 32), 'hex'), '2026-01-02T12:00:00Z')",
    )
    .bind(UID)
    .execute(env.pool())
    .await
    .expect("边界之后的另一次 opening");
    env.assert_rejected(certify(2, None), is_conflict).await;
}

#[tokio::test]
async fn postgres_consignment_funding_cod_receipt_insert_fails_closed_on_zero_rows() {
    // Sent MID 没有对应绑定：寄售收据的 INSERT…SELECT 零行。通用收据已写，必须整笔失败关闭，不留来源与收据。
    let env = Env::new(Shape::STRICT).await;
    let mut unbound = certify(2, None);
    unbound.receipt.payment_mail_id = "9999".to_owned();
    let error = env.store.test_insert_receipt(unbound).await.expect_err("零行必须失败");
    assert!(matches!(error, FundingError::Unproven), "零行应按未证明失败关闭：{error:?}");
    let leftovers: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.settlement_funding_receipts),
                (SELECT count(*) FROM public.settlement_funding_sources
                 WHERE kind = 'collected_payment')",
    )
    .fetch_one(env.pool())
    .await
    .expect("读取残留");
    assert_eq!(leftovers, (0, 0), "整笔回滚：没有通用收据，也没有付款来源");
}

#[tokio::test]
async fn postgres_consignment_funding_certify_cod_after_season_end_keeps_original_blocked() {
    // 季窗交割界已过：收款照常认证（责任保全），但不放原件交接；等号起即归季后。
    let env = Env::new(Shape { season_ended: true, ..Shape::STRICT }).await;
    env.store.test_certify_cod(certify(2, None)).await.expect("季后实收仍认证");
    let after = ledger(env.pool()).await;
    assert_eq!(
        (after.payment_state.as_str(), after.handoff_state.as_str()),
        ("satisfied", "blocked"),
        "季末后不放原件"
    );
    assert_eq!((after.receipts, after.funded), (1, 1));
}

/// 另一连接上的 NOWAIT 锁探测：能锁到返回 `true`，被占用（55P03）返回 `false`。
async fn lockable(tx: &mut sqlx::PgConnection, sql: &'static str, key: uuid::Uuid) -> bool {
    sqlx::query("SAVEPOINT probe").execute(&mut *tx).await.expect("探测保存点");
    let locked = sqlx::query(sql).bind(key).execute(&mut *tx).await.is_ok();
    sqlx::query("ROLLBACK TO SAVEPOINT probe").execute(&mut *tx).await.expect("回滚探测");
    locked
}

#[tokio::test]
async fn postgres_consignment_funding_certify_cod_locks_allocations_before_deliveries() {
    // D9 §1.3 / R5 §3.8：obligations → allocations → deliveries → fulfillments。外部连接占住 opening 原余额
    // 行，让认证停在 allocations 段；此时 obligations 已被认证持有，deliveries 与 fulfillments 尚未被锁。
    let env = Env::new(Shape::STRICT).await;
    let sealed = env.seal(OPENING_ENTRY, 60).await;
    let mut holder = env.pool().acquire().await.expect("外部连接");
    sqlx::query("BEGIN").execute(&mut *holder).await.expect("开启外部事务");
    sqlx::query("SELECT id FROM public.settlement_funding_allocations WHERE id = $1 FOR UPDATE")
        .bind(id(QUARANTINE))
        .execute(&mut *holder)
        .await
        .expect("占住 opening 原余额");
    let store = env.store.clone();
    let certifying =
        tokio::spawn(async move { store.test_certify_cod(certify(2, Some(sealed))).await });
    let mut waiting = false;
    for _ in 0..200 {
        let blocked: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity
             WHERE datname = current_database() AND wait_event_type = 'Lock'",
        )
        .fetch_one(env.pool())
        .await
        .expect("读取等待锁的会话");
        if blocked > 0 {
            waiting = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(waiting, "认证应停在 allocations 锁上");
    assert!(
        !lockable(
            &mut holder,
            "SELECT id FROM public.settlement_obligations WHERE contract_id = $1 FOR UPDATE NOWAIT",
            id(CONTRACT),
        )
        .await,
        "obligations 先于 allocations 被锁"
    );
    assert!(
        lockable(
            &mut holder,
            "SELECT id FROM public.deliveries WHERE id = $1 FOR UPDATE NOWAIT",
            id(PAY_DELIVERY)
        )
        .await,
        "deliveries 在 allocations 之后才锁"
    );
    assert!(
        lockable(
            &mut holder,
            "SELECT obligation_id FROM public.settlement_obligation_fulfillments
             WHERE obligation_id = $1 FOR UPDATE NOWAIT",
            id(OBLIGATION_1),
        )
        .await,
        "fulfillments 在 allocations 之后才锁"
    );
    sqlx::query("ROLLBACK").execute(&mut *holder).await.expect("释放外部事务");
    certifying.await.expect("认证任务").expect("放开后认证成功");
}

#[tokio::test]
async fn postgres_consignment_funding_certify_cod_and_principal_fact_race_has_one_winner() {
    // 与 6B3 的本金结论写口争同一槽位：付款本金与 RTS 本金互斥，只有一方成功，另一方整笔回滚，
    // 不死锁（死锁会以存储错误或悬挂暴露）。
    for round in 0..4 {
        let env = Env::new(Shape::STRICT).await;
        let (certified, principal) = tokio::join!(
            env.store.test_certify_cod(certify(2, None)),
            env.store.test_record_principal_fact(
                id(CONTRACT),
                id(OBLIGATION_1),
                id(PAY_DELIVERY),
                PrincipalConclusion::RtsPrincipal,
                FundingMode::StrictCash,
            )
        );
        let certify_won = certified.is_ok();
        let principal_won = matches!(principal, Ok(true));
        assert_ne!(certify_won, principal_won, "第 {round} 轮必须恰有一方成功");
        let loser = if certify_won { principal.err() } else { certified.err() };
        // 败方是回滚的错误（认证侧为冲突；本金写口侧的唯一键冲突目前以存储错误返回，属 6B3 的分类），
        // 死锁与序列化冲突在两侧都会被有界重试，不会以悬挂或成环的形式出现。
        assert!(loser.is_some(), "第 {round} 轮败方必须失败：{loser:?}");
        let after = ledger(env.pool()).await;
        let conclusion: String =
            sqlx::query_scalar("SELECT conclusion FROM public.consignment_principal_facts")
                .fetch_one(env.pool())
                .await
                .expect("恰有一条本金结论");
        if certify_won {
            assert_eq!((conclusion.as_str(), after.receipts), ("payment_principal", 1));
        } else {
            assert_eq!((conclusion.as_str(), after.receipts), ("rts_principal", 0));
            assert_eq!(after.payment_state, "enqueued", "败方整笔回滚，付款义务不变");
        }
    }
}
