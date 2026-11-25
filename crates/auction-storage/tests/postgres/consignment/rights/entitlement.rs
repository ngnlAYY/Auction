//! `EntitlementRecipient` 与受限会话边界（7D-6）：冻结受益人核对、身份连续性、会话只触及自己的权利、
//! 损失权利只读、状态刷新只给可执行动作。

use auction_application::consignment::{
    ClaimDisposition, ClaimRightCommand, RightsStore, RightsStoreError,
};
use auction_application::core::RequestHash;
use auction_domain::CommandId;
use auction_domain::consignment::rights::{RightAction, RightKind, session_digest};
use auction_storage::{
    EntitlementDecision, EntitlementRequest, RetainReason, authorize_entitlement_recipient_in_tx,
};
use uuid::Uuid;

use super::super::contracts::fixture::{CONTRACT, OBLIGATION_1, OBLIGATION_2};
use super::super::deposit::support::{USER_1, USER_2};
use super::fixture::{Rig, SeededRight, browser, challenge_command, verify_command};

const BUYER_NAME: &str = "FakeSellerTwo";
const SELLER_NAME: &str = "FakeSellerOne";

fn uuid(text: &str) -> Uuid {
    Uuid::parse_str(text).expect("夹具 UUID")
}

/// 以与生产相同的派生登记买方 / 卖方权利（SQL 的 `c7_right_id`），状态版本 1 为 active。
async fn register_derived(rig: &Rig) {
    for (role, kind, user, identity) in [
        ("buyer", "continue_order", USER_2, BUYER_NAME),
        ("seller", "claim_payment", USER_1, SELLER_NAME),
    ] {
        sqlx::query(
            "INSERT INTO public.consignment_rights
                (right_id, locator_digest, beneficiary_user_id, frozen_identity, world_id, kind,
                 contract_id, position_id, created_command_id, created_at)
             SELECT public.c7_right_id($1, $2), sha256(convert_to($2 || 'entitlement-fixture', 'UTF8')),
                    $3, $4, 'main', $5, $1, NULL, origin_command_id, created_at
             FROM public.consignment_contracts WHERE contract_id = $1",
        )
        .bind(uuid(CONTRACT))
        .bind(role)
        .bind(uuid(user))
        .bind(identity)
        .bind(kind)
        .execute(&rig.pool)
        .await
        .expect("登记权利");
    }
    sqlx::query(
        "INSERT INTO public.consignment_right_states (right_id, version, state, allowed_actions, occurred_at)
         SELECT right_id, 1, 'active', ARRAY[]::text[], created_at FROM public.consignment_rights
         WHERE contract_id = $1",
    )
    .bind(uuid(CONTRACT))
    .execute(&rig.pool)
    .await
    .expect("登记权利状态");
}

/// 生产授权点的形状：在锁定履约行的事务里核对，核对结果只在该锁保护的事务内有效。
async fn authorize(rig: &Rig, request: &EntitlementRequest) -> EntitlementDecision {
    let mut tx = rig.pool.begin().await.expect("开启事务");
    lock_fulfillment(&mut tx, request.obligation_id).await;
    let decision = authorize_entitlement_recipient_in_tx(&mut tx, request).await.expect("核对");
    tx.commit().await.expect("提交");
    decision
}

async fn lock_fulfillment(tx: &mut sqlx::PgConnection, obligation_id: Uuid) {
    sqlx::query(
        "SELECT 1 FROM public.settlement_obligation_fulfillments WHERE obligation_id = $1 FOR UPDATE",
    )
    .bind(obligation_id)
    .fetch_one(tx)
    .await
    .expect("锁履约行");
}

fn request(obligation: &str, user: Option<&str>, recipient: &str) -> EntitlementRequest {
    EntitlementRequest {
        obligation_id: uuid(obligation),
        beneficiary_user_id: user.map(uuid),
        frozen_recipient: recipient.to_owned(),
    }
}

