//! 退件接管夹具：现金退款腿的 RTS、Take 领取与存量构造器。
//!
//! 交给第 7 期复用：`seed_started_unproven`（已起写、证明未提交，现金阻断在）与 `seed_consumed`
//! （已消费，资金已回补、阻断已解除）。

use std::sync::Arc;

use auction_application::consignment::{
    AcquireReceipt, ConsignmentError, ConsumeReturnInput, ReturnIntakeStore, ReturnService,
    StartTakeInput, WriteStart,
};
use auction_application::settlement::{CashLeg, CashPhase, SettleCash};
use auction_domain::SettlementWorld;
use auction_domain::consignment::OriginalMailKey;
use auction_domain::consignment::dispatch::AttemptKey;
use auction_domain::consignment::returns::{ReturnIntakeFacts, ReturnLeg};
use auction_storage::DatabaseStore;
use auction_time::Timestamp;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::contracts::fixture::{CONTRACT, OBLIGATION_1, OBLIGATION_2, OBLIGATION_3};
use super::super::deposit::support::{CREDENTIAL, LISTING, POSITION, SESSION, succeeds};
use super::super::funding::fixture::{id, mutation, seed_refund_spent};
use super::super::ledger136::fixture::{
    AVAILABLE_COMMAND, PAY_DELIVERY, PAY_MID, PRICE, PURCHASED_AT, REFUND_DELIVERY, REFUND_MID,
    SOLD_AT, original_dispatch_sql, round_sql, seed_purchased, send_sql,
};
use super::super::recovery::sale_recovery::refund_call::{
    PAYOUT_ATTEMPT, PAYOUT_DELIVERY, PAYOUT_MID, PAYOUT_ROUND, fund_the_sale,
};

/// 已签收验证器版本；与存储端 `test-db` 构建认的版本一致。
pub const SIGNED_VALIDATOR: &str = "test-signed-off-validator-v1";
/// 未签收验证器版本（生产唯一的现状）。
pub const UNSIGNED_VALIDATOR: &str = "mooglemail-consignment-disposition-v1-unsigned";
/// RTS 观察的清单摘要，也是 Take 工作冻结的清单摘要。
pub const MANIFEST: [u8; 32] = [0xe1; 32];

/// 一封 RTS：n 区分不同来信（MID 600 + n）。
#[derive(Debug, Clone, Copy)]
pub struct Rts {
    pub n: u32,
}

impl Rts {
    pub fn mid(&self) -> String {
        (600 + self.n).to_string()
    }

    pub fn observation(&self) -> Uuid {
        Uuid::from_u128(0xc7f0_0000_0000_4000_8000_0000_0000_0000 + u128::from(self.n))
    }

    pub fn key(&self) -> OriginalMailKey {
        OriginalMailKey::new("12345", SettlementWorld::Main, &self.mid()).expect("规范原信键")
    }

    /// 系统 RTS 的原信与观察行；Credits、COD 取与首派冻结身份一致的值，装备为空。
    pub fn sql(&self, credits: i64, cod: i64) -> String {
        let (mid, obs) = (self.mid(), self.observation());
        format!(
            "INSERT INTO public.mail_originals
                (bot_uid, world_id, external_mid, first_session_id, received_minute_raw,
                 received_timezone, first_seen_at, season_window_id)
             VALUES ('12345', 'main', '{mid}', '{SESSION}', '2026-01-01T05:00', 'UTC',
                     '{SOLD_AT}', NULL);
             INSERT INTO public.mail_envelope_observations
                (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
                 mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
                 evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
                 returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
                 season_window_id)
             VALUES ('{obs}', '12345', 'main', '{mid}', 'obs-{mid}', '{SESSION}',
                     'fake-mailbox', NULL, NULL, '{SOLD_AT}', 'parser-v1',
                     NULL, decode(repeat('66', 32), 'hex'), decode(repeat('67', 32), 'hex'),
                     decode(repeat('e1', 32), 'hex'), 'MoogleMail', 'FakeBot',
                     true, true, {credits}, {cod}, NULL, '[]'::jsonb,
                     '{{\"subject_is_consign\":true,\"sender_identity\":\"verified\",
                       \"list_detail_consistent\":true,\"duplicate_raw_eid\":false,
                       \"canonical_alias_ambiguous\":false,\"unknown_attachment\":false,
                       \"system_rts\":true,\"legacy_auction_owner\":null,
                       \"received_minute_raw\":\"2026-01-01T05:00\",\"items\":[]}}'::jsonb,
                     NULL);"
        )
    }

