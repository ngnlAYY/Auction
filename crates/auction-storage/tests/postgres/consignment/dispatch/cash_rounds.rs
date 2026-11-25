//! 派发轮现金腿的资金闭环：退款腿起写同事务占用 P 并在 Sent 后结清；未付载体 RTS 精确 Take 后回补原
//! carrier 与原自有、记 `rts_principal`、解除 UID 阻断，之后同合同的载体新轮可再预留与起写（D9 §4.2–4.4）。
//!
//! 只测核心规则：金额与资金、命令幂等（重放无新增）、失败关闭、故障注入下的原子性。

use std::sync::Arc;

use auction_application::consignment::{
    AcquireReceipt, ConsignmentError, ReturnService, StartTakeInput, WriteStart,
};
use auction_application::delivery::DeliverySendCredentialBinding;
use auction_domain::SettlementWorld;
use auction_domain::consignment::dispatch::{
    AttemptKey, DispatchRoundKey, EvidenceCategory, FrozenDispatchIdentity, dispatch_ref,
    dispatch_round_id,
};
use auction_domain::consignment::returns::{ReturnIntakeFacts, ReturnLeg};
use auction_time::Timestamp;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::contracts::fixture::{CONTRACT, OBLIGATION_1, OBLIGATION_4};
use super::super::deposit::support::{CREDENTIAL, SESSION, succeeds};
use super::super::ledger136::fixture::{PRICE, PURCHASED_AT};
use super::super::returns::fixture::{
    Claim, MANIFEST, Rts, SIGNED_VALIDATOR, consume_input, service as return_service, take_delivery,
};
use super::carrier_funding::{
    BALANCE, Carrier, Mode, OWN, UID, carrier, carrier_frozen, cash_evidence,
    clear_order_event_failure, entries, fresh_evidence, inject_order_event_failure, ledger, own,
};
use super::fixture::{BOT_UID, at, binding, uuid};

/// 载体首派在 main 的 Sent MID。
const CARRIER_MID: &str = "9101";
/// 退件之后改用的 main 凭据与证据会话（同类 cookie 全库只许一个 active）。
const MAIN_CREDENTIAL: &str = "80000000-0000-0000-0000-0000000000c5";
const MAIN_SESSION: &str = "c4000000-0000-4000-8000-0000000000c5";

/// 一个额外派发轮的冻结身份：main 派发、无附件。
pub(in crate::consignment) fn frozen(
    key: &DispatchRoundKey,
    template: &str,
    credits: i64,
    cod: i64,
) -> FrozenDispatchIdentity {
    FrozenDispatchIdentity {
        rights_holder_identity: "FakeSellerTwo".to_owned(),
        bot_uid: BOT_UID.to_owned(),
        world: SettlementWorld::Main,
        subject: "fake subject".to_owned(),
        template: template.to_owned(),
        body_digest: [0xd2; 32],
        round_ref: dispatch_ref(key),
        d0: at(PURCHASED_AT),
        credits,
        cod,
        equipment_ids: Vec::new(),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// 与生产轮同形的 prepared 投递与轮行（收件人 `recipient`）；`authorized` 给出新轮引用的上一轮
/// (intake, consumption)。
pub(in crate::consignment) async fn insert_round(
    pool: &PgPool,
    key: &DispatchRoundKey,
    frozen: &FrozenDispatchIdentity,
    recipient: &str,
    authorized: Option<(Uuid, Uuid)>,
) -> Uuid {
    let delivery = Uuid::new_v4();
    let round_id = dispatch_round_id(key);
    let digest = frozen.identity_digest();
    let payload = serde_json::json!({
        "operation": "send",
        "world": "main",
        "template": frozen.template,
        "subject": frozen.subject,
        "credits": frozen.credits,
        "cod": frozen.cod,
        "equipment_ids": [],
        "round_ref": frozen.round_ref,
        "identity_digest": hex(&digest),
    });
    let (prior, intake, consumption) = match authorized {
        Some((intake, consumption)) => {
            (format!("{}", key.round - 1), format!("'{intake}'"), format!("'{consumption}'"))
        }
        None => ("NULL".to_owned(), "NULL".to_owned(), "NULL".to_owned()),
    };
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.deliveries
                (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status)
             VALUES ('{delivery}', gen_random_uuid(), decode('{digest_hex}', 'hex'), 'settlement',
                     '{obligation}', '{recipient}', '{payload}'::jsonb, 'prepared');
             INSERT INTO public.consignment_dispatch_rounds
                (contract_id, obligation_id, round, round_id, delivery_id, rights_holder_identity,
                 bot_uid, world_id, subject, template, body_digest, round_ref, d0, credits, cod,
                 prior_round, authorized_by_intake, authorized_by_consumption)
             VALUES ('{contract}', '{obligation}', {round}, '{round_id}', '{delivery}',
                     '{recipient}', '{BOT_UID}', 'main', '{subject}', '{template}',
                     decode('{body}', 'hex'), '{round_id}', '{PURCHASED_AT}', {credits}, {cod},
                     {prior}, {intake}, {consumption});",
            digest_hex = hex(&digest),
            obligation = key.obligation_id,
            contract = key.contract_id,
            round = key.round,
            subject = frozen.subject,
            template = frozen.template,
            body = hex(&frozen.body_digest),
            credits = frozen.credits,
            cod = frozen.cod,
        ),
    )
    .await;
    delivery
}