/// 外写面的行数：投递与寄售派发尝试；授权核对只读，不得新增。
async fn send_surface(rig: &Rig) -> i64 {
    rig.count(
        "SELECT (SELECT count(*) FROM public.deliveries)
              + (SELECT count(*) FROM public.consignment_dispatch_attempts)",
    )
    .await
}

#[tokio::test]
async fn postgres_consignment_entitlement_authorizes_the_frozen_beneficiary_even_when_blacklisted()
{
    let rig = Rig::new().await;
    // 反例先行：合同没有权利行时失败关闭，不授权。
    assert_eq!(
        authorize(&rig, &request(OBLIGATION_1, Some(USER_2), BUYER_NAME)).await,
        EntitlementDecision::Retain(RetainReason::RightMissing)
    );
    register_derived(&rig).await;
    let buyer = request(OBLIGATION_1, Some(USER_2), BUYER_NAME);
    // 正例：买方（带用户 ID）与卖方（只带游戏身份的卖款）都按冻结受益人授权；
    // 买方被拉黑后仍授权——黑名单只阻断新购，不剥夺既有订单的权利。
    sqlx::query("UPDATE public.users SET status = 'blacklisted' WHERE id = $1")
        .bind(uuid(USER_2))
        .execute(&rig.pool)
        .await
        .expect("拉黑买方");
    assert_eq!(authorize(&rig, &buyer).await, EntitlementDecision::Authorized);
    assert_eq!(
        authorize(&rig, &request(OBLIGATION_2, None, SELLER_NAME)).await,
        EntitlementDecision::Authorized
    );
    // 反例：受益人 UUID 或冻结收件名与合同冻结值不符，一律保留权利、不授权。
    for wrong in [
        request(OBLIGATION_1, Some(USER_1), BUYER_NAME),
        request(OBLIGATION_1, Some(USER_2), "SomeoneElse"),
        request(OBLIGATION_2, None, BUYER_NAME),
    ] {
        assert_eq!(
            authorize(&rig, &wrong).await,
            EntitlementDecision::Retain(RetainReason::BeneficiaryMismatch)
        );
    }
}

#[tokio::test]
async fn postgres_consignment_entitlement_retains_the_right_and_sends_nothing_when_continuity_is_unproven()
 {
    let rig = Rig::new().await;
    register_derived(&rig).await;
    let before = send_surface(&rig).await;
    // 买方改名：冻结名可能已属于别人，站内改名不证明游戏内同一角色。
    sqlx::query("UPDATE public.users SET username = 'FakeRenamedBuyer' WHERE id = $1")
        .bind(uuid(USER_2))
        .execute(&rig.pool)
        .await
        .expect("改名");
    assert_eq!(
        authorize(&rig, &request(OBLIGATION_1, Some(USER_2), BUYER_NAME)).await,
        EntitlementDecision::Retain(RetainReason::IdentityContinuityUnproven)
    );
    // 权利原样保留（仍 active、无新状态版本），零发送面变化。
    assert_eq!(
        rig.count(
            "SELECT count(*) FROM public.consignment_right_states WHERE state = 'active' AND version = 1"
        )
        .await,
        2
    );
    assert_eq!(rig.count("SELECT count(*) FROM public.consignment_right_states").await, 2);
    assert_eq!(send_surface(&rig).await, before);
    // 反例：改回冻结名后连续性成立，授权恢复（权利从未被消耗）；权利终态则不授权。
    sqlx::query("UPDATE public.users SET username = $2 WHERE id = $1")
        .bind(uuid(USER_2))
        .bind(BUYER_NAME)
        .execute(&rig.pool)
        .await
        .expect("改回");
    assert_eq!(
        authorize(&rig, &request(OBLIGATION_1, Some(USER_2), BUYER_NAME)).await,
        EntitlementDecision::Authorized
    );
    sqlx::query(
        "INSERT INTO public.consignment_right_states (right_id, version, state, allowed_actions, occurred_at)
         SELECT right_id, 2, 'revoked', ARRAY[]::text[], now() FROM public.consignment_rights
         WHERE kind = 'continue_order'",
    )
    .execute(&rig.pool)
    .await
    .expect("撤销权利");
    assert_eq!(
        authorize(&rig, &request(OBLIGATION_1, Some(USER_2), BUYER_NAME)).await,
        EntitlementDecision::Retain(RetainReason::RightNotActive)
    );
}