    /// 应用层判定的接管事实；存储在锁内只用它定位，其余全部重读。
    pub fn facts(&self, obligation: &str) -> ReturnIntakeFacts {
        ReturnIntakeFacts {
            k: self.key(),
            contract_id: Uuid::parse_str(CONTRACT).expect("合同"),
            obligation_id: Uuid::parse_str(obligation).expect("义务"),
            original_round: 1,
            original_attempt: AttemptKey {
                delivery_id: Uuid::parse_str(REFUND_DELIVERY).expect("投递"),
                fence: 1,
            },
            sent_mid: REFUND_MID.to_owned(),
            rts_observation_id: self.observation(),
            rts_returned_at: Timestamp::UNIX_EPOCH,
            leg: ReturnLeg::CashRefund,
            frozen_manifest_digest: MANIFEST,
            session_id: Uuid::parse_str(SESSION).expect("会话"),
        }
    }
}

/// 现金退款腿已支出（原支出 P），并有一封与首派冻结身份一致的 RTS。
pub async fn seed_refund_rts(pool: &PgPool, store: &DatabaseStore, rts: Rts) {
    seed_refund_spent(pool, store).await;
    // 退款首派的冻结身份是 Credits 0、COD 0、无装备（夹具的 Send 行），RTS 照此一致。
    succeeds(pool, &rts.sql(0, 0)).await;
}

/// 原件腿的 RTS 事实：原付款义务、原付款 Send 与原件腿。
pub fn original_facts(rts: Rts) -> ReturnIntakeFacts {
    let mut facts = rts.facts(OBLIGATION_1);
    facts.original_attempt.delivery_id = Uuid::parse_str(PAY_DELIVERY).expect("投递");
    facts.sent_mid = PAY_MID.to_owned();
    facts.leg = ReturnLeg::Original;
    facts
}

/// 卖款 S（成交图 P=101、F=5）。
pub const PAYOUT: i64 = 96;

/// 卖款腿的 RTS 事实：卖款义务、卖款 Send 与现金卖款腿。
pub fn payout_facts(rts: Rts) -> ReturnIntakeFacts {
    let mut facts = rts.facts(OBLIGATION_2);
    facts.original_attempt.delivery_id = Uuid::parse_str(PAYOUT_DELIVERY).expect("投递");
    facts.sent_mid = PAYOUT_MID.to_owned();
    facts.leg = ReturnLeg::CashPayout;
    facts
}

/// 卖款腿的一次现金状态迁移（命令 ID 与摘要由 `byte` 决定）。
fn payout_cash(byte: u8, phase: CashPhase) -> SettleCash {
    SettleCash {
        mutation: mutation(byte),
        contract_id: id(CONTRACT),
        obligation_id: id(OBLIGATION_2),
        delivery_id: id(PAYOUT_DELIVERY),
        leg: CashLeg::Seller,
        phase,
        amount: PAYOUT,
    }
}