/// 以 SQL 预置下游义务已放行：义务 `ready`、或有标记清除、前置履约已满足（起写复核要求它，PIWP）。本文件只测
/// 现金腿的资金闭环；放行本身经真实写端（`prove_handoff`、`convert_refund`）的用例见 `payout/`。
pub(in crate::consignment) async fn release_for_dispatch(
    pool: &PgPool,
    obligation: &str,
    prerequisite: &str,
) {
    succeeds(
        pool,
        &format!(
            "UPDATE public.settlement_obligations
             SET business_state = 'ready', state_version = state_version + 1
             WHERE id = '{obligation}' AND business_state <> 'ready';
             UPDATE public.settlement_obligation_fulfillments SET dormant = false
             WHERE obligation_id = '{obligation}' AND dormant;
             UPDATE public.settlement_obligation_fulfillments
             SET execution_state = 'satisfied', completed_at = transaction_timestamp(),
                 claim_token = NULL, claimed_at = NULL, claim_expires_at = NULL
             WHERE obligation_id = '{prerequisite}' AND execution_state <> 'satisfied';"
        ),
    )
    .await;
}

/// 激活并领取任一轮投递。
pub(in crate::consignment) async fn claim_delivery(pool: &PgPool, delivery: Uuid, token: Uuid) {
    succeeds(
        pool,
        &format!(
            "UPDATE public.deliveries
             SET status = 'claimed', activation_command_id = gen_random_uuid(),
                 activation_request_digest = decode(repeat('ab', 32), 'hex'),
                 activated_at = statement_timestamp(), lease_token = '{token}',
                 lease_expires_at = clock_timestamp() + interval '5 minutes',
                 fencing_version = 1, attempt_count = 1, claimed_at = statement_timestamp()
             WHERE id = '{delivery}' AND status = 'prepared';"
        ),
    )
    .await;
}

/// 领取 fence 1、登记尝试并带同会话的新鲜资金证据起写该轮。
pub(super) async fn start_round(
    seeded: &Carrier,
    key: DispatchRoundKey,
    frozen: FrozenDispatchIdentity,
    delivery: Uuid,
    credential: DeliverySendCredentialBinding,
) -> (AttemptKey, Result<WriteStart, ConsignmentError>) {
    let token = Uuid::new_v4();
    claim_delivery(seeded.pool(), delivery, token).await;
    let attempt = AttemptKey { delivery_id: delivery, fence: 1 };
    let service = seeded.service();
    service.start_attempt(key, attempt, token, frozen).await.expect("登记尝试");
    let evidence = cash_evidence(
        seeded.pool(),
        attempt,
        token,
        &credential,
        UID,
        Some(BALANCE),
        Timestamp::now(),
    )
    .await;
    (attempt, service.mark_cash_write_started(attempt, token, Some(credential), &evidence).await)
}

pub(in crate::consignment) fn sent(
    attempt: AttemptKey,
    mid: &str,
) -> auction_application::consignment::RecordEvidenceInput {
    auction_application::consignment::RecordEvidenceInput {
        attempt,
        category: EvidenceCategory::SentBinding,
        remote_observation_id: mid.to_owned(),
        observed_at: at("2026-01-01T05:00:00Z"),
        validator_version: "fake-validator-v1".to_owned(),
        evidence_digest: [7; 32],
    }
}