/// 发码、解封、核验，返回会话令牌摘要与版本。
async fn session_for(
    rig: &Rig,
    right: &SeededRight,
) -> (auction_domain::consignment::rights::SessionDigest, u64) {
    let issued = rig
        .store
        .challenge(challenge_command(Uuid::new_v4(), 7, "198.51.100.7", right))
        .await
        .expect("发码");
    let code = rig.open_code(issued.challenge_id).await;
    let granted = rig
        .store
        .verify(verify_command(Uuid::new_v4(), 7, issued.challenge_id, code))
        .await
        .expect("核验");
    (session_digest(&granted.session_token), granted.right_version)
}

#[tokio::test]
async fn postgres_consignment_rights_session_only_reaches_its_own_right_and_cannot_redirect_the_order()
 {
    let rig = Rig::new().await;
    let own = rig.seed_right(1, "claim_payment", &["claim_payment"]).await;
    let other = rig.seed_right(2, "continue_order", &["continue_order"]).await;
    let (session, version) = session_for(&rig, &own).await;
    let command = |right: &SeededRight, action: RightAction| ClaimRightCommand {
        command_id: CommandId::from_uuid(Uuid::new_v4()),
        right_id: right.right_id,
        session,
        browser: browser(7),
        expected_version: version,
        action,
        request_hash: RequestHash::new([9; 32]),
    };
    // 正例：只读到自己的权利。
    let view = rig.store.current(session, browser(7)).await.expect("读取").expect("有会话");
    assert_eq!(view.right_id, own.right_id);
    // 反例：持本会话办理他人权利、读他人权利，都按未认证失败，账本与订单不留任何痕迹。
    let snapshot =
        "SELECT count(*) FROM public.consignment_commands WHERE operation = 'claim_right'";
    let before = rig.count(snapshot).await;
    assert_eq!(
        rig.store.claim(command(&other, RightAction::ContinueOrder)).await.err(),
        Some(RightsStoreError::Unauthenticated)
    );
    assert_eq!(rig.count(snapshot).await, before);
    // 续办被受理后，订单的收件人 / 受益人列不变：会话没有任何改目的地的入口。
    let recipients = "SELECT md5(string_agg(concat_ws('|', id, recipient_user_id, recipient_game_identity), ',' ORDER BY id))
                      FROM public.settlement_obligations";
    let fingerprint: String =
        sqlx::query_scalar(recipients).fetch_one(&rig.pool).await.expect("指纹");
    let accepted = rig.store.claim(command(&own, RightAction::ClaimPayment)).await.expect("受理");
    assert_eq!(accepted.disposition, ClaimDisposition::ClaimAccepted);
    let after: String = sqlx::query_scalar(recipients).fetch_one(&rig.pool).await.expect("指纹");
    assert_eq!(after, fingerprint);
}