/// 卖款 S 已支出：已收款成交（卖款分配 S 可用）、卖款首派（轮的 Credits 为 S）与真实写端的
/// reserve → in_flight → spent；并有一封与卖款 Send 冻结身份一致的 RTS（Credits 0、COD 0，同退款腿）。
pub async fn seed_payout_rts(pool: &PgPool, store: &DatabaseStore, rts: Rts) {
    seed_purchased(pool).await;
    succeeds(pool, &original_dispatch_sql()).await;
    fund_the_sale(store).await;
    let round =
        round_sql(PAYOUT_ROUND, PAYOUT_DELIVERY, OBLIGATION_2, PAYOUT_ATTEMPT, "cash_payout");
    let payout_round = round.replace(
        &format!("'{PURCHASED_AT}', 0, {PRICE},"),
        &format!("'{PURCHASED_AT}', {PAYOUT}, 0,"),
    );
    assert_ne!(payout_round, round, "夹具应把轮的 Credits 改为卖款 S");
    // 投递载荷按生产首派的形状带 credits / cod（预览据此判定资产有界）。
    let send = send_sql(PAYOUT_DELIVERY, OBLIGATION_2, PAYOUT_MID, 0).replace(
        "'{}'::jsonb",
        &format!(
            "'{{\"operation\":\"send\",\"world\":\"main\",\"credits\":{PAYOUT},\"cod\":0}}'::jsonb"
        ),
    );
    succeeds(pool, &(send + &payout_round)).await;
    for (byte, phase) in
        [(41, CashPhase::Reserve), (42, CashPhase::InFlight), (43, CashPhase::Spent)]
    {
        store.test_settle_cash(payout_cash(byte, phase)).await.expect("卖款腿状态迁移");
    }
    // spent 只随已证外达：生产里同一 Sent 绑定把轮投递收口为 succeeded，夹具照此补齐投递终态。
    succeeds(
        pool,
        &format!(
            "UPDATE public.deliveries
             SET status = 'succeeded', fencing_version = 1, attempt_count = 1,
                 sending_at = '{PURCHASED_AT}', completed_at = '{SOLD_AT}',
                 external_reference = 'mooglemail:main:send:{PAYOUT_MID}', updated_at = '{SOLD_AT}'
             WHERE id = '{PAYOUT_DELIVERY}';"
        ),
    )
    .await;
    succeeds(pool, &rts.sql(0, 0)).await;
}

/// 卖款腿：已接管、Take 已起写（现金阻断已建立），证明尚未提交。
pub async fn seed_payout_started(
    pool: &PgPool,
    store: &Arc<DatabaseStore>,
    rts: Rts,
) -> (AcquireReceipt, Claim) {
    seed_payout_rts(pool, store, rts).await;
    let service = service(store);
    let receipt = service.acquire(payout_facts(rts)).await.expect("接管");
    let claim = Claim::new(21, 1, true);
    claim_take(pool, &receipt, take_delivery(pool, &receipt).await, claim).await;
    let started = start(&service, pool, &receipt, claim).await.expect("起写");
    assert!(matches!(started, WriteStart::FirstCommitted { .. }), "首次起写得到许可");
    (receipt, claim)
}

/// position committed -> in_transit（版本 4）加同版本事件；须与原件腿首派同事务。
fn in_transit_sql() -> String {
    format!(
        "UPDATE public.consignment_custody_positions
         SET state = 'in_transit', version = 4, updated_at = '{PURCHASED_AT}'
         WHERE id = '{POSITION}';
         INSERT INTO public.consignment_custody_events
            (position_id, version, command_id, event_kind, from_state, to_state, proof_id,
             listing_id, listing_version, occurred_at)
         VALUES ('{POSITION}', 4, '{AVAILABLE_COMMAND}', 'in_transit', 'committed',
                 'in_transit', NULL, '{LISTING}', 6, '{PURCHASED_AT}');"
    )
}

/// 原件腿已派发（position 在 in_transit），并有一封与冻结身份一致的 RTS（Credits 0、COD 为价格）。
pub async fn seed_original_rts(pool: &PgPool, rts: Rts) {
    seed_purchased(pool).await;
    succeeds(pool, &(original_dispatch_sql() + &in_transit_sql())).await;
    succeeds(pool, &rts.sql(0, PRICE)).await;
}

/// 原件腿：已接管、Take 已起写，证明尚未提交。
pub async fn seed_original_started(
    pool: &PgPool,
    store: &Arc<DatabaseStore>,
    rts: Rts,
) -> (AcquireReceipt, Claim) {
    seed_original_rts(pool, rts).await;
    let service = service(store);
    let receipt = service.acquire(original_facts(rts)).await.expect("接管");
    let claim = Claim::new(21, 1, true);
    claim_take(pool, &receipt, take_delivery(pool, &receipt).await, claim).await;
    let started = start(&service, pool, &receipt, claim).await.expect("起写");
    assert!(matches!(started, WriteStart::FirstCommitted { .. }), "首次起写得到许可");
    (receipt, claim)
}