/// 用途分配的 (状态, 金额, 绑定投递)。
pub(super) async fn allocation(
    pool: &PgPool,
    purpose: &str,
) -> Option<(String, i64, Option<Uuid>)> {
    sqlx::query_as(
        "SELECT state, amount, delivery_id FROM public.settlement_funding_allocations
         WHERE uid = $1 AND purpose = $2",
    )
    .bind(UID)
    .bind(purpose)
    .fetch_optional(pool)
    .await
    .expect("用途分配")
}

async fn count(pool: &PgPool, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.expect("计数")
}

#[tokio::test]
async fn postgres_consignment_dispatch_refund_round_reserves_and_spends_p() {
    let seeded = Carrier::purchased(Mode::Strict { own: OWN }).await;
    let pool = seeded.pool();
    let key = DispatchRoundKey {
        contract_id: uuid(CONTRACT),
        obligation_id: uuid(OBLIGATION_4),
        round: 1,
    };
    let refund = frozen(&key, "consignment_refund", PRICE, 0);
    let delivery = insert_round(pool, &key, &refund, "FakeSellerTwo", None).await;
    release_for_dispatch(pool, OBLIGATION_4, OBLIGATION_1).await;

    // 反例：退款用途里还没有 P（退款未转换），起写整笔回滚，零许可、零起写、资金零痕迹。
    let before = ledger(pool).await;
    let (attempt, denied) = start_round(&seeded, key, refund.clone(), delivery, binding()).await;
    assert_eq!(denied.expect_err("退款未到账"), ConsignmentError::Unavailable);
    assert_eq!(ledger(pool).await, before);
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.delivery_send_attempts").await,
        0,
        "零 POST 许可"
    );

    // 退款转换后用途里有 P。故障注入：订单阶段事件失败，占用随起写一并回滚。
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.settlement_funding_allocations
                (id, uid, purpose, obligation_id, consignment_contract_id, amount)
             VALUES (gen_random_uuid(), '{UID}', 'refund', '{OBLIGATION_4}', '{CONTRACT}', {PRICE});"
        ),
    )
    .await;
    let token: Uuid = sqlx::query_scalar("SELECT lease_token FROM public.deliveries WHERE id = $1")
        .bind(delivery)
        .fetch_one(pool)
        .await
        .expect("租约");
    let service = seeded.service();
    let funded = ledger(pool).await;
    let evidence = fresh_evidence(pool, seeded.mode, attempt, token).await;
    inject_order_event_failure(pool).await;
    assert!(
        service.mark_cash_write_started(attempt, token, Some(binding()), &evidence).await.is_err()
    );
    clear_order_event_failure(pool).await;
    assert_eq!(ledger(pool).await, funded);
    assert_eq!(allocation(pool, "refund").await, Some(("available".to_owned(), PRICE, None)));

    // 正例：起写同事务 reserve → in_flight，占用绑定该投递；Sent 绑定后转 spent，重放不重复。
    let started = service.mark_cash_write_started(attempt, token, Some(binding()), &evidence).await;
    assert!(matches!(started, Ok(WriteStart::FirstCommitted { .. })), "{started:?}");
    assert_eq!(
        allocation(pool, "refund").await,
        Some(("in_flight".to_owned(), PRICE, Some(delivery)))
    );
    assert!(!service.record_evidence(sent(attempt, "9104")).await.expect("Sent").replayed);
    assert_eq!(allocation(pool, "refund").await, Some(("spent".to_owned(), PRICE, Some(delivery))));
    let settled = ledger(pool).await;
    assert!(service.record_evidence(sent(attempt, "9104")).await.expect("重放").replayed);
    assert_eq!(ledger(pool).await, settled);
    assert_eq!(own(pool).await, OWN, "退款腿不动原 UID 自有");
}