#[tokio::test]
async fn postgres_consignment_rights_loss_right_is_read_only() {
    let rig = Rig::new().await;
    let loss = rig.seed_right(3, "loss_case", &[]).await;
    let (session, version) = session_for(&rig, &loss).await;
    // 正例：能读到损失权利，且没有任何可执行动作。
    let view = rig.store.current(session, browser(7)).await.expect("读取").expect("有会话");
    assert_eq!(view.kind, RightKind::LossCase);
    assert!(view.allowed_actions.is_empty());
    // 反例：任何动作都被拒，受理回执不产生，权利状态不变。
    for action in [
        RightAction::ClaimEquipment,
        RightAction::ClaimPayment,
        RightAction::WithdrawHeld,
        RightAction::ContinueOrder,
    ] {
        let result = rig
            .store
            .claim(ClaimRightCommand {
                command_id: CommandId::from_uuid(Uuid::new_v4()),
                right_id: loss.right_id,
                session,
                browser: browser(7),
                expected_version: version,
                action,
                request_hash: RequestHash::new([action as u8 + 1; 32]),
            })
            .await;
        assert_eq!(result.err(), Some(RightsStoreError::ActionNotAllowed));
    }
    assert_eq!(
        rig.count(
            "SELECT count(*) FROM public.consignment_commands
             WHERE operation = 'claim_right' AND result_status < 400"
        )
        .await,
        0
    );
}

#[tokio::test]
async fn postgres_consignment_rights_refresh_grants_only_executable_actions_and_keeps_terminal_states()
 {
    let rig = Rig::new().await;
    register_derived(&rig).await;
    // 正例：买方付款腿 ready 且待办 → continue_order；卖款腿仍 blocked → 卖方无动作。
    assert_eq!(rig.store.refresh_contract_rights(uuid(CONTRACT)).await.expect("刷新"), 1);
    let actions = |kind: &'static str| {
        let pool = rig.pool.clone();
        async move {
            sqlx::query_scalar::<_, Vec<String>>(
                "SELECT s.allowed_actions FROM public.consignment_rights AS r
                 JOIN public.consignment_right_states AS s USING (right_id)
                 WHERE r.kind = $1 ORDER BY s.version DESC LIMIT 1",
            )
            .bind(kind)
            .fetch_one(&pool)
            .await
            .expect("读取动作")
        }
    };
    assert_eq!(actions("continue_order").await, vec!["continue_order".to_owned()]);
    assert!(actions("claim_payment").await.is_empty());
    // 无变化的重复刷新不追加版本。
    assert_eq!(rig.store.refresh_contract_rights(uuid(CONTRACT)).await.expect("刷新"), 0);
    // 反例：付款腿与卖款腿履约满足后两项都进入终态 fulfilled，动作清空；终态之后再刷新不改动。
    sqlx::query(
        "UPDATE public.settlement_obligation_fulfillments
         SET execution_state = 'satisfied', state_version = state_version + 1,
             completed_at = now(), updated_at = now()
         WHERE obligation_id IN ($1, $2)",
    )
    .bind(uuid(OBLIGATION_1))
    .bind(uuid(OBLIGATION_2))
    .execute(&rig.pool)
    .await
    .expect("满足履约");
    assert_eq!(rig.store.refresh_contract_rights(uuid(CONTRACT)).await.expect("刷新"), 2);
    assert!(actions("continue_order").await.is_empty());
    assert_eq!(
        rig.count("SELECT count(*) FROM public.consignment_right_states WHERE state = 'fulfilled'")
            .await,
        2
    );
    assert_eq!(rig.store.refresh_contract_rights(uuid(CONTRACT)).await.expect("刷新"), 0);
}

/// 轮询到有后端在等行锁（`wait_event_type = 'Lock'`），带上限；取代按时长睡眠判断“已阻塞”。
async fn wait_for_a_lock_wait(pool: &sqlx::PgPool) {
    for _ in 0..500 {
        let waiting: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_stat_activity
                            WHERE datname = current_database() AND wait_event_type = 'Lock'
                              AND pid <> pg_backend_pid())",
        )
        .fetch_one(pool)
        .await
        .expect("读取等待状态");
        if waiting {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("等待上限内没有后端进入锁等待");
}