/// K 已被另一种终局占用：空信归档（首次决定与终局同种，与 RTS 的原信、观察同 K）。
pub fn archived_empty_final_sql(rts: Rts) -> String {
    let (mid, obs) = (rts.mid(), rts.observation());
    let command = Uuid::from_u128(0xc7a0_0000_0000_5000_8000_0000_0000_0000 + u128::from(rts.n));
    let decision = Uuid::from_u128(0xc7a1_0000_0000_4000_8000_0000_0000_0000 + u128::from(rts.n));
    format!(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_bot_uid, target_world_id, target_external_mid,
             listing_id, result_status, result_code, result_body, created_at)
         VALUES ('{command}', 2, 'worker', 'consignment', 'decide_envelope', '12345|main|{mid}',
                 decode(repeat('e6', 32), 'hex'), 'mail_key', '12345', 'main', '{mid}', NULL,
                 200, 'ok',
                 '{{\"schema_version\":2,\"operation\":\"decide_envelope\",
                    \"disposition\":\"archived_empty\",\"command_id\":\"{command}\",
                    \"decision_id\":\"{decision}\"}}', '{SOLD_AT}');
         INSERT INTO public.mail_initial_decisions
            (bot_uid, world_id, external_mid, id, observation_id, command_id, kind,
             quarantine_id, decided_at)
         VALUES ('12345', 'main', '{mid}', '{decision}', '{obs}', '{command}',
                 'archived_empty', NULL, '{SOLD_AT}');
         INSERT INTO public.mail_final_dispositions
            (bot_uid, world_id, external_mid, id, kind, source_observation_id, command_id,
             decided_at)
         VALUES ('12345', 'main', '{mid}', '{decision}', 'archived_empty', '{obs}', '{command}',
                 '{SOLD_AT}');"
    )
}

pub fn service(store: &Arc<DatabaseStore>) -> ReturnService {
    let port: Arc<dyn ReturnIntakeStore> = store.clone();
    ReturnService::new(port)
}

pub async fn acquire(
    service: &ReturnService,
    rts: Rts,
) -> Result<AcquireReceipt, ConsignmentError> {
    service.acquire(rts.facts(OBLIGATION_3)).await
}

/// Take 的一次领取：attempt 与 fence、claim token、租约是否仍有效。
#[derive(Debug, Clone, Copy)]
pub struct Claim {
    pub attempt: Uuid,
    pub fence: u64,
    pub token: Uuid,
    pub live: bool,
}

impl Claim {
    pub fn new(n: u8, fence: u64, live: bool) -> Self {
        Self {
            attempt: Uuid::from_bytes([n; 16]),
            fence,
            token: Uuid::from_bytes([n.wrapping_add(100); 16]),
            live,
        }
    }
}

/// 领取 Take 投递：写 attempt、把投递推进到该 fence 与 token，租约按 `live` 设为未来或已过。
pub async fn claim_take(pool: &PgPool, receipt: &AcquireReceipt, delivery: Uuid, claim: Claim) {
    let lease =
        if claim.live { "timestamptz '2099-01-01T00:00:00Z'" } else { "now() - interval '1 hour'" };
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.mail_disposition_attempts
                (id, work_id, delivery_id, fence, credential_id, credential_version,
                 runtime_generation, session_id, expected_digest, prepared_at)
             VALUES ('{attempt}', '{work}', '{delivery}', {fence}, '{CREDENTIAL}', 1, 1,
                     '{SESSION}', decode(repeat('e1', 32), 'hex'), '{SOLD_AT}');
             UPDATE public.deliveries
             SET status = 'claimed', lease_token = '{token}', lease_expires_at = {lease},
                 fencing_version = {fence}, attempt_count = {fence}
             WHERE id = '{delivery}';",
            attempt = claim.attempt,
            work = receipt.work_id,
            fence = claim.fence,
            token = claim.token,
        ),
    )
    .await;
}

pub async fn take_delivery(pool: &PgPool, receipt: &AcquireReceipt) -> Uuid {
    sqlx::query_scalar(
        "SELECT take_delivery_id FROM public.consignment_return_intakes WHERE id = $1",
    )
    .bind(receipt.intake_id)
    .fetch_one(pool)
    .await
    .expect("读取 Take 投递")
}

pub async fn session_binding(pool: &PgPool) -> [u8; 32] {
    let bytes: Vec<u8> = sqlx::query_scalar(
        "SELECT binding_digest FROM public.mail_evidence_sessions WHERE id = $1",
    )
    .bind(Uuid::parse_str(SESSION).expect("会话"))
    .fetch_one(pool)
    .await
    .expect("读取会话绑定");
    <[u8; 32]>::try_from(bytes.as_slice()).expect("32 字节摘要")
}