/// 退件之后的凭据轮换：停用旧凭据，启用 main 凭据与证据会话，供 RTS 原信、Take 与新轮起写使用。
async fn switch_to_main_mail(pool: &PgPool) {
    succeeds(
        pool,
        &format!(
            "UPDATE public.credential_keys SET status = 'disabled', updated_at = transaction_timestamp()
             WHERE id = '{CREDENTIAL}';
             INSERT INTO public.credential_keys
                (id, credential_kind, scope, label, ciphertext, nonce, encryption_key_id,
                 fingerprint, status, state_version, created_at, updated_at)
             VALUES ('{MAIN_CREDENTIAL}', 'mail_cookie', 'main', 'fake-main', decode('11', 'hex'),
                     decode(repeat('22', 12), 'hex'), 'fake-data-key', repeat('c', 64),
                     'active', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
             INSERT INTO public.mail_evidence_sessions
                (id, bot_uid, world_id, player_name, credential_id, credential_version,
                 runtime_generation, observed_at, protocol_version, binding_digest)
             VALUES ('{MAIN_SESSION}', '{BOT_UID}', 'main', 'FakeBot', '{MAIN_CREDENTIAL}', 1, 1,
                     '2026-01-01T00:00:00Z', 'hv_uid_forum_profile_armory_v1',
                     decode(repeat('35', 32), 'hex'));"
        ),
    )
    .await;
}

/// 载体 RTS 的接管事实：原载体首派、精确清单、main 证据会话；腿由存储从原尝试推出，这里给出同一结论。
fn carrier_rts_facts(rts: Rts, delivery: Uuid) -> ReturnIntakeFacts {
    ReturnIntakeFacts {
        k: rts.key(),
        contract_id: uuid(CONTRACT),
        obligation_id: uuid(OBLIGATION_1),
        original_round: 1,
        original_attempt: AttemptKey { delivery_id: delivery, fence: 1 },
        sent_mid: CARRIER_MID.to_owned(),
        rts_observation_id: rts.observation(),
        rts_returned_at: Timestamp::UNIX_EPOCH,
        leg: ReturnLeg::CashPrincipal,
        frozen_manifest_digest: MANIFEST,
        session_id: uuid(MAIN_SESSION),
    }
}

/// 领取 Take 投递并登记 Take 尝试（main 凭据与会话）。
async fn claim_take(pool: &PgPool, work: Uuid, delivery: Uuid, claim: Claim) {
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.mail_disposition_attempts
                (id, work_id, delivery_id, fence, credential_id, credential_version,
                 runtime_generation, session_id, expected_digest, prepared_at)
             VALUES ('{attempt}', '{work}', '{delivery}', {fence}, '{MAIN_CREDENTIAL}', 1, 1,
                     '{MAIN_SESSION}', decode(repeat('e1', 32), 'hex'), '2026-01-01T05:00:00Z');
             UPDATE public.deliveries
             SET status = 'claimed', lease_token = '{token}',
                 lease_expires_at = timestamptz '2099-01-01T00:00:00Z',
                 fencing_version = {fence}, attempt_count = {fence}
             WHERE id = '{delivery}';",
            attempt = claim.attempt,
            fence = claim.fence,
            token = claim.token,
        ),
    )
    .await;
}

/// (未解除阻断数, 资金回补数, return 分录 (条数, 金额), rts_principal 结论数, 消费数)。
async fn return_effects(pool: &PgPool) -> (i64, i64, (i64, i64), i64, i64) {
    (
        count(
            pool,
            "SELECT count(*) FROM public.consignment_cash_return_blocks WHERE state = 'open'",
        )
        .await,
        count(pool, "SELECT count(*) FROM public.consignment_funding_returns").await,
        entries(pool, "return").await,
        count(
            pool,
            "SELECT count(*) FROM public.consignment_principal_facts
             WHERE conclusion = 'rts_principal'",
        )
        .await,
        count(pool, "SELECT count(*) FROM public.consignment_return_consumptions").await,
    )
}