async fn try_lock_user(tx: &mut sqlx::PgConnection, share: bool) -> Result<(), sqlx::Error> {
    let sql = if share {
        "SELECT 1 FROM public.users WHERE id = $1 FOR SHARE"
    } else {
        "SELECT 1 FROM public.users WHERE id = $1 FOR UPDATE"
    };
    sqlx::query(sql).bind(uuid(USER_2)).fetch_one(tx).await.map(|_| ())
}

async fn try_lock_fulfillment(tx: &mut sqlx::PgConnection) -> Result<(), sqlx::Error> {
    sqlx::query(
        "SELECT 1 FROM public.settlement_obligation_fulfillments WHERE obligation_id = $1 FOR UPDATE",
    )
    .bind(uuid(OBLIGATION_1))
    .fetch_one(tx)
    .await
    .map(|_| ())
}

#[tokio::test]
async fn postgres_consignment_entitlement_waits_for_an_uncommitted_rename_and_never_reads_the_old_name()
 {
    let rig = Rig::new().await;
    register_derived(&rig).await;
    // 改名事务已改未提交。
    let mut renamer = rig.pool.begin().await.expect("开启改名事务");
    sqlx::query("UPDATE public.users SET username = 'FakeRenamedBuyer' WHERE id = $1")
        .bind(uuid(USER_2))
        .execute(&mut *renamer)
        .await
        .expect("改名未提交");
    let pool = rig.pool.clone();
    let judge = tokio::spawn(async move {
        let mut tx = pool.begin().await.expect("开启事务");
        lock_fulfillment(&mut tx, uuid(OBLIGATION_1)).await;
        let decision = authorize_entitlement_recipient_in_tx(
            &mut tx,
            &request(OBLIGATION_1, Some(USER_2), BUYER_NAME),
        )
        .await
        .expect("核对");
        tx.commit().await.expect("提交");
        decision
    });
    // 判定必须在受益用户行锁上等待改名事务，而不是读到旧值后放行。
    wait_for_a_lock_wait(&rig.pool).await;
    assert!(!judge.is_finished(), "判定应阻塞在未提交的改名上");
    renamer.commit().await.expect("提交改名");
    assert_eq!(
        judge.await.expect("判定任务"),
        EntitlementDecision::Retain(RetainReason::IdentityContinuityUnproven)
    );
}

/// 锁序：结算路径先锁用户再锁履约行；授权点按同一顺序（先用户 FOR SHARE，再履约行，再核对）。
/// 两个事务交错也必须都完成，不能出现死锁（40P01）。
#[tokio::test]
async fn postgres_consignment_entitlement_lock_order_matches_settlement_and_never_deadlocks() {
    let rig = Rig::new().await;
    register_derived(&rig).await;
    // T1：结算顺序，先 users FOR UPDATE。
    let mut settlement = rig.pool.begin().await.expect("开启结算事务");
    try_lock_user(&mut settlement, false).await.expect("T1 锁用户");
    // T2：授权点按新约定的顺序；此刻会在用户锁上等待 T1。
    let pool = rig.pool.clone();
    let authorization = tokio::spawn(async move {
        let mut tx = pool.begin().await?;
        try_lock_user(&mut tx, true).await?;
        try_lock_fulfillment(&mut tx).await?;
        let decision = authorize_entitlement_recipient_in_tx(
            &mut tx,
            &request(OBLIGATION_1, Some(USER_2), BUYER_NAME),
        )
        .await?;
        tx.commit().await?;
        Ok::<_, sqlx::Error>(decision)
    });
    wait_for_a_lock_wait(&rig.pool).await;
    // T1 继续按结算顺序锁履约行，随后提交；两边都必须完成。
    let settled = try_lock_fulfillment(&mut settlement).await;
    settlement.commit().await.expect("T1 提交");
    assert!(settled.is_ok(), "结算事务不得被死锁中止：{settled:?}");
    assert_eq!(
        authorization.await.expect("T2 任务").expect("授权事务不得被死锁中止"),
        EntitlementDecision::Authorized
    );
}