pub async fn start_input(pool: &PgPool, receipt: &AcquireReceipt, claim: Claim) -> StartTakeInput {
    StartTakeInput {
        intake_id: receipt.intake_id,
        work_id: receipt.work_id,
        delivery_id: take_delivery(pool, receipt).await,
        attempt_id: claim.attempt,
        fence: claim.fence,
        claim_token: claim.token,
        session_binding_digest: session_binding(pool).await,
        expected_manifest_digest: MANIFEST,
    }
}

pub async fn start(
    service: &ReturnService,
    pool: &PgPool,
    receipt: &AcquireReceipt,
    claim: Claim,
) -> Result<WriteStart, ConsignmentError> {
    service.start_take(start_input(pool, receipt, claim).await).await
}

/// 消费输入；`validator` 与 `digest` 决定证明的身份与签收状态。
pub fn consume_input(
    receipt: &AcquireReceipt,
    rts: Rts,
    claim: Claim,
    validator: &str,
    digest: u8,
) -> ConsumeReturnInput {
    ConsumeReturnInput {
        intake_id: receipt.intake_id,
        original_attempt_id: claim.attempt,
        fence: claim.fence,
        observation_id: rts.observation(),
        validator_version: validator.to_owned(),
        proof_digest: [digest; 32],
    }
}

/// 交给第 7 期复用：已接管、Take 已起写（现金阻断已建立），证明尚未提交。
pub async fn seed_started_unproven(
    pool: &PgPool,
    store: &Arc<DatabaseStore>,
    rts: Rts,
) -> (AcquireReceipt, Claim) {
    seed_refund_rts(pool, store, rts).await;
    let service = service(store);
    let receipt = acquire(&service, rts).await.expect("接管");
    let claim = Claim::new(21, 1, true);
    claim_take(pool, &receipt, take_delivery(pool, &receipt).await, claim).await;
    let started = start(&service, pool, &receipt, claim).await.expect("起写");
    assert!(matches!(started, WriteStart::FirstCommitted { .. }), "首次起写得到许可");
    (receipt, claim)
}

/// 交给第 7 期复用：已消费，精确 Take 证明、消费行与资金回补同事务已提交，阻断已解除。
pub async fn seed_consumed(
    pool: &PgPool,
    store: &Arc<DatabaseStore>,
    rts: Rts,
) -> (AcquireReceipt, Claim) {
    let (receipt, claim) = seed_started_unproven(pool, store, rts).await;
    service(store)
        .consume(consume_input(&receipt, rts, claim, SIGNED_VALIDATOR, 0xa7))
        .await
        .expect("消费");
    (receipt, claim)
}

pub const AMOUNT: i64 = PRICE;