/// 载体首派已起写并绑定 Sent、被买家原样退回、经真实接管入口接管并已起写 Take：UID 阻断已建、尚未消费。
async fn carrier_rts_take_started(seeded: &Carrier) -> (ReturnService, AcquireReceipt, Claim, Rts) {
    let pool = seeded.pool();
    let (attempt, started) = seeded.start().await;
    assert!(matches!(started, Ok(WriteStart::FirstCommitted { .. })), "{started:?}");
    seeded.service().record_evidence(sent(attempt, CARRIER_MID)).await.expect("Sent 证据");

    // 买家拒收：载体 1 Credit + COD P+1 原样退回 main Bot。
    switch_to_main_mail(pool).await;
    let rts = Rts { n: 1 };
    // RTS 必须晚于原 Send：收件分钟取载体冻结 Send 时刻之后的下一分钟。
    let minute: String = sqlx::query_scalar(
        "SELECT to_char(date_trunc('minute', sending_at AT TIME ZONE 'UTC') + interval '1 minute',
                        'YYYY-MM-DD\"T\"HH24:MI')
         FROM public.delivery_send_attempts WHERE delivery_id = $1",
    )
    .bind(seeded.delivery_id)
    .fetch_one(pool)
    .await
    .expect("载体 Send 时刻");
    let rts_sql = rts
        .sql(1, PRICE + 1)
        .replace(SESSION, MAIN_SESSION)
        .replace("2026-01-01T05:00:00Z", &format!("{minute}:00Z"))
        .replace("2026-01-01T05:00", &minute);
    succeeds(pool, &rts_sql).await;
    let store = Arc::new(seeded.runtime.clone());
    let returns: ReturnService = return_service(&store);
    let receipt =
        returns.acquire(carrier_rts_facts(rts, seeded.delivery_id)).await.expect("接管载体退件");
    let leg: String =
        sqlx::query_scalar("SELECT leg FROM public.consignment_return_intakes WHERE id = $1")
            .bind(receipt.intake_id)
            .fetch_one(pool)
            .await
            .expect("接管腿");
    assert_eq!(leg, "cash_principal", "载体退件是现金本金腿，不是装备腿");

    // Take 起写同事务建立该 UID 的 opening 阻断（本金 1、原载体投递）。
    let claim = Claim::new(31, 1, true);
    let take = take_delivery(pool, &receipt).await;
    claim_take(pool, receipt.work_id, take, claim).await;
    let binding_digest: Vec<u8> = sqlx::query_scalar(
        "SELECT binding_digest FROM public.mail_evidence_sessions WHERE id = $1",
    )
    .bind(uuid(MAIN_SESSION))
    .fetch_one(pool)
    .await
    .expect("会话绑定");
    let take_started = returns
        .start_take(StartTakeInput {
            intake_id: receipt.intake_id,
            work_id: receipt.work_id,
            delivery_id: take,
            attempt_id: claim.attempt,
            fence: claim.fence,
            claim_token: claim.token,
            session_binding_digest: <[u8; 32]>::try_from(binding_digest.as_slice()).expect("摘要"),
            expected_manifest_digest: MANIFEST,
        })
        .await
        .expect("Take 起写");
    assert!(matches!(take_started, WriteStart::FirstCommitted { .. }));
    let block: (String, i64, Uuid) = sqlx::query_as(
        "SELECT leg, amount, original_delivery_id FROM public.consignment_cash_return_blocks
         WHERE uid = $1 AND state = 'open'",
    )
    .bind(UID)
    .fetch_one(pool)
    .await
    .expect("UID 阻断");
    assert_eq!(block, ("cash_principal".to_owned(), 1, seeded.delivery_id));
    assert_eq!(return_effects(pool).await, (1, 0, (0, 0), 0, 0));
    (returns, receipt, claim, rts)
}

#[tokio::test]
async fn postgres_consignment_dispatch_unpaid_carrier_rts_restores_principal_and_allows_next_round()
{
    let seeded = Carrier::purchased(Mode::Strict { own: OWN }).await;
    let pool = seeded.pool();
    let (returns, receipt, claim, rts) = carrier_rts_take_started(&seeded).await;
    assert_eq!(own(pool).await, OWN - 1);

    // 精确 Take 1：returned → 原 carrier → 原自有，记 rts_principal，解除阻断。
    let input = consume_input(&receipt, rts, claim, SIGNED_VALIDATOR, 0xa7);
    assert!(!returns.consume(input.clone()).await.expect("消费").replayed);
    assert_eq!(return_effects(pool).await, (0, 1, (1, 1), 1, 1));
    assert_eq!(own(pool).await, OWN, "严格现金恢复原自有");
    assert_eq!(carrier(pool).await, Some(("available".to_owned(), 0, None)));
    // 重放无第二次恢复。
    let after = ledger(pool).await;
    assert!(returns.consume(input).await.expect("重放").replayed);
    assert_eq!(ledger(pool).await, after);
    assert_eq!(return_effects(pool).await, (0, 1, (1, 1), 1, 1));

    // 同合同的载体新轮（引用已封闭上一轮的接管与消费）可再预留并取得唯一起写许可。
    let consumption: Uuid = sqlx::query_scalar(
        "SELECT id FROM public.consignment_return_consumptions WHERE intake_id = $1",
    )
    .bind(receipt.intake_id)
    .fetch_one(pool)
    .await
    .expect("消费行");
    let key = DispatchRoundKey {
        contract_id: uuid(CONTRACT),
        obligation_id: uuid(OBLIGATION_1),
        round: 2,
    };
    let next = FrozenDispatchIdentity { round_ref: dispatch_ref(&key), ..carrier_frozen() };
    let delivery =
        insert_round(pool, &key, &next, "FakeSellerTwo", Some((receipt.intake_id, consumption)))
            .await;
    succeeds(
        pool,
        &format!(
            "UPDATE public.credential_keys SET scope = 'mooglemail:main' WHERE id = '{MAIN_CREDENTIAL}';"
        ),
    )
    .await;
    let main_binding = DeliverySendCredentialBinding {
        credential_id: uuid(MAIN_CREDENTIAL),
        credential_version: 1,
        runtime_generation: 1,
    };
    let (_attempt, again) = start_round(&seeded, key, next, delivery, main_binding).await;
    assert!(matches!(again, Ok(WriteStart::FirstCommitted { .. })), "{again:?}");
    assert_eq!(carrier(pool).await, Some(("in_flight".to_owned(), 1, Some(delivery))));
    assert_eq!(own(pool).await, OWN - 1);
}

/// 核销子账的 (状态, 金额, 是否绑定本合同)；不存在为 `None`。
async fn writeoff(pool: &PgPool) -> Option<(String, i64, bool)> {
    sqlx::query_as(
        "SELECT state, amount, consignment_contract_id = $2 FROM public.settlement_funding_allocations
         WHERE uid = $1 AND purpose = 'carrier_writeoff'",
    )
    .bind(UID)
    .bind(uuid(CONTRACT))
    .fetch_optional(pool)
    .await
    .expect("核销子账")
}

/// record_only 的未付载体 RTS（D9 §4.4 表“未付载体 RTS”的 record_only 列）：精确 Take 1 后 returned 来源回到原受限
/// carrier，再转入同合同不可支用的核销子账，不进任何自有、不形成销售收入；Take 消费之前本金回补为 0。
/// 同时按现状断言 isekai 接管只得待判返回事实（S2b：可交付性无可读事实），零拒收、零处罚。
///
/// 保护不变量 2、4、6：本金只随精确 Take 的消费同事务结清一次，重放不产生第二次回补。
#[tokio::test]
async fn postgres_consignment_dispatch_record_only_carrier_rts_writes_principal_off() {
    let seeded = Carrier::purchased(Mode::RecordOnly).await;
    let pool = seeded.pool();
    let (returns, receipt, claim, rts) = carrier_rts_take_started(&seeded).await;
    let verdict: String = sqlx::query_scalar(
        "SELECT verdict.verdict || ':' || verdict.reason
         FROM public.consignment_return_fact_verdicts AS verdict
         JOIN public.consignment_return_facts AS fact ON fact.return_fact_id = verdict.return_fact_id
         WHERE fact.intake_id = $1",
    )
    .bind(receipt.intake_id)
    .fetch_one(pool)
    .await
    .expect("接管同提的返回事实");
    assert_eq!(verdict, "pending:deliverability_disputed");
    assert_eq!(count(pool, "SELECT count(*) FROM public.consignment_refusals").await, 0);
    assert_eq!(count(pool, "SELECT count(*) FROM public.consignment_sanctions").await, 0);

    // 反例：Take 已起写、未证精确 Take：本金回补 0，核销子账不存在，阻断仍在。
    assert_eq!(return_effects(pool).await, (1, 0, (0, 0), 0, 0));
    assert_eq!(writeoff(pool).await, None);

    // 正例：精确 Take 1 → returned 来源 → 原 carrier → 同合同核销子账；自有为零，账户里没有任何自有分配。
    let input = consume_input(&receipt, rts, claim, SIGNED_VALIDATOR, 0xa8);
    assert!(!returns.consume(input.clone()).await.expect("消费").replayed);
    assert_eq!(return_effects(pool).await, (0, 1, (1, 1), 1, 1));
    assert_eq!(writeoff(pool).await, Some(("available".to_owned(), 1, true)));
    assert_eq!(carrier(pool).await, Some(("available".to_owned(), 0, None)));
    assert_eq!(own(pool).await, 0);
    assert_eq!(
        count(
            pool,
            "SELECT count(*) FROM public.consignment_principal_facts
             WHERE conclusion = 'rts_principal' AND mode = 'record_only' AND amount = 1"
        )
        .await,
        1
    );
    // 重放：不产生第二次回补或核销。
    let after = ledger(pool).await;
    assert!(returns.consume(input).await.expect("重放").replayed);
    assert_eq!(ledger(pool).await, after);
    assert_eq!(writeoff(pool).await, Some(("available".to_owned(), 1, true)));
}