/// 隔离中的系统 RTS：首次观察（版本 1）与第二次观察（版本 2，引用 `reference` 指向的轮）。
///
/// 隔离已处于版本 2，可用第二次观察解除；`dispatch_reference` 是 RTS 正文末行的派发引用（轮 ID）。
pub fn quarantined_rts_sql(rts: Rts, reference: &str) -> String {
    let n = rts.n;
    let (mid, second) = (rts.mid(), rts.observation());
    let first = Uuid::from_u128(0xc7e0_0000_0000_4000_8000_0000_0000_0000 + u128::from(n));
    let command = Uuid::from_u128(0xc7c0_0000_0000_5000_8000_0000_0000_0000 + u128::from(n));
    let initial = Uuid::from_u128(0xc7d0_0000_0000_4000_8000_0000_0000_0000 + u128::from(n));
    let quarantine = Uuid::from_u128(0xc7b0_0000_0000_4000_8000_0000_0000_0000 + u128::from(n));
    let observation = |id: Uuid, remote: &str, digest: &str, link: &str, facts: &str| {
        format!(
            "INSERT INTO public.mail_envelope_observations
                (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
                 mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
                 evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
                 returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
                 season_window_id)
             VALUES ('{id}', '12345', 'main', '{mid}', '{remote}', '{SESSION}', 'fake-mailbox',
                     {link}, '{SOLD_AT}', 'parser-v1', NULL, decode(repeat('66', 32), 'hex'),
                     decode(repeat('67', 32), 'hex'), decode(repeat('{digest}', 32), 'hex'),
                     'MoogleMail', 'FakeBot', true, true, 0, 0, NULL, '[]'::jsonb,
                     '{facts}'::jsonb, NULL);"
        )
    };
    let facts = |extra: &str| {
        format!(
            "{{\"subject_is_consign\":true,\"sender_identity\":\"verified\",
              \"list_detail_consistent\":true,\"duplicate_raw_eid\":false,
              \"canonical_alias_ambiguous\":false,\"unknown_attachment\":false,
              \"system_rts\":true,\"legacy_auction_owner\":null,
              \"received_minute_raw\":\"2026-01-01T05:00\",\"items\":[]{extra}}}"
        )
    };
    let body = format!(
        "{{\"schema_version\":2,\"operation\":\"decide_envelope\",
          \"disposition\":\"quarantined\",\"command_id\":\"{command}\",
          \"decision_id\":\"{initial}\",
          \"quarantine\":{{\"quarantine_id\":\"{quarantine}\",\"version\":1}}}}"
    );
    format!(
        "INSERT INTO public.mail_originals
            (bot_uid, world_id, external_mid, first_session_id, received_minute_raw,
             received_timezone, first_seen_at, season_window_id)
         VALUES ('12345', 'main', '{mid}', '{SESSION}', '2026-01-01T05:00', 'UTC',
                 '{SOLD_AT}', NULL);
         {first_row}
         INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_bot_uid, target_world_id, target_external_mid,
             listing_id, result_status, result_code, result_body, created_at)
         VALUES ('{command}', 2, 'worker', 'consignment', 'decide_envelope',
                 '12345|main|{mid}', decode(repeat('e5', 32), 'hex'), 'mail_key', '12345',
                 'main', '{mid}', NULL, 200, 'ok', '{body}', '{SOLD_AT}');
         INSERT INTO public.mail_initial_decisions
            (bot_uid, world_id, external_mid, id, observation_id, command_id, kind,
             quarantine_id, decided_at)
         VALUES ('12345', 'main', '{mid}', '{initial}', '{first}', '{command}', 'quarantined',
                 '{quarantine}', '{SOLD_AT}');
         INSERT INTO public.mail_quarantines
            (id, bot_uid, world_id, external_mid, initial_decision_id, version, state,
             reread_locator, responsibility_code, created_at, updated_at)
         VALUES ('{quarantine}', '12345', 'main', '{mid}', '{initial}', 1, 'active',
                 '{{\"v\":2,\"b\":\"fake-mailbox\"}}', 'suspected_system_rts', '{SOLD_AT}', '{SOLD_AT}');
         UPDATE public.mail_quarantines SET version = 2, updated_at = '{SOLD_AT}'
         WHERE id = '{quarantine}';
         {second_row}",
        first_row = observation(first, &format!("obs-q1-{mid}"), "77", "NULL, NULL", &facts("")),
        second_row = observation(
            second,
            &format!("obs-q2-{mid}"),
            "e1",
            &format!("'{quarantine}', 2"),
            &facts(&format!(",\"dispatch_reference\":\"{reference}\"")),
        ),
    )
}

/// main 世界就绪且引用夹具会话（领取 Take 与 poll 预读都按它绑定会话）。
pub async fn seed_main_readiness(pool: &PgPool) {
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.consignment_mail_readiness
                (world_id, session_id, service_epoch, policy_digest, allowlist_mode,
                 allowed_senders, validator_versions, detail_ready, take_ready, return_ready,
                 checked_at, valid_until, version)
             VALUES ('main', '{SESSION}', 1, decode(repeat('35', 32), 'hex'), 'disabled',
                     ARRAY[]::text[], ARRAY[]::text[], true, true, true,
                     clock_timestamp(), clock_timestamp() + interval '1 day', 1)
             ON CONFLICT (world_id) DO UPDATE
             SET session_id = excluded.session_id, detail_ready = true,
                 valid_until = excluded.valid_until,
                 version = public.consignment_mail_readiness.version + 1;"
        ),
    )
    .await;
}

pub fn quarantine_of(rts: Rts) -> Uuid {
    Uuid::from_u128(0xc7b0_0000_0000_4000_8000_0000_0000_0000 + u128::from(rts.n))
}
