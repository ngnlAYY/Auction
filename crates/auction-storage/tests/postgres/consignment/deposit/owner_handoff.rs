//! 隔离解除交还两个旧 owner 的 PG18 行为：原结算案件遵守旧分类与履约迁移，原恢复流程只写
//! 接管并在下一事务消费；同时覆盖并发、回滚、未知义务与批处理对照。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use auction_application::consignment::{
    DecideEnvelopeCommand, DepositActor, EnvelopeStore, ReleaseQuarantineCommand, ReleaseReceipt,
};
use auction_application::core::RequestHash;
use auction_application::recovery::RecoveryWorkStore;
use auction_application::settlement::SettlementReturnReason;
use auction_domain::SettlementWorld;
use auction_domain::consignment::OriginalMailKey;
use auction_domain::consignment::deposit::{
    EnvelopeCapabilities, EnvelopeDecision, LegacyAuctionOwner, LockedEnvelopeFacts,
    QuarantineReason, ReleaseKind,
};
use auction_domain::{CommandId, SettlementDispatchReference};
use auction_time::{Duration, Timestamp};
use serde_json::{Value, json};
use sqlx::{AssertSqlSafe, PgPool};
use uuid::Uuid;

use super::envelopes::{observation, quarantine_command, repository, seed_base, worker_command};
use crate::recovery_returns::{CodSale, insert_sale, settlement_minute};
use crate::settlement_returns::{evidence, insert_sent_fulfillment};
use crate::support::TestDatabase;

const MID: &str = "555";
const ISEKAI_MID: &str = "556";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReturnFixture {
    SellerPayout,
    UnsoldReturn,
    WithdrawnReturn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecoveryFixture {
    MainEquipmentCod,
    IsekaiCarrier,
    IsekaiEquipmentHandoff,
}

pub(in crate::consignment) struct MainReturn {
    pub(in crate::consignment) obligation_id: Uuid,
    pub(in crate::consignment) delivery_id: Uuid,
    pub(in crate::consignment) return_mail_id: Uuid,
}

struct RecoveryReturn {
    recovery_id: Uuid,
    obligation_id: Uuid,
    payment_obligation_id: Uuid,
    equipment_obligation_id: Option<Uuid>,
    return_mail_id: Uuid,
    action_kind: &'static str,
}

fn at(value: &str) -> Timestamp {
    Timestamp::parse_from_rfc3339(value).expect("固定时刻")
}

fn owner_name(owner: LegacyAuctionOwner) -> &'static str {
    match owner {
        LegacyAuctionOwner::SellerPayout => "seller_payout",
        LegacyAuctionOwner::BuyerRefund => "buyer_refund",
        LegacyAuctionOwner::UnsoldReturn => "unsold_return",
        LegacyAuctionOwner::WithdrawnReturn => "withdrawn_return",
        LegacyAuctionOwner::WinnerEquipmentCod => "winner_equipment_cod",
        LegacyAuctionOwner::WinnerPaymentCarrierCod => "winner_payment_carrier_cod",
        LegacyAuctionOwner::WinnerEquipmentHandoff => "winner_equipment_handoff",
    }
}

pub(in crate::consignment) async fn count(pool: &PgPool, query: &'static str) -> i64 {
    sqlx::query_scalar(AssertSqlSafe(query.to_owned())).fetch_one(pool).await.expect("读取计数")
}

async fn insert_mail(
    pool: &PgPool,
    world_id: &str,
    external_mail_id: &str,
    received_at: Timestamp,
    subject: String,
    metadata: Value,
) -> Uuid {
    insert_inbound(
        pool,
        "owner-handoff",
        "settlement_recovery_returned",
        world_id,
        external_mail_id,
        received_at,
        subject,
        metadata,
    )
    .await
}

#[allow(clippy::too_many_arguments, reason = "测试夹具逐列描述入站邮件")]
async fn insert_inbound(
    pool: &PgPool,
    mailbox_key: &str,
    decision_code: &str,
    world_id: &str,
    external_mail_id: &str,
    received_at: Timestamp,
    subject: String,
    metadata: Value,
) -> Uuid {
    let return_mail_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.inbound_mail
            (id, mailbox_key, external_mail_id, world_id, mail_kind, content_digest,
             received_at, status, decision_code, decision_metadata, processed_at, sender,
             subject)
         VALUES ($1, $2, $3, $4, 'delivery', $5, $6, 'accepted',
                 $7, $8, $6 + interval '1 minute',
                 'MoogleMail', $9)",
    )
    .bind(return_mail_id)
    .bind(mailbox_key)
    .bind(external_mail_id)
    .bind(world_id)
    .bind([9_u8; 32].as_slice())
    .bind(received_at)
    .bind(decision_code)
    .bind(metadata)
    .bind(subject)
    .execute(pool)
    .await
    .expect("写入退件入站");
    return_mail_id
}

async fn seed_seller_payout(pool: &PgPool, sequence: i64) -> MainReturn {
    let (minute, settled_at) = settlement_minute();
    let sale = insert_sale(pool, sequence, settled_at, 1).await;
    let delivery_id = insert_sent_fulfillment(
        pool,
        sale.seller_obligation,
        "settlement_seller_payout",
        90,
        minute + Duration::minutes(10),
        "enqueued",
    )
    .await;
    let return_mail_id = insert_mail(
        pool,
        "main",
        MID,
        minute + Duration::minutes(40),
        format!("[RTS] Auction seller payout [{}]", sale.seller_obligation),
        evidence(sale.seller_obligation, 90, &[]),
    )
    .await;
    MainReturn { obligation_id: sale.seller_obligation, delivery_id, return_mail_id }
}

pub(in crate::consignment) async fn seed_buyer_refund(pool: &PgPool, sequence: i64) -> MainReturn {
    let (minute, settled_at) = settlement_minute();
    let sale = insert_sale(pool, sequence, settled_at, 1).await;
    let delivery_id = insert_sent_fulfillment(
        pool,
        sale.refund_obligation,
        "settlement_buyer_refund",
        100,
        minute + Duration::minutes(10),
        "enqueued",
    )
    .await;
    let return_mail_id = insert_mail(
        pool,
        "main",
        MID,
        minute + Duration::minutes(40),
        format!("[RTS] Auction buyer refund [{}]", sale.refund_obligation),
        evidence(sale.refund_obligation, 100, &[]),
    )
    .await;
    MainReturn { obligation_id: sale.refund_obligation, delivery_id, return_mail_id }
}

async fn insert_item_obligation(pool: &PgPool, sale: &CodSale, kind: &str) -> Uuid {
    let obligation_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.settlement_obligations
            (id, auction_id, lot_id, world_id, origin_command_id, policy_version,
             obligation_kind, business_state, recipient_game_identity, amount, carrier_credit,
             equipment_id)
         SELECT $1, auction_id, lot_id, 'main', origin_command_id, 1, $2, 'ready',
                'FakeSellerOne', 0, 0, equipment_id
         FROM public.settlement_obligations WHERE id = $3",
    )
    .bind(obligation_id)
    .bind(kind)
    .bind(sale.payment_obligation)
    .execute(pool)
    .await
    .expect("写入流拍或撤拍返还义务");
    obligation_id
}

async fn seed_item_return(pool: &PgPool, sequence: i64, kind: &str) -> MainReturn {
    let (minute, settled_at) = settlement_minute();
    let sale = insert_sale(pool, sequence, settled_at, 1).await;
    let obligation_id = insert_item_obligation(pool, &sale, kind).await;
    let delivery_id = insert_sent_fulfillment(
        pool,
        obligation_id,
        "settlement_equipment_handoff",
        0,
        minute + Duration::minutes(10),
        "enqueued",
    )
    .await;
    let return_mail_id = insert_mail(
        pool,
        "main",
        MID,
        minute + Duration::minutes(40),
        format!("[RTS] Auction equipment delivery [{obligation_id}]"),
        evidence(obligation_id, 0, &[sale.external_equipment_id.as_str()]),
    )
    .await;
    MainReturn { obligation_id, delivery_id, return_mail_id }
}

async fn seed_recovery(pool: &PgPool, sequence: i64, fixture: RecoveryFixture) -> RecoveryReturn {
    let (minute, settled_at) = settlement_minute();
    let sale = insert_sale(pool, sequence, settled_at, 1).await;
    let (world, payment, equipment_obligation, return_world, subject, metadata, action_kind) =
        match fixture {
            RecoveryFixture::MainEquipmentCod => (
                "main",
                sale.payment_obligation,
                None,
                "main",
                format!("[RTS] Auction equipment delivery [{}]", sale.payment_obligation),
                evidence(sale.payment_obligation, 0, &[sale.external_equipment_id.as_str()]),
                "reclaim_equipment_take",
            ),
            RecoveryFixture::IsekaiCarrier => {
                let (carrier, handoff) = insert_isekai_recovery_obligations(pool, &sale).await;
                (
                    "isekai",
                    carrier,
                    Some(handoff),
                    "main",
                    format!("[RTS] Auction payment request [{carrier}]"),
                    evidence(carrier, 1, &[]),
                    "cancel_carrier_take",
                )
            }
            RecoveryFixture::IsekaiEquipmentHandoff => {
                let (carrier, handoff) = insert_isekai_recovery_obligations(pool, &sale).await;
                (
                    "isekai",
                    carrier,
                    Some(handoff),
                    "isekai",
                    format!("[RTS] Auction equipment delivery [{handoff}]"),
                    evidence(handoff, 0, &[sale.external_equipment_id.as_str()]),
                    "recover_equipment_take",
                )
            }
        };
    let obligation_id = if fixture == RecoveryFixture::IsekaiCarrier {
        payment
    } else if fixture == RecoveryFixture::IsekaiEquipmentHandoff {
        equipment_obligation.expect("isekai 交割义务")
    } else {
        payment
    };
    let recovery_id =
        insert_recovery(pool, &sale, world, payment, equipment_obligation, settled_at).await;
    let return_mail_id = insert_mail(
        pool,
        return_world,
        if return_world == "isekai" { ISEKAI_MID } else { MID },
        minute,
        subject,
        metadata,
    )
    .await;
    RecoveryReturn {
        recovery_id,
        obligation_id,
        payment_obligation_id: payment,
        equipment_obligation_id: equipment_obligation,
        return_mail_id,
        action_kind,
    }
}

async fn insert_recovery(
    pool: &PgPool,
    sale: &CodSale,
    world: &str,
    payment_obligation_id: Uuid,
    equipment_obligation_id: Option<Uuid>,
    created_at: Timestamp,
) -> Uuid {
    let recovery_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.settlement_recoveries
            (id, auction_id, lot_id, world_id, equipment_id, winner_user_id,
             payment_obligation_id, equipment_obligation_id, seller_payout_obligation_id,
             buyer_refund_obligation_id, policy_version, payment_deadline_at, stage,
             state_version, cod_return_count, cod_return_limit, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 1,
                 $11 + interval '7 days', 'awaiting_payment', 1, 0, 3, $11, $11)",
    )
    .bind(recovery_id)
    .bind(sale.auction_id)
    .bind(sale.lot_id)
    .bind(world)
    .bind(sale.equipment_id)
    .bind(sale.winner_user_id)
    .bind(payment_obligation_id)
    .bind(equipment_obligation_id)
    .bind(sale.seller_obligation)
    .bind(sale.refund_obligation)
    .bind(created_at)
    .execute(pool)
    .await
    .expect("写入恢复记录");
    recovery_id
}

async fn insert_isekai_recovery_obligations(pool: &PgPool, sale: &CodSale) -> (Uuid, Uuid) {
    let carrier = Uuid::new_v4();
    let handoff = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.settlement_obligations
            (id, auction_id, lot_id, world_id, origin_command_id, policy_version,
             obligation_kind, business_state, recipient_user_id, amount, carrier_credit,
             equipment_id, prerequisite_obligation_id)
         SELECT $1, auction_id, lot_id, 'isekai', origin_command_id, 1,
                'winner_payment_carrier_cod', 'ready', recipient_user_id, 101, 1,
                equipment_id, NULL
         FROM public.settlement_obligations WHERE id = $2",
    )
    .bind(carrier)
    .bind(sale.payment_obligation)
    .execute(pool)
    .await
    .expect("写入 isekai 载体付款义务");
    sqlx::query(
        "INSERT INTO public.settlement_obligations
            (id, auction_id, lot_id, world_id, origin_command_id, policy_version,
             obligation_kind, business_state, recipient_user_id, amount, carrier_credit,
             equipment_id, prerequisite_obligation_id)
         SELECT $1, auction_id, lot_id, 'isekai', origin_command_id, 1,
                'winner_equipment_handoff', 'ready', recipient_user_id, 0, 0, equipment_id, $2
         FROM public.settlement_obligations WHERE id = $3",
    )
    .bind(handoff)
    .bind(carrier)
    .bind(sale.payment_obligation)
    .execute(pool)
    .await
    .expect("写入 isekai 交割义务");
    (carrier, handoff)
}

async fn seed_isekai_observation(pool: &PgPool) {
    let credential = Uuid::new_v4();
    let session = Uuid::new_v4();
    let observation_id =
        Uuid::parse_str("8f000000-0000-0000-0000-000000000001").expect("isekai 观察");
    let source_observation: Uuid = sqlx::query_scalar(
        "SELECT id FROM public.mail_envelope_observations
         WHERE remote_observation_id = 'obs-quarantine'",
    )
    .fetch_one(pool)
    .await
    .expect("读取 main 隔离观察");
    sqlx::query(
        "UPDATE public.credential_keys
         SET status = 'disabled', state_version = state_version + 1, updated_at = clock_timestamp()
         WHERE credential_kind = 'mail_cookie' AND status = 'active'",
    )
    .execute(pool)
    .await
    .expect("停用 main 邮件凭据");
    sqlx::query(
        "INSERT INTO public.credential_keys
            (id, credential_kind, scope, label, ciphertext, nonce, encryption_key_id,
             fingerprint, status, state_version, created_at, updated_at)
         VALUES ($1, 'mail_cookie', 'isekai', 'fake-isekai', decode('21', 'hex'),
                 decode(repeat('22', 12), 'hex'), 'fake-data-key', repeat('b', 64),
                 'active', 1, $2, $2)",
    )
    .bind(credential)
    .bind(at("2026-01-01T00:00:00Z"))
    .execute(pool)
    .await
    .expect("写入 isekai 邮件凭据");
    sqlx::query(
        "INSERT INTO public.mail_evidence_sessions
            (id, bot_uid, world_id, player_name, credential_id, credential_version,
             runtime_generation, observed_at, protocol_version, binding_digest)
         VALUES ($1, '12345', 'isekai', 'FakeBot', $2, 1, 1, $3, 'v1', $4)",
    )
    .bind(session)
    .bind(credential)
    .bind(at("2026-01-01T01:00:00Z"))
    .bind([0x34_u8; 32].as_slice())
    .execute(pool)
    .await
    .expect("写入 isekai 证据会话");
    sqlx::query(
        "INSERT INTO public.mail_originals
            (bot_uid, world_id, external_mid, first_session_id, received_minute_raw,
             received_timezone, first_seen_at, season_window_id)
         VALUES ('12345', 'isekai', $1, $2, '2026-01-01T03:00', 'UTC', $3, NULL)",
    )
    .bind(ISEKAI_MID)
    .bind(session)
    .bind(at("2026-01-01T03:00:00Z"))
    .execute(pool)
    .await
    .expect("写入 isekai 原信");
    let inserted = sqlx::query(
        "INSERT INTO public.mail_envelope_observations
            (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
             mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
             evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
             returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
             season_window_id)
         SELECT $1, '12345', 'isekai', $2, 'isekai-quarantine', $3, 'isekai-mailbox',
                NULL, NULL, observed_at, parser_version, evidence_ref, subject_digest,
                body_digest, manifest_digest, sender, recipient, returnable, takeable,
                credits, cod, raw_count, manifest, derived_facts, NULL
         FROM public.mail_envelope_observations
         WHERE id = $4",
    )
    .bind(observation_id)
    .bind(ISEKAI_MID)
    .bind(session)
    .bind(source_observation)
    .execute(pool)
    .await
    .expect("写入 isekai 隔离观察");
    assert_eq!(inserted.rows_affected(), 1, "isekai 观察必须从 main 观察派生");
}

fn isekai_quarantine_command() -> DecideEnvelopeCommand {
    let key =
        OriginalMailKey::new("12345", SettlementWorld::Isekai, ISEKAI_MID).expect("isekai 原信键");
    let mut observed = observation(true);
    observed.observation_id =
        Uuid::parse_str("8f000000-0000-0000-0000-000000000001").expect("观察");
    observed.session_binding_digest = [0x34; 32];
    observed.manifest_digest = [0x7b; 32];
    DecideEnvelopeCommand {
        command_id: worker_command(&format!("isekai-quarantine-{}", Uuid::new_v4())),
        request_hash: RequestHash::new([18; 32]),
        mail_key: key.clone(),
        facts: LockedEnvelopeFacts {
            mail_key: Some(key),
            observation: observed,
            reservations: Vec::new(),
            envelope_seller_user_id: None,
            capabilities: EnvelopeCapabilities { safe_original_return: true, take_ready: true },
            read_failure: false,
            already_decided: false,
            now: Timestamp::UNIX_EPOCH,
        },
        decision: EnvelopeDecision::Quarantine(QuarantineReason::DuplicateRawEid),
    }
}

pub(in crate::consignment) async fn create_quarantine(
    database: &TestDatabase,
    repo: &auction_storage::DepositRepository,
    owner: LegacyAuctionOwner,
    isekai: bool,
) -> (Uuid, Uuid) {
    if isekai {
        seed_isekai_observation(
            database.maintenance_store().await.expect("维护账号").postgres_pool(),
        )
        .await;
    }
    let receipt = repo
        .decide_envelope(if isekai {
            isekai_quarantine_command()
        } else {
            quarantine_command(worker_command(&format!(
                "owner-handoff-{}-{}",
                owner_name(owner),
                Uuid::new_v4()
            )))
        })
        .await
        .expect("首次隔离");
    let quarantine = receipt.quarantine.expect("隔离引用");
    let maintenance = database.maintenance_store().await.expect("维护账号");
    let pool = maintenance.postgres_pool();
    let source_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM public.mail_envelope_observations WHERE quarantine_id=$1",
    )
    .bind(quarantine.quarantine_id)
    .fetch_one(pool)
    .await
    .expect("读取隔离内观察");
    let advanced_observation = Uuid::new_v4();
    let mut transaction = pool.begin().await.expect("开始补证事务");
    sqlx::query(
        "WITH advanced AS (
             UPDATE public.mail_quarantines
             SET reread_locator = $2, version = version + 1, updated_at = clock_timestamp()
             WHERE id = $1
             RETURNING id, version
         )
         INSERT INTO public.mail_envelope_observations
            (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
             mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
             evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
             returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
             season_window_id)
         SELECT $3, o.bot_uid, o.world_id, o.external_mid, $4, o.session_id, o.mailbox_key,
                advanced.id, advanced.version, o.observed_at, o.parser_version,
                o.evidence_ref, o.subject_digest, o.body_digest, o.manifest_digest,
                o.sender, o.recipient, o.returnable, o.takeable, o.credits, o.cod,
                o.raw_count, o.manifest,
                jsonb_set(o.derived_facts, '{legacy_auction_owner}', to_jsonb($6::text), false),
                o.season_window_id
         FROM public.mail_envelope_observations AS o, advanced
         WHERE o.id = $5",
    )
    .bind(quarantine.quarantine_id)
    .bind("{\"v\":2,\"b\":\"owner-handoff\"}")
    .bind(advanced_observation)
    .bind(format!("legacy-{}", Uuid::new_v4()))
    .bind(source_id)
    .bind(owner_name(owner))
    .execute(&mut *transaction)
    .await
    .expect("追加旧 owner 观察");
    transaction.commit().await.expect("提交补证事务");
    (quarantine.quarantine_id, advanced_observation)
}

pub(in crate::consignment) async fn release(
    repo: &auction_storage::DepositRepository,
    quarantine_id: Uuid,
    observation_id: Uuid,
    kind: auction_domain::consignment::deposit::ReleaseKind,
    command_id: CommandId,
) -> Result<ReleaseReceipt, auction_application::consignment::ConsignmentError> {
    repo.release_quarantine(ReleaseQuarantineCommand {
        command_id,
        actor: DepositActor::Admin("fake-admin".to_owned()),
        quarantine_id,
        expected_version: 2,
        observation_id,
        proposed_kind: kind,
        request_hash: RequestHash::new([22; 32]),
        request_id: None,
        auto_binding: None,
    })
    .await
}

async fn release_retry(
    repo: &auction_storage::DepositRepository,
    quarantine_id: Uuid,
    observation_id: Uuid,
    kind: auction_domain::consignment::deposit::ReleaseKind,
    command_id: CommandId,
) -> Result<ReleaseReceipt, auction_application::consignment::ConsignmentError> {
    let mut last_error = None;
    for _ in 0..3 {
        match release(repo, quarantine_id, observation_id, kind, command_id).await {
            Ok(receipt) => return Ok(receipt),
            Err(auction_application::consignment::ConsignmentError::Unavailable) => {
                last_error = Some(auction_application::consignment::ConsignmentError::Unavailable);
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error.expect("至少尝试一次"))
}

pub(in crate::consignment) async fn assert_return_case_written(
    pool: &PgPool,
    fixture: &MainReturn,
    expected_reason: SettlementReturnReason,
) {
    let evidence_row: (String, String, Option<Uuid>, Option<Uuid>, Option<String>) =
        sqlx::query_as(
            "SELECT match_state, reason_code, obligation_id, delivery_id, previous_execution_state
         FROM public.settlement_return_evidence WHERE return_mail_id = $1",
        )
        .bind(fixture.return_mail_id)
        .fetch_one(pool)
        .await
        .expect("读取退件证据");
    assert_eq!(
        evidence_row,
        (
            "manual_reconcile".to_owned(),
            expected_reason.as_str().to_owned(),
            Some(fixture.obligation_id),
            Some(fixture.delivery_id),
            Some("enqueued".to_owned())
        )
    );
    let case_row: (Option<Uuid>, Option<Uuid>, Option<i32>, String, String) = sqlx::query_as(
        "SELECT obligation_id, delivery_id, dispatch_no, status, reason_code
         FROM public.settlement_return_cases WHERE return_mail_id = $1",
    )
    .bind(fixture.return_mail_id)
    .fetch_one(pool)
    .await
    .expect("读取退件案件");
    assert_eq!(
        case_row,
        (
            Some(fixture.obligation_id),
            Some(fixture.delivery_id),
            Some(1),
            "awaiting_decision".to_owned(),
            expected_reason.as_str().to_owned()
        )
    );
    let fulfillment: (String, Option<String>, i64) = sqlx::query_as(
        "SELECT execution_state, last_error_code, state_version
         FROM public.settlement_obligation_fulfillments WHERE obligation_id = $1",
    )
    .bind(fixture.obligation_id)
    .fetch_one(pool)
    .await
    .expect("读取履约");
    assert_eq!(
        fulfillment,
        ("manual_reconcile".to_owned(), Some("settlement_mail_returned".to_owned()), 4)
    );
}

pub(in crate::consignment) async fn assert_no_asset_side_effects(
    pool: &PgPool,
    before_deliveries: i64,
) {
    assert_eq!(count(pool, "SELECT count(*) FROM public.deliveries").await, before_deliveries);
    assert_eq!(count(pool, "SELECT count(*) FROM public.mail_disposition_work").await, 0);
    assert_eq!(count(pool, "SELECT count(*) FROM public.consignment_deposit_items").await, 0);
    assert_eq!(count(pool, "SELECT count(*) FROM public.consignment_custody_positions").await, 0);
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_funding_returns").await, 0);
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_funding_allocations").await, 0);
}

async fn seed_return_case_evidence(pool: &PgPool, return_mail_id: Uuid, obligation_id: Uuid) {
    sqlx::query(
        "INSERT INTO public.settlement_return_evidence
            (return_mail_id, match_state, reason_code, obligation_id, delivery_id,
             previous_execution_state, recorded_at)
         VALUES ($1, 'unbound', 'evidence_invalid', $2, NULL, NULL, now())",
    )
    .bind(return_mail_id)
    .bind(obligation_id)
    .execute(pool)
    .await
    .expect("写入先到的案件证据");
    sqlx::query(
        "INSERT INTO public.settlement_return_cases
            (return_mail_id, world_id, obligation_id, delivery_id, dispatch_no, status,
             reason_code, resend_limit, created_at, updated_at)
         VALUES ($1, 'main', $2, NULL, NULL, 'quarantined', 'evidence_invalid', 3,
                 now(), now())",
    )
    .bind(return_mail_id)
    .bind(obligation_id)
    .execute(pool)
    .await
    .expect("写入先到的案件");
    sqlx::query(
        "INSERT INTO public.settlement_return_events
            (return_mail_id, case_version, event_kind, status, details, occurred_at)
         VALUES ($1, 1, 'opened', 'quarantined', '{}'::jsonb, now())",
    )
    .bind(return_mail_id)
    .execute(pool)
    .await
    .expect("写入先到的案件事件");
}

async fn seed_foreign_link(pool: &PgPool, inbound_id: Uuid, source_observation_id: Uuid) {
    let foreign_observation_id = Uuid::new_v4();
    let foreign_session_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.mail_evidence_sessions
            (id, bot_uid, world_id, player_name, credential_id, credential_version,
             runtime_generation, observed_at, protocol_version, binding_digest)
         SELECT $1, '99999', session.world_id, 'FakeForeignBot', session.credential_id,
                session.credential_version, session.runtime_generation, session.observed_at,
                session.protocol_version, $2
         FROM public.mail_envelope_observations AS observation
         JOIN public.mail_evidence_sessions AS session ON session.id = observation.session_id
         WHERE observation.id = $3",
    )
    .bind(foreign_session_id)
    .bind([0x44_u8; 32].as_slice())
    .bind(source_observation_id)
    .execute(pool)
    .await
    .expect("写入另一认证 Bot 的证据会话");
    sqlx::query(
        "INSERT INTO public.mail_originals
            (bot_uid, world_id, external_mid, first_session_id, received_minute_raw,
             received_timezone, first_seen_at, season_window_id)
         SELECT '99999', 'main', '555', $1, received_minute_raw,
                received_timezone, first_seen_at, NULL
         FROM public.mail_originals
         WHERE bot_uid = '12345' AND world_id = 'main' AND external_mid = '555'",
    )
    .bind(foreign_session_id)
    .execute(pool)
    .await
    .expect("写入另一认证 Bot 的原信");
    sqlx::query(
        "INSERT INTO public.mail_envelope_observations
            (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
             mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
             evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
             returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
             season_window_id)
         SELECT $1, '99999', 'main', '555', 'obs-foreign', $3, mailbox_key,
                NULL, NULL, observed_at, parser_version, evidence_ref, subject_digest,
                body_digest, manifest_digest, sender, recipient, returnable, takeable,
                credits, cod, raw_count, manifest, derived_facts, NULL
         FROM public.mail_envelope_observations WHERE id = $2",
    )
    .bind(foreign_observation_id)
    .bind(source_observation_id)
    .bind(foreign_session_id)
    .execute(pool)
    .await
    .expect("写入另一认证 Bot 的观察");
    sqlx::query(
        "INSERT INTO public.mail_original_links
            (inbound_id, mailbox_key, external_mid, bot_uid, world_id, auth_observation_id)
         VALUES ($1, 'owner-handoff-bot', '555', '99999', 'main', $2)",
    )
    .bind(inbound_id)
    .bind(foreign_observation_id)
    .execute(pool)
    .await
    .expect("写入另一认证 Bot 的原信映射");
}

#[tokio::test]
async fn postgres_quarantine_hands_payout_and_item_returns_to_original_case() {
    for (sequence, fixture_kind) in [
        (7_500_001_i64, ReturnFixture::SellerPayout),
        (7_500_002, ReturnFixture::UnsoldReturn),
        (7_500_003, ReturnFixture::WithdrawnReturn),
    ] {
        let database = TestDatabase::create().await.expect("克隆模板库");
        let runtime = database.store().await.expect("运行账号");
        let pool = runtime.postgres_pool();
        seed_base(pool).await;
        let fixture = match fixture_kind {
            ReturnFixture::SellerPayout => seed_seller_payout(pool, sequence).await,
            ReturnFixture::UnsoldReturn => seed_item_return(pool, sequence, "unsold_return").await,
            ReturnFixture::WithdrawnReturn => {
                seed_item_return(pool, sequence, "withdrawn_return").await
            }
        };
        let repo = repository(&database).await;
        let owner = match fixture_kind {
            ReturnFixture::SellerPayout => LegacyAuctionOwner::SellerPayout,
            ReturnFixture::UnsoldReturn => LegacyAuctionOwner::UnsoldReturn,
            ReturnFixture::WithdrawnReturn => LegacyAuctionOwner::WithdrawnReturn,
        };
        let (quarantine_id, observation_id) =
            create_quarantine(&database, &repo, owner, false).await;
        let deliveries_before = count(pool, "SELECT count(*) FROM public.deliveries").await;
        let receipt = release(
            &repo,
            quarantine_id,
            observation_id,
            auction_domain::consignment::deposit::ReleaseKind::AuctionReturnCase,
            CommandId::from_uuid(Uuid::new_v4()),
        )
        .await
        .expect("旧案件交接成功");
        assert_eq!(receipt.obligation_id, Some(fixture.obligation_id));
        assert_eq!(receipt.return_mail_id, Some(fixture.return_mail_id));
        assert_eq!(receipt.return_evidence_id, Some(fixture.return_mail_id));
        assert_return_case_written(pool, &fixture, SettlementReturnReason::ReturnedAssetsMatch)
            .await;
        assert_no_asset_side_effects(pool, deliveries_before).await;
        pool.close().await;
    }
}

#[tokio::test]
async fn postgres_unknown_refund_obligation_creates_one_unbound_case() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let (minute, _) = settlement_minute();
    let unknown = Uuid::new_v4();
    let return_mail_id = insert_mail(
        pool,
        "main",
        MID,
        minute + Duration::minutes(5),
        format!("[RTS] Auction buyer refund [{unknown}]"),
        evidence(unknown, 100, &[]),
    )
    .await;
    let repo = repository(&database).await;
    let (quarantine_id, observation_id) =
        create_quarantine(&database, &repo, LegacyAuctionOwner::BuyerRefund, false).await;
    let receipt = release(
        &repo,
        quarantine_id,
        observation_id,
        auction_domain::consignment::deposit::ReleaseKind::AuctionReturnCase,
        CommandId::from_uuid(Uuid::new_v4()),
    )
    .await
    .expect("未知义务仍建立原案");
    assert_eq!(receipt.obligation_id, None);
    assert_eq!(receipt.return_mail_id, Some(return_mail_id));
    assert_eq!(receipt.return_evidence_id, Some(return_mail_id));
    let evidence_row: (String, String, Option<Uuid>) = sqlx::query_as(
        "SELECT match_state, reason_code, obligation_id
         FROM public.settlement_return_evidence WHERE return_mail_id = $1",
    )
    .bind(return_mail_id)
    .fetch_one(pool)
    .await
    .expect("读取未知义务证据");
    assert_eq!(evidence_row, ("unbound".to_owned(), "obligation_unknown".to_owned(), None));
    let case_row: (String, Option<Uuid>) = sqlx::query_as(
        "SELECT status, obligation_id FROM public.settlement_return_cases
         WHERE return_mail_id = $1",
    )
    .bind(return_mail_id)
    .fetch_one(pool)
    .await
    .expect("读取未知义务案件");
    assert_eq!(case_row, ("quarantined".to_owned(), None));
    assert_no_asset_side_effects(pool, 0).await;
    pool.close().await;
}

#[tokio::test]
async fn postgres_quarantine_hands_legacy_refund_to_original_case() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let fixture = seed_buyer_refund(pool, 7_500_010).await;
    let repo = repository(&database).await;
    let (quarantine_id, observation_id) =
        create_quarantine(&database, &repo, LegacyAuctionOwner::BuyerRefund, false).await;
    let deliveries_before = count(pool, "SELECT count(*) FROM public.deliveries").await;
    let receipt = release(
        &repo,
        quarantine_id,
        observation_id,
        auction_domain::consignment::deposit::ReleaseKind::AuctionReturnCase,
        CommandId::from_uuid(Uuid::new_v4()),
    )
    .await
    .expect("旧退款交接成功");
    assert_eq!(receipt.obligation_id, Some(fixture.obligation_id));
    assert_return_case_written(pool, &fixture, SettlementReturnReason::ReturnedAssetsMatch).await;
    assert_no_asset_side_effects(pool, deliveries_before).await;
    pool.close().await;
}

#[tokio::test]
async fn postgres_quarantine_hands_three_recovery_legs_to_original_recovery() {
    for (sequence, fixture_kind, isekai) in [
        (7_500_020_i64, RecoveryFixture::MainEquipmentCod, false),
        (7_500_021, RecoveryFixture::IsekaiCarrier, false),
        (7_500_022, RecoveryFixture::IsekaiEquipmentHandoff, true),
    ] {
        let database = TestDatabase::create().await.expect("克隆模板库");
        let runtime = database.store().await.expect("运行账号");
        let pool = runtime.postgres_pool();
        seed_base(pool).await;
        let fixture = seed_recovery(pool, sequence, fixture_kind).await;
        let repo = repository(&database).await;
        let owner = match fixture_kind {
            RecoveryFixture::MainEquipmentCod => LegacyAuctionOwner::WinnerEquipmentCod,
            RecoveryFixture::IsekaiCarrier => LegacyAuctionOwner::WinnerPaymentCarrierCod,
            RecoveryFixture::IsekaiEquipmentHandoff => LegacyAuctionOwner::WinnerEquipmentHandoff,
        };
        let (quarantine_id, observation_id) =
            create_quarantine(&database, &repo, owner, isekai).await;
        let deliveries_before = count(pool, "SELECT count(*) FROM public.deliveries").await;
        let receipt = release(
            &repo,
            quarantine_id,
            observation_id,
            auction_domain::consignment::deposit::ReleaseKind::AuctionRecoveryReturn,
            CommandId::from_uuid(Uuid::new_v4()),
        )
        .await
        .expect("恢复腿交接成功");
        assert_eq!(receipt.obligation_id, Some(fixture.obligation_id));
        assert_eq!(receipt.return_mail_id, Some(fixture.return_mail_id));
        assert_eq!(receipt.recovery_id, Some(fixture.recovery_id));
        assert!(receipt.recovery_handoff_id.is_some());
        assert_eq!(
            count(pool, "SELECT count(*) FROM public.settlement_recovery_return_handoffs").await,
            1
        );
        assert_eq!(
            count(pool, "SELECT count(*) FROM public.settlement_recovery_return_consumptions")
                .await,
            0,
            "交接不得预写 consumption"
        );
        assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_recovery_actions").await, 0);
        assert_no_asset_side_effects(pool, deliveries_before).await;
        let advance = runtime
            .record_due_return(Timestamp::now())
            .await
            .expect("旧恢复消费不报错")
            .expect("旧恢复必须消费该封");
        assert_eq!(advance.return_mail_id, fixture.return_mail_id);
        assert_eq!(advance.recovery_id, fixture.recovery_id.into());
        assert!(advance.blocking_code.is_none());
        assert_eq!(advance.action_kinds, vec![fixture.action_kind.to_owned()]);
        assert_eq!(
            count(pool, "SELECT count(*) FROM public.settlement_recovery_return_consumptions")
                .await,
            1
        );
        assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_recovery_actions").await, 1);
        pool.close().await;
    }
}

#[tokio::test]
async fn postgres_handed_off_mismatched_payment_is_consumed_once() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let fixture = seed_recovery(pool, 7_500_030, RecoveryFixture::MainEquipmentCod).await;
    sqlx::query(
        "UPDATE public.inbound_mail
         SET decision_metadata = jsonb_set(decision_metadata, '{attachment_ids}', '[\"999999\"]')
         WHERE id = $1",
    )
    .bind(fixture.return_mail_id)
    .execute(pool)
    .await
    .expect("改写退回资产");
    let repo = repository(&database).await;
    let (quarantine_id, observation_id) =
        create_quarantine(&database, &repo, LegacyAuctionOwner::WinnerEquipmentCod, false).await;
    release(
        &repo,
        quarantine_id,
        observation_id,
        auction_domain::consignment::deposit::ReleaseKind::AuctionRecoveryReturn,
        CommandId::from_uuid(Uuid::new_v4()),
    )
    .await
    .expect("资产不符不是移交门槛");
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.settlement_recovery_return_consumptions").await,
        0
    );
    let advance = runtime
        .record_due_return(Timestamp::now())
        .await
        .expect("旧恢复消费不报错")
        .expect("旧恢复必须消费该封");
    assert_eq!(advance.return_mail_id, fixture.return_mail_id);
    assert_eq!(advance.blocking_code.as_deref(), Some("evidence_mismatch"));
    assert!(advance.action_kinds.is_empty());
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.settlement_recovery_return_consumptions").await,
        1
    );
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_recovery_actions").await, 0);
    pool.close().await;
}

async fn seed_authorizing_case(pool: &PgPool) -> Uuid {
    let mail_id = insert_mail(
        pool,
        "main",
        "999999",
        at("2099-01-01T00:00:00Z"),
        format!("[RTS] Auction seller payout [{}]", Uuid::new_v4()),
        json!({"obligation_id": null, "credits": 0, "cod": 0, "attachment_ids": []}),
    )
    .await;
    sqlx::query(
        "INSERT INTO public.settlement_return_evidence
            (return_mail_id, match_state, reason_code, obligation_id, delivery_id,
             previous_execution_state, recorded_at)
         VALUES ($1, 'unbound', 'evidence_invalid', NULL, NULL, NULL, now())",
    )
    .bind(mail_id)
    .execute(pool)
    .await
    .expect("写入授权案件证据");
    sqlx::query(
        "INSERT INTO public.settlement_return_cases
            (return_mail_id, world_id, obligation_id, delivery_id, dispatch_no, status,
             reason_code, resend_limit, created_at, updated_at)
         VALUES ($1, 'main', NULL, NULL, NULL, 'quarantined', 'evidence_invalid', 3,
                 now(), now())",
    )
    .bind(mail_id)
    .execute(pool)
    .await
    .expect("写入授权案件");
    sqlx::query(
        "INSERT INTO public.settlement_return_events
            (return_mail_id, case_version, event_kind, status, details, occurred_at)
         VALUES ($1, 1, 'opened', 'quarantined', '{}'::jsonb, now())",
    )
    .bind(mail_id)
    .execute(pool)
    .await
    .expect("写入授权案件事件");
    mail_id
}

async fn insert_resend_delivery(
    pool: &PgPool,
    obligation_id: Uuid,
    delivery_id: Uuid,
    sending_at: Timestamp,
) {
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, fencing_version, attempt_count, claimed_at, sending_at, completed_at,
             external_reference, created_at, updated_at, result_metadata,
             activation_command_id, activation_request_digest, activated_at)
         VALUES ($1, $2, decode(repeat('91', 32), 'hex'), 'settlement', $3,
                 'Recovery seller',
                 jsonb_build_object('schema_version', 1, 'operation', 'send', 'world', 'main',
                     'template', 'settlement_seller_payout',
                     'variables', jsonb_build_object('reference', $3),
                     'credits', 90, 'cod', 0, 'equipment_secret_refs', '[]'::jsonb),
                 'succeeded', 1, 1, $4, $4, $4, $5, $4, $4,
                 '{\"operation\":\"send\"}'::jsonb, $6, decode(repeat('92', 32), 'hex'), $4)",
    )
    .bind(delivery_id)
    .bind(Uuid::new_v4())
    .bind(obligation_id.to_string())
    .bind(sending_at)
    .bind(format!("mooglemail:main:send:{}", 680_000_000 + delivery_id.as_u128() % 1_000_000))
    .bind(Uuid::new_v4())
    .execute(pool)
    .await
    .expect("写入补寄投递");
    sqlx::query(
        "INSERT INTO public.settlement_obligation_fulfillments
            (obligation_id, execution_state, state_version, dispatch_world,
             delivery_operation, delivery_id, dispatch_no, prepare_command_id,
             prepare_request_digest, activate_command_id, activate_request_digest,
             completed_at, created_at, updated_at)
         VALUES ($1, 'enqueued', 3, 'main', 'send', $2, 2, $3,
                 decode(repeat('91', 32), 'hex'), $4, decode(repeat('92', 32), 'hex'),
                 NULL, $5, $5)",
    )
    .bind(obligation_id)
    .bind(delivery_id)
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4())
    .bind(sending_at)
    .execute(pool)
    .await
    .expect("写入补寄履约");
}

#[tokio::test]
async fn postgres_resend_without_bound_body_stays_in_original_case() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let (minute, settled_at) = settlement_minute();
    let sale = insert_sale(pool, 7_500_040, settled_at, 1).await;
    let reference =
        SettlementDispatchReference::for_resend(sale.seller_obligation, 2).expect("补寄引用");
    insert_resend_delivery(
        pool,
        sale.seller_obligation,
        reference.delivery_id,
        minute + Duration::minutes(10),
    )
    .await;
    let authorizing_case = seed_authorizing_case(pool).await;
    sqlx::query(
        "INSERT INTO public.settlement_fulfillment_dispatches
            (obligation_id, dispatch_no, delivery_id, authorized_by, recorded_at)
         VALUES ($1, 2, $2, $3, now())",
    )
    .bind(sale.seller_obligation)
    .bind(reference.delivery_id)
    .bind(authorizing_case)
    .execute(pool)
    .await
    .expect("登记补寄派发");
    let return_mail_id = insert_mail(
        pool,
        "main",
        MID,
        minute + Duration::minutes(40),
        format!("[RTS] Auction seller payout [{}]", sale.seller_obligation),
        json!({
            "obligation_id": sale.seller_obligation.to_string(),
            "credits": 90,
            "cod": 0,
            "attachment_ids": [],
            "dispatch_reference": reference.encode(),
        }),
    )
    .await;
    let repo = repository(&database).await;
    let (quarantine_id, observation_id) =
        create_quarantine(&database, &repo, LegacyAuctionOwner::SellerPayout, false).await;
    let receipt = release(
        &repo,
        quarantine_id,
        observation_id,
        auction_domain::consignment::deposit::ReleaseKind::AuctionReturnCase,
        CommandId::from_uuid(Uuid::new_v4()),
    )
    .await
    .expect("缺正文证明仍交还原案");
    assert_eq!(receipt.return_mail_id, Some(return_mail_id));
    let case_row: (String, String, Option<Uuid>, Option<i32>, Option<Uuid>) = sqlx::query_as(
        "SELECT status, reason_code, delivery_id, dispatch_no, obligation_id
         FROM public.settlement_return_cases WHERE return_mail_id = $1",
    )
    .bind(return_mail_id)
    .fetch_one(pool)
    .await
    .expect("读取补寄未绑定案件");
    assert_eq!(
        case_row,
        (
            "quarantined".to_owned(),
            "body_unproven".to_owned(),
            None,
            None,
            Some(sale.seller_obligation)
        )
    );
    let fulfillment: (String, i64) = sqlx::query_as(
        "SELECT execution_state, state_version
         FROM public.settlement_obligation_fulfillments WHERE obligation_id = $1",
    )
    .bind(sale.seller_obligation)
    .fetch_one(pool)
    .await
    .expect("读取未迁移履约");
    assert_eq!(fulfillment, ("enqueued".to_owned(), 3));
    assert_eq!(
        count(
            pool,
            "SELECT count(*) FROM public.settlement_return_evidence WHERE delivery_id IS NOT NULL"
        )
        .await,
        0
    );
    pool.close().await;
}

/// 退件交接前后必须逐项不变的资产外写与资金计数：投递、发送围栏（POST）、Take/Return 工作与起写、
/// 补寄派发登记、寄售代管与资金分录。
const RESEND_ZERO_DELTA: [&str; 8] = [
    "SELECT count(*) FROM public.deliveries",
    "SELECT count(*) FROM public.delivery_send_attempts",
    "SELECT count(*) FROM public.mail_disposition_work",
    "SELECT count(*) FROM public.mail_disposition_write_starts",
    "SELECT count(*) FROM public.settlement_fulfillment_dispatches",
    "SELECT count(*) FROM public.consignment_custody_positions",
    "SELECT count(*) FROM public.settlement_funding_entries",
    "SELECT count(*) FROM public.settlement_funding_returns",
];

async fn resend_zero_delta(pool: &PgPool) -> Vec<i64> {
    let mut counts = Vec::new();
    for query in RESEND_ZERO_DELTA {
        counts.push(count(pool, query).await);
    }
    counts
}

/// 补寄退件带正文摘要（等于该补寄派发发送围栏冻结的摘要）与系统后缀的原收件人 `returned_from`，
/// 经隔离解除交还拍卖原案件；返回案件的 (状态, 原因, 投递, 派发编号)。断言交接前后资产外写与资金计数不变。
async fn resend_return_with_body_digest(
    returned_from: &str,
) -> (String, String, Option<Uuid>, Option<i32>, Uuid) {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let (minute, settled_at) = settlement_minute();
    let sale = insert_sale(pool, 7_500_041, settled_at, 1).await;
    let reference =
        SettlementDispatchReference::for_resend(sale.seller_obligation, 2).expect("补寄引用");
    let sending_at = minute + Duration::minutes(10);
    insert_resend_delivery(pool, sale.seller_obligation, reference.delivery_id, sending_at).await;
    // 补寄派发的发送围栏冻结了正文摘要（32 字节 0x5a）与收件人。
    sqlx::query(
        "INSERT INTO public.delivery_send_attempts
            (delivery_id, fencing_version, command_id, request_digest, world_id, sending_at,
             expectation)
         VALUES ($1, 1, $2, decode(repeat('93', 32), 'hex'), 'main', $3,
                 jsonb_build_object('world_id', 'main', 'recipient', 'Recovery seller',
                     'subject', 'fake', 'body_digest',
                     (SELECT jsonb_agg(90) FROM generate_series(1, 32)), 'credits', 90,
                     'cod', 0, 'equipment_ids', '[]'::jsonb, 'credential', 'null'::jsonb))",
    )
    .bind(reference.delivery_id)
    .bind(Uuid::new_v4())
    .bind(sending_at)
    .execute(pool)
    .await
    .expect("写入补寄发送围栏");
    let authorizing_case = seed_authorizing_case(pool).await;
    sqlx::query(
        "INSERT INTO public.settlement_fulfillment_dispatches
            (obligation_id, dispatch_no, delivery_id, authorized_by, recorded_at)
         VALUES ($1, 2, $2, $3, now())",
    )
    .bind(sale.seller_obligation)
    .bind(reference.delivery_id)
    .bind(authorizing_case)
    .execute(pool)
    .await
    .expect("登记补寄派发");
    let return_mail_id = insert_mail(
        pool,
        "main",
        MID,
        minute + Duration::minutes(40),
        format!("[RTS] Auction seller payout [{}]", sale.seller_obligation),
        json!({
            "obligation_id": sale.seller_obligation.to_string(),
            "credits": 90,
            "cod": 0,
            "attachment_ids": [],
            "returned_from": returned_from,
            "body_digest": "5a".repeat(32),
            "dispatch_reference": reference.encode(),
        }),
    )
    .await;
    let repo = repository(&database).await;
    let (quarantine_id, observation_id) =
        create_quarantine(&database, &repo, LegacyAuctionOwner::SellerPayout, false).await;
    let before = resend_zero_delta(pool).await;
    let receipt = release(
        &repo,
        quarantine_id,
        observation_id,
        auction_domain::consignment::deposit::ReleaseKind::AuctionReturnCase,
        CommandId::from_uuid(Uuid::new_v4()),
    )
    .await
    .expect("交还原案");
    assert_eq!(receipt.return_mail_id, Some(return_mail_id));
    // 交接只建原案件：不新增投递、POST、Take/Return、补寄派发或资金分录。
    assert_eq!(resend_zero_delta(pool).await, before);
    let (status, reason, delivery_id, dispatch_no): (String, String, Option<Uuid>, Option<i32>) =
        sqlx::query_as(
            "SELECT status, reason_code, delivery_id, dispatch_no
             FROM public.settlement_return_cases WHERE return_mail_id = $1",
        )
        .bind(return_mail_id)
        .fetch_one(pool)
        .await
        .expect("读取补寄退件案件");
    pool.close().await;
    (status, reason, delivery_id, dispatch_no, reference.delivery_id)
}

/// 补寄退件的正文摘要在、但系统后缀的原收件人与该派发冻结的收件人不符：正文证明不成立，留在原案件
/// 隔离（`body_unproven`），不绑定任何派发；正例：同一摘要且原收件人相符（大小写不敏感）才绑定补寄派发。
/// 两者交接前后资产外写与资金计数均不变。
///
/// 保护的不变量：4（退件不触发外写）、6（证明不全时失败关闭）。
#[tokio::test]
async fn postgres_resend_digest_with_mismatched_recipient_stays_in_original_case() {
    let (status, reason, delivery_id, dispatch_no, _) =
        resend_return_with_body_digest("SomeoneElse").await;
    assert_eq!(
        (status.as_str(), reason.as_str(), delivery_id, dispatch_no),
        ("quarantined", "body_unproven", None, None)
    );

    let (_, reason, delivery_id, dispatch_no, resend_delivery) =
        resend_return_with_body_digest("recovery SELLER").await;
    assert_ne!(reason, "body_unproven");
    assert_eq!((delivery_id, dispatch_no), (Some(resend_delivery), Some(2)));
}

async fn preinsert_archived_final(pool: &PgPool) -> Uuid {
    let (observation_id, command_id): (Uuid, Uuid) = sqlx::query_as(
        "SELECT observation_id, command_id FROM public.mail_initial_decisions
         WHERE bot_uid = '12345' AND world_id = 'main' AND external_mid = $1",
    )
    .bind(MID)
    .fetch_one(pool)
    .await
    .expect("读取首次隔离决定");
    let decision_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.mail_final_dispositions
            (bot_uid, world_id, external_mid, id, kind, source_observation_id, command_id,
             decided_at)
         VALUES ('12345', 'main', $1, $2, 'archived_empty', $3, $4, now())",
    )
    .bind(MID)
    .bind(decision_id)
    .bind(observation_id)
    .bind(command_id)
    .execute(pool)
    .await
    .expect("预写冲突最终决定");
    decision_id
}

async fn assert_quarantine_active(pool: &PgPool, quarantine_id: Uuid, version: i64) {
    let state: (String, i64) =
        sqlx::query_as("SELECT state, version FROM public.mail_quarantines WHERE id = $1")
            .bind(quarantine_id)
            .fetch_one(pool)
            .await
            .expect("读取隔离状态");
    assert_eq!(state, ("active".to_owned(), version));
}

#[tokio::test]
async fn postgres_original_case_release_race_rolls_back_or_commits_once() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let fixture = seed_buyer_refund(pool, 7_500_050).await;
    let repo = repository(&database).await;
    let (quarantine_id, observation_id) =
        create_quarantine(&database, &repo, LegacyAuctionOwner::BuyerRefund, false).await;
    let preexisting_final = preinsert_archived_final(pool).await;
    let error = release(
        &repo,
        quarantine_id,
        observation_id,
        auction_domain::consignment::deposit::ReleaseKind::AuctionReturnCase,
        CommandId::from_uuid(Uuid::new_v4()),
    )
    .await
    .expect_err("冲突最终决定必须让整笔解除回滚");
    assert!(matches!(error, auction_application::consignment::ConsignmentError::Unavailable));
    assert_quarantine_active(pool, quarantine_id, 2).await;
    assert_eq!(count(pool, "SELECT count(*) FROM public.mail_quarantine_releases").await, 0);
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_evidence").await, 0);
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_cases").await, 0);
    assert_eq!(
        count(
            pool,
            "SELECT count(*) FROM public.audit_records
             WHERE action = 'auction.fulfillment.manual_reconcile'"
        )
        .await,
        0
    );
    let final_rows: (i64, Option<Uuid>) = sqlx::query_as(
        "SELECT count(*), (
             SELECT id FROM public.mail_final_dispositions
             WHERE bot_uid = '12345' AND world_id = 'main' AND external_mid = $1
             LIMIT 1
         )
         FROM public.mail_final_dispositions
         WHERE bot_uid = '12345' AND world_id = 'main' AND external_mid = $1",
    )
    .bind(MID)
    .fetch_one(pool)
    .await
    .expect("读取最终决定");
    assert_eq!(final_rows, (1, Some(preexisting_final)));
    let fulfillment: (String, i64) = sqlx::query_as(
        "SELECT execution_state, state_version
         FROM public.settlement_obligation_fulfillments WHERE obligation_id = $1",
    )
    .bind(fixture.obligation_id)
    .fetch_one(pool)
    .await
    .expect("读取回滚后履约");
    assert_eq!(fulfillment, ("enqueued".to_owned(), 3));
    pool.close().await;

    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let fixture = seed_buyer_refund(pool, 7_500_051).await;
    let repo = repository(&database).await;
    let (quarantine_id, observation_id) =
        create_quarantine(&database, &repo, LegacyAuctionOwner::BuyerRefund, false).await;
    let admin = CommandId::from_uuid(Uuid::new_v4());
    let automatic = CommandId::from_uuid(Uuid::new_v4());
    let (admin_result, automatic_result, consumer_result) = tokio::join!(
        release_retry(
            &repo,
            quarantine_id,
            observation_id,
            auction_domain::consignment::deposit::ReleaseKind::AuctionReturnCase,
            admin,
        ),
        release_retry(
            &repo,
            quarantine_id,
            observation_id,
            auction_domain::consignment::deposit::ReleaseKind::AuctionReturnCase,
            automatic,
        ),
        runtime.consume_due_settlement_return(Timestamp::now()),
    );
    let admin_receipt = admin_result.expect("Admin 解除成功");
    let automatic_receipt = automatic_result.expect("自动复判成功");
    if let Err(error) = consumer_result {
        eprintln!("旧消费者在竞争中回滚：{error}");
    }
    assert_eq!(admin_receipt.release_id, automatic_receipt.release_id);
    assert_eq!(admin_receipt.decision_id, automatic_receipt.decision_id);
    assert_eq!(admin_receipt.return_mail_id, Some(fixture.return_mail_id));
    assert_eq!(automatic_receipt.return_evidence_id, Some(fixture.return_mail_id));
    assert_eq!(count(pool, "SELECT count(*) FROM public.mail_quarantine_releases").await, 1);
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_evidence").await, 1);
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_cases").await, 1);
    assert_eq!(count(pool, "SELECT count(*) FROM public.mail_final_dispositions").await, 1);
    let replay = release(
        &repo,
        quarantine_id,
        observation_id,
        auction_domain::consignment::deposit::ReleaseKind::AuctionReturnCase,
        admin,
    )
    .await
    .expect("同命令重放");
    assert_eq!(replay.release_id, admin_receipt.release_id);
    assert_eq!(replay.return_mail_id, admin_receipt.return_mail_id);
    pool.close().await;
}

#[tokio::test]
async fn postgres_recovery_release_race_rolls_back_or_commits_once() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    seed_recovery(pool, 7_500_060, RecoveryFixture::MainEquipmentCod).await;
    let repo = repository(&database).await;
    let (quarantine_id, observation_id) =
        create_quarantine(&database, &repo, LegacyAuctionOwner::WinnerEquipmentCod, false).await;
    let preexisting_final = preinsert_archived_final(pool).await;
    let error = release(
        &repo,
        quarantine_id,
        observation_id,
        auction_domain::consignment::deposit::ReleaseKind::AuctionRecoveryReturn,
        CommandId::from_uuid(Uuid::new_v4()),
    )
    .await
    .expect_err("冲突最终决定必须让恢复接管回滚");
    assert!(matches!(error, auction_application::consignment::ConsignmentError::Unavailable));
    assert_quarantine_active(pool, quarantine_id, 2).await;
    assert_eq!(count(pool, "SELECT count(*) FROM public.mail_quarantine_releases").await, 0);
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.settlement_recovery_return_handoffs").await,
        0
    );
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.settlement_recovery_return_consumptions").await,
        0
    );
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_recovery_actions").await, 0);
    let final_rows: (i64, Option<Uuid>) = sqlx::query_as(
        "SELECT count(*), (
             SELECT id FROM public.mail_final_dispositions
             WHERE bot_uid = '12345' AND world_id = 'main' AND external_mid = $1
             LIMIT 1
         )
         FROM public.mail_final_dispositions
         WHERE bot_uid = '12345' AND world_id = 'main' AND external_mid = $1",
    )
    .bind(MID)
    .fetch_one(pool)
    .await
    .expect("读取最终决定");
    assert_eq!(final_rows, (1, Some(preexisting_final)));
    pool.close().await;

    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let fixture = seed_recovery(pool, 7_500_061, RecoveryFixture::MainEquipmentCod).await;
    let repo = repository(&database).await;
    let (quarantine_id, observation_id) =
        create_quarantine(&database, &repo, LegacyAuctionOwner::WinnerEquipmentCod, false).await;
    let command_id = CommandId::from_uuid(Uuid::new_v4());
    let (release_result, first_consumer) = tokio::join!(
        release_retry(
            &repo,
            quarantine_id,
            observation_id,
            auction_domain::consignment::deposit::ReleaseKind::AuctionRecoveryReturn,
            command_id,
        ),
        runtime.record_due_return(Timestamp::now()),
    );
    let receipt = release_result.expect("恢复交接成功");
    let mut consumed = match first_consumer {
        Ok(Some(advance)) => Some(advance),
        Ok(None) | Err(_) => None,
    };
    if consumed.is_none() {
        for _ in 0..3 {
            match runtime.record_due_return(Timestamp::now()).await {
                Ok(Some(advance)) => {
                    consumed = Some(advance);
                    break;
                }
                Ok(None) | Err(_) => {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            }
        }
    }
    let advance = consumed.expect("交接后旧恢复必须消费");
    assert_eq!(advance.return_mail_id, fixture.return_mail_id);
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.settlement_recovery_return_handoffs").await,
        1
    );
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.settlement_recovery_return_consumptions").await,
        1
    );
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_recovery_actions").await, 1);
    assert_eq!(count(pool, "SELECT count(*) FROM public.mail_quarantine_releases").await, 1);
    let replay = release(
        &repo,
        quarantine_id,
        observation_id,
        auction_domain::consignment::deposit::ReleaseKind::AuctionRecoveryReturn,
        command_id,
    )
    .await
    .expect("恢复接管重放");
    assert_eq!(replay.recovery_handoff_id, receipt.recovery_handoff_id);
    assert_eq!(replay.recovery_id, receipt.recovery_id);
    pool.close().await;
}

type EvidenceSnapshot =
    (Uuid, String, String, Option<Uuid>, Option<Uuid>, Option<String>, Timestamp);
type CaseSnapshot = (
    Uuid,
    String,
    Option<Uuid>,
    Option<Uuid>,
    Option<i32>,
    String,
    String,
    i16,
    Option<i32>,
    i64,
    i32,
    Option<i64>,
    Timestamp,
    Timestamp,
    Option<Timestamp>,
);
type EventSnapshot = (Uuid, i64, String, String, Option<Uuid>, Option<String>, Value, Timestamp);
type FulfillmentSnapshot = (
    String,
    i64,
    Option<String>,
    Option<Uuid>,
    Option<Timestamp>,
    Option<Timestamp>,
    Option<Timestamp>,
    Option<Timestamp>,
    Timestamp,
);
type AuditSnapshot = (Timestamp, String, String, String, String, String, String, String, Value);

struct ReturnWriteSnapshot {
    evidence: EvidenceSnapshot,
    case: CaseSnapshot,
    events: Vec<EventSnapshot>,
    fulfillment: FulfillmentSnapshot,
    audits: Vec<AuditSnapshot>,
}

async fn return_write_snapshot(
    pool: &PgPool,
    return_mail_id: Uuid,
    obligation_id: Uuid,
) -> ReturnWriteSnapshot {
    let evidence = sqlx::query_as::<_, EvidenceSnapshot>(
        "SELECT return_mail_id, match_state, reason_code, obligation_id, delivery_id,
                previous_execution_state, recorded_at
         FROM public.settlement_return_evidence WHERE return_mail_id = $1",
    )
    .bind(return_mail_id)
    .fetch_one(pool)
    .await
    .expect("读取证据快照");
    let case = sqlx::query_as::<_, CaseSnapshot>(
        "SELECT return_mail_id, world_id, obligation_id, delivery_id, dispatch_no, status,
                reason_code, resend_limit, resend_dispatch_no, case_version, recheck_count,
                verified_version, created_at, updated_at, closed_at
         FROM public.settlement_return_cases WHERE return_mail_id = $1",
    )
    .bind(return_mail_id)
    .fetch_one(pool)
    .await
    .expect("读取案件快照");
    let events = sqlx::query_as::<_, EventSnapshot>(
        "SELECT return_mail_id, case_version, event_kind, status, command_id, actor_id,
                details, occurred_at
         FROM public.settlement_return_events WHERE return_mail_id = $1
         ORDER BY case_version",
    )
    .bind(return_mail_id)
    .fetch_all(pool)
    .await
    .expect("读取事件快照");
    let fulfillment = sqlx::query_as::<_, FulfillmentSnapshot>(
        "SELECT execution_state, state_version, last_error_code, claim_token, claimed_at,
                claim_expires_at, next_attempt_at, completed_at, updated_at
         FROM public.settlement_obligation_fulfillments WHERE obligation_id = $1",
    )
    .bind(obligation_id)
    .fetch_one(pool)
    .await
    .expect("读取履约快照");
    let audits = sqlx::query_as::<_, AuditSnapshot>(
        "SELECT occurred_at, actor_type, actor_id, action, entity_type, entity_id, result,
                source, metadata
         FROM public.audit_records
         WHERE entity_type = 'settlement_obligation' AND entity_id = $1
           AND action = 'auction.fulfillment.manual_reconcile'
         ORDER BY occurred_at",
    )
    .bind(obligation_id.to_string())
    .fetch_all(pool)
    .await
    .expect("读取审计快照");
    ReturnWriteSnapshot { evidence, case, events, fulfillment, audits }
}

#[tokio::test]
async fn postgres_batch_and_handoff_write_identical_return_facts() {
    let minute = at("2026-06-01T00:00:00Z");
    let settled_at = minute + Duration::seconds(30);

    let batch_database = TestDatabase::create().await.expect("克隆批处理库");
    let batch_runtime = batch_database.store().await.expect("批处理运行账号");
    let batch_pool = batch_runtime.postgres_pool();
    seed_base(batch_pool).await;
    let batch_sale = insert_sale(batch_pool, 7_500_070, settled_at, 1).await;
    let _batch_delivery = insert_sent_fulfillment(
        batch_pool,
        batch_sale.refund_obligation,
        "settlement_buyer_refund",
        100,
        minute + Duration::minutes(10),
        "enqueued",
    )
    .await;
    let batch_mail = insert_mail(
        batch_pool,
        "main",
        MID,
        minute + Duration::minutes(40),
        format!("[RTS] Auction buyer refund [{}]", batch_sale.refund_obligation),
        evidence(batch_sale.refund_obligation, 100, &[]),
    )
    .await;

    let handoff_database = TestDatabase::create().await.expect("克隆交接库");
    let handoff_runtime = handoff_database.store().await.expect("交接运行账号");
    let handoff_pool = handoff_runtime.postgres_pool();
    seed_base(handoff_pool).await;
    let handoff_sale = insert_sale(handoff_pool, 7_500_070, settled_at, 1).await;
    let _handoff_delivery = insert_sent_fulfillment(
        handoff_pool,
        handoff_sale.refund_obligation,
        "settlement_buyer_refund",
        100,
        minute + Duration::minutes(10),
        "enqueued",
    )
    .await;
    let handoff_mail = insert_mail(
        handoff_pool,
        "main",
        MID,
        minute + Duration::minutes(40),
        format!("[RTS] Auction buyer refund [{}]", handoff_sale.refund_obligation),
        evidence(handoff_sale.refund_obligation, 100, &[]),
    )
    .await;
    let repo = repository(&handoff_database).await;
    let (quarantine_id, observation_id) =
        create_quarantine(&handoff_database, &repo, LegacyAuctionOwner::BuyerRefund, false).await;
    let receipt = release(
        &repo,
        quarantine_id,
        observation_id,
        auction_domain::consignment::deposit::ReleaseKind::AuctionReturnCase,
        CommandId::from_uuid(Uuid::new_v4()),
    )
    .await
    .expect("交接成功");
    assert_eq!(receipt.return_mail_id, Some(handoff_mail));
    let now: Timestamp = sqlx::query_scalar(
        "SELECT released_at FROM public.mail_quarantine_releases WHERE quarantine_id = $1",
    )
    .bind(quarantine_id)
    .fetch_one(handoff_pool)
    .await
    .expect("读取交接时刻");
    let consumed = batch_runtime
        .consume_due_settlement_return(now)
        .await
        .expect("批处理消费")
        .expect("必须消费");
    assert_eq!(consumed.return_mail_id, batch_mail);
    let batch = return_write_snapshot(batch_pool, batch_mail, batch_sale.refund_obligation).await;
    let handoff =
        return_write_snapshot(handoff_pool, handoff_mail, handoff_sale.refund_obligation).await;
    assert_eq!(
        (&batch.evidence.1, &batch.evidence.2, batch.evidence.5, batch.evidence.6),
        (&handoff.evidence.1, &handoff.evidence.2, handoff.evidence.5, handoff.evidence.6)
    );
    assert_eq!(batch.evidence.3.is_some(), handoff.evidence.3.is_some());
    assert_eq!(batch.evidence.4.is_some(), handoff.evidence.4.is_some());
    assert_eq!(
        (
            &batch.case.1,
            batch.case.4,
            &batch.case.5,
            &batch.case.6,
            batch.case.7,
            batch.case.8,
            batch.case.9,
            batch.case.10,
            batch.case.11,
            batch.case.12,
            batch.case.13,
            batch.case.14,
        ),
        (
            &handoff.case.1,
            handoff.case.4,
            &handoff.case.5,
            &handoff.case.6,
            handoff.case.7,
            handoff.case.8,
            handoff.case.9,
            handoff.case.10,
            handoff.case.11,
            handoff.case.12,
            handoff.case.13,
            handoff.case.14,
        )
    );
    assert_eq!(batch.case.2.is_some(), handoff.case.2.is_some());
    assert_eq!(batch.case.3.is_some(), handoff.case.3.is_some());
    assert_eq!(
        batch
            .events
            .iter()
            .map(|event| (&event.1, &event.2, &event.3, event.4, &event.5, &event.6, event.7))
            .collect::<Vec<_>>(),
        handoff
            .events
            .iter()
            .map(|event| (&event.1, &event.2, &event.3, event.4, &event.5, &event.6, event.7))
            .collect::<Vec<_>>()
    );
    assert_eq!(batch.fulfillment, handoff.fulfillment);
    let normalize = |audit: &AuditSnapshot, obligation: Uuid, mail: Uuid, delivery: Uuid| {
        (
            audit.0,
            audit.1.clone(),
            audit.2.clone(),
            audit.3.clone(),
            audit.4.clone(),
            audit.6.clone(),
            audit.7.clone(),
            audit
                .8
                .to_string()
                .replace(&obligation.to_string(), "<obligation>")
                .replace(&mail.to_string(), "<mail>")
                .replace(&delivery.to_string(), "<delivery>"),
        )
    };
    assert_eq!(
        batch
            .audits
            .iter()
            .map(|audit| normalize(
                audit,
                batch_sale.refund_obligation,
                batch_mail,
                batch.evidence.4.expect("批处理投递"),
            ))
            .collect::<Vec<_>>(),
        handoff
            .audits
            .iter()
            .map(|audit| normalize(
                audit,
                handoff_sale.refund_obligation,
                handoff_mail,
                handoff.evidence.4.expect("交接投递"),
            ))
            .collect::<Vec<_>>()
    );
    batch_pool.close().await;
    handoff_pool.close().await;
}

#[tokio::test]
async fn postgres_recovery_release_requires_unique_evidence_obligation() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let (minute, _) = settlement_minute();
    let unknown = Uuid::new_v4();
    insert_mail(
        pool,
        "main",
        MID,
        minute,
        format!("[RTS] Auction equipment delivery [{unknown}]"),
        evidence(unknown, 0, &[]),
    )
    .await;
    let repo = repository(&database).await;
    let (quarantine_id, observation_id) =
        create_quarantine(&database, &repo, LegacyAuctionOwner::WinnerEquipmentCod, false).await;
    let error = release(
        &repo,
        quarantine_id,
        observation_id,
        ReleaseKind::AuctionRecoveryReturn,
        CommandId::from_uuid(Uuid::new_v4()),
    )
    .await
    .expect_err("证据义务没有对应恢复记录时必须失败关闭");
    assert!(matches!(error, auction_application::consignment::ConsignmentError::Unavailable));
    assert_quarantine_active(pool, quarantine_id, 2).await;
    assert_eq!(count(pool, "SELECT count(*) FROM public.mail_quarantine_releases").await, 0);
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.settlement_recovery_return_handoffs").await,
        0
    );
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.settlement_recovery_return_consumptions").await,
        0
    );
    pool.close().await;

    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let (minute, settled_at) = settlement_minute();
    let sale_a = insert_sale(pool, 7_510_001, settled_at, 1).await;
    let _recovery_a =
        insert_recovery(pool, &sale_a, "main", sale_a.payment_obligation, None, settled_at).await;
    let sale_b = insert_sale(pool, 7_510_002, settled_at, 1).await;
    let _recovery_b = insert_recovery(
        pool,
        &sale_b,
        "isekai",
        sale_b.payment_obligation,
        Some(sale_a.payment_obligation),
        settled_at,
    )
    .await;
    insert_mail(
        pool,
        "main",
        MID,
        minute,
        format!("[RTS] Auction equipment delivery [{}]", sale_a.payment_obligation),
        evidence(sale_a.payment_obligation, 0, &[sale_a.external_equipment_id.as_str()]),
    )
    .await;
    let repo = repository(&database).await;
    let (quarantine_id, observation_id) =
        create_quarantine(&database, &repo, LegacyAuctionOwner::WinnerEquipmentCod, false).await;
    let error = release(
        &repo,
        quarantine_id,
        observation_id,
        ReleaseKind::AuctionRecoveryReturn,
        CommandId::from_uuid(Uuid::new_v4()),
    )
    .await
    .expect_err("同一证据义务命中两条恢复记录时必须失败关闭");
    assert!(matches!(error, auction_application::consignment::ConsignmentError::Unavailable));
    assert_quarantine_active(pool, quarantine_id, 2).await;
    assert_eq!(count(pool, "SELECT count(*) FROM public.mail_quarantine_releases").await, 0);
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.settlement_recovery_return_handoffs").await,
        0
    );
    pool.close().await;
}

#[tokio::test]
async fn postgres_recovery_handoff_rejects_obligation_for_other_leg() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let fixture = seed_recovery(pool, 7_510_010, RecoveryFixture::IsekaiCarrier).await;
    let repo = repository(&database).await;
    let (_, observation_id) =
        create_quarantine(&database, &repo, LegacyAuctionOwner::WinnerPaymentCarrierCod, false)
            .await;
    let wrong_obligation = fixture.equipment_obligation_id.expect("载体腿必须有交割义务");
    let error = repo
        .test_handoff_recovery_return(
            fixture.return_mail_id,
            fixture.recovery_id,
            wrong_obligation,
            observation_id,
        )
        .await
        .expect_err("载体腿传入交割义务必须失败关闭");
    assert!(error.contains("义务与原恢复腿不一致"), "实际错误：{error}");
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.settlement_recovery_return_handoffs").await,
        0
    );
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.settlement_recovery_return_consumptions").await,
        0
    );
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_recovery_actions").await, 0);

    let handoff_id = repo
        .test_handoff_recovery_return(
            fixture.return_mail_id,
            fixture.recovery_id,
            fixture.payment_obligation_id,
            observation_id,
        )
        .await
        .expect("正确的付款义务必须接管成功");
    assert!(!handoff_id.is_nil());
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.settlement_recovery_return_handoffs").await,
        1
    );
    pool.close().await;
}

#[tokio::test]
async fn postgres_recovery_handoff_consumes_quarantined_decision_once() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let fixture = seed_recovery(pool, 7_510_020, RecoveryFixture::MainEquipmentCod).await;
    sqlx::query(
        "UPDATE public.inbound_mail SET decision_code = 'suspected_system_rts' WHERE id = $1",
    )
    .bind(fixture.return_mail_id)
    .execute(pool)
    .await
    .expect("把退件改成隔离后才交给恢复的形状");
    let repo = repository(&database).await;
    let (quarantine_id, observation_id) =
        create_quarantine(&database, &repo, LegacyAuctionOwner::WinnerEquipmentCod, false).await;
    release(
        &repo,
        quarantine_id,
        observation_id,
        ReleaseKind::AuctionRecoveryReturn,
        CommandId::from_uuid(Uuid::new_v4()),
    )
    .await
    .expect("隔离后才交给恢复的退件必须接管成功");
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.settlement_recovery_return_handoffs").await,
        1
    );
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.settlement_recovery_return_consumptions").await,
        0
    );
    let advance = runtime
        .record_due_return(Timestamp::now())
        .await
        .expect("旧恢复消费不报错")
        .expect("旧恢复必须消费接管分支");
    assert_eq!(advance.return_mail_id, fixture.return_mail_id);
    assert!(advance.blocking_code.is_none());
    assert_eq!(advance.action_kinds, vec![fixture.action_kind.to_owned()]);
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.settlement_recovery_return_consumptions").await,
        1
    );
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_recovery_actions").await, 1);
    pool.close().await;
}

#[tokio::test]
async fn postgres_recovery_handoff_for_other_recovery_is_not_consumed() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let fixture = seed_recovery(pool, 7_510_030, RecoveryFixture::MainEquipmentCod).await;
    sqlx::query(
        "UPDATE public.inbound_mail SET decision_code = 'suspected_system_rts' WHERE id = $1",
    )
    .bind(fixture.return_mail_id)
    .execute(pool)
    .await
    .expect("把退件改成只能靠接管分支消费的形状");
    let repo = repository(&database).await;
    let (_, observation_id) =
        create_quarantine(&database, &repo, LegacyAuctionOwner::WinnerEquipmentCod, false).await;
    let (minute, settled_at) = settlement_minute();
    let other_sale = insert_sale(pool, 7_510_031, settled_at, 1).await;
    let other_recovery =
        insert_recovery(pool, &other_sale, "main", other_sale.payment_obligation, None, settled_at)
            .await;
    let unrelated = Uuid::new_v4();
    let other_mail = insert_inbound(
        pool,
        "owner-handoff-other",
        "suspected_system_rts",
        "main",
        MID,
        minute,
        format!("[RTS] Auction equipment delivery [{unrelated}]"),
        evidence(unrelated, 0, &[]),
    )
    .await;
    sqlx::query(
        "INSERT INTO public.settlement_recovery_return_handoffs
            (id, bot_uid, world_id, external_mid, return_mail_id, recovery_id, obligation_id,
             observation_id, accepted_at)
         VALUES ($1, '12345', 'main', $2, $3, $4, $5, $6, now())",
    )
    .bind(Uuid::new_v4())
    .bind(MID)
    .bind(other_mail)
    .bind(other_recovery)
    .bind(other_sale.payment_obligation)
    .bind(observation_id)
    .execute(pool)
    .await
    .expect("写入指向另一条恢复记录的接管");
    let advance = runtime.record_due_return(Timestamp::now()).await.expect("旧恢复消费不报错");
    assert!(advance.is_none(), "接管引用指向别的恢复记录时不得消费");
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.settlement_recovery_return_consumptions").await,
        0
    );
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_recovery_actions").await, 0);
    assert_eq!(
        count(
            pool,
            "SELECT count(*) FROM public.settlement_recoveries WHERE return_mail_id IS NOT NULL"
        )
        .await,
        0
    );
    pool.close().await;
}

#[tokio::test]
async fn postgres_return_case_and_recovery_paths_are_mutually_exclusive() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    seed_recovery(pool, 7_510_040, RecoveryFixture::MainEquipmentCod).await;
    let repo = repository(&database).await;
    let (quarantine_id, observation_id) =
        create_quarantine(&database, &repo, LegacyAuctionOwner::SellerPayout, false).await;
    let error = release(
        &repo,
        quarantine_id,
        observation_id,
        ReleaseKind::AuctionReturnCase,
        CommandId::from_uuid(Uuid::new_v4()),
    )
    .await
    .expect_err("恢复拥有的系统 RTS 不得由案件流程消费");
    assert!(matches!(error, auction_application::consignment::ConsignmentError::Unavailable));
    assert_quarantine_active(pool, quarantine_id, 2).await;
    assert_eq!(count(pool, "SELECT count(*) FROM public.mail_quarantine_releases").await, 0);
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_evidence").await, 0);
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_cases").await, 0);
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.settlement_recovery_return_handoffs").await,
        0
    );
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.settlement_recovery_return_consumptions").await,
        0
    );
    pool.close().await;

    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let fixture = seed_recovery(pool, 7_510_041, RecoveryFixture::MainEquipmentCod).await;
    let repo = repository(&database).await;
    let (quarantine_id, observation_id) =
        create_quarantine(&database, &repo, LegacyAuctionOwner::WinnerEquipmentCod, false).await;
    seed_return_case_evidence(pool, fixture.return_mail_id, fixture.payment_obligation_id).await;
    let error = release(
        &repo,
        quarantine_id,
        observation_id,
        ReleaseKind::AuctionRecoveryReturn,
        CommandId::from_uuid(Uuid::new_v4()),
    )
    .await
    .expect_err("已有案件证据的退件不得交给恢复");
    assert!(matches!(error, auction_application::consignment::ConsignmentError::Unavailable));
    assert_quarantine_active(pool, quarantine_id, 2).await;
    assert_eq!(count(pool, "SELECT count(*) FROM public.mail_quarantine_releases").await, 0);
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.settlement_recovery_return_handoffs").await,
        0
    );
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.settlement_recovery_return_consumptions").await,
        0
    );
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_evidence").await, 1);
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_cases").await, 1);
    let advance = runtime.record_due_return(Timestamp::now()).await.expect("恢复消费不报错");
    assert!(advance.is_none(), "已有案件证据的退件不得由恢复消费");
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.settlement_recovery_return_consumptions").await,
        0
    );
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_recovery_actions").await, 0);
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_evidence").await, 1);
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_cases").await, 1);
    pool.close().await;
}

async fn assert_case_handoff_rejects_observation_mismatch(
    mailbox_key: &str,
    world: &str,
    external_mid: &str,
    foreign_link: bool,
) {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let repo = repository(&database).await;
    let (quarantine_id, observation_id) =
        create_quarantine(&database, &repo, LegacyAuctionOwner::BuyerRefund, false).await;
    let obligation = Uuid::new_v4();
    let mail_id = insert_inbound(
        pool,
        mailbox_key,
        "settlement_recovery_returned",
        world,
        external_mid,
        at("2026-01-01T03:00:00Z"),
        format!("[RTS] Auction buyer refund [{obligation}]"),
        evidence(obligation, 0, &[]),
    )
    .await;
    if foreign_link {
        seed_foreign_link(pool, mail_id, observation_id).await;
    }
    let error = repo
        .test_handoff_return_case(mail_id, observation_id)
        .await
        .expect_err("观察与入站不同源必须失败关闭");
    assert!(!error.is_empty());
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_evidence").await, 0);
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_cases").await, 0);
    assert_quarantine_active(pool, quarantine_id, 2).await;
    pool.close().await;
}

#[tokio::test]
async fn postgres_case_handoff_rejects_observation_mismatch() {
    assert_case_handoff_rejects_observation_mismatch("owner-handoff-world", "isekai", MID, false)
        .await;
    assert_case_handoff_rejects_observation_mismatch("owner-handoff-mid", "main", "777", false)
        .await;
    assert_case_handoff_rejects_observation_mismatch("owner-handoff-bot", "main", MID, true).await;
}

#[tokio::test]
async fn postgres_recovery_handoff_rejects_unproven_source() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let fixture = seed_recovery(pool, 7_510_051, RecoveryFixture::MainEquipmentCod).await;
    sqlx::query("UPDATE public.inbound_mail SET sender = 'FakeSellerOne' WHERE id = $1")
        .bind(fixture.return_mail_id)
        .execute(pool)
        .await
        .expect("改写非系统发件人");
    let repo = repository(&database).await;
    let (quarantine_id, observation_id) =
        create_quarantine(&database, &repo, LegacyAuctionOwner::WinnerEquipmentCod, false).await;
    let error = release(
        &repo,
        quarantine_id,
        observation_id,
        ReleaseKind::AuctionRecoveryReturn,
        CommandId::from_uuid(Uuid::new_v4()),
    )
    .await
    .expect_err("非系统发件人不得交给恢复");
    assert!(matches!(error, auction_application::consignment::ConsignmentError::Unavailable));
    assert_quarantine_active(pool, quarantine_id, 2).await;
    assert_eq!(count(pool, "SELECT count(*) FROM public.mail_quarantine_releases").await, 0);
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.settlement_recovery_return_handoffs").await,
        0
    );
    pool.close().await;

    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let fixture = seed_recovery(pool, 7_510_052, RecoveryFixture::MainEquipmentCod).await;
    sqlx::query("UPDATE public.inbound_mail SET subject = $2 WHERE id = $1")
        .bind(fixture.return_mail_id)
        .bind(format!("[RTS] Auction buyer refund [{}]", fixture.payment_obligation_id))
        .execute(pool)
        .await
        .expect("改写非规范主题");
    let repo = repository(&database).await;
    let (quarantine_id, observation_id) =
        create_quarantine(&database, &repo, LegacyAuctionOwner::WinnerEquipmentCod, false).await;
    let error = release(
        &repo,
        quarantine_id,
        observation_id,
        ReleaseKind::AuctionRecoveryReturn,
        CommandId::from_uuid(Uuid::new_v4()),
    )
    .await
    .expect_err("主题与义务种类不符不得交给恢复");
    assert!(matches!(error, auction_application::consignment::ConsignmentError::Unavailable));
    assert_quarantine_active(pool, quarantine_id, 2).await;
    assert_eq!(count(pool, "SELECT count(*) FROM public.mail_quarantine_releases").await, 0);
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.settlement_recovery_return_handoffs").await,
        0
    );
    pool.close().await;
}

#[tokio::test]
async fn postgres_recovery_handoff_rejects_terminal_bound_or_taken_key() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let fixture = seed_recovery(pool, 7_510_060, RecoveryFixture::MainEquipmentCod).await;
    sqlx::query(
        "UPDATE public.settlement_recoveries
         SET stage = 'completed', completed_at = updated_at
         WHERE id = $1",
    )
    .bind(fixture.recovery_id)
    .execute(pool)
    .await
    .expect("把恢复记录改成已终结");
    let repo = repository(&database).await;
    let (quarantine_id, observation_id) =
        create_quarantine(&database, &repo, LegacyAuctionOwner::WinnerEquipmentCod, false).await;
    let error = release(
        &repo,
        quarantine_id,
        observation_id,
        ReleaseKind::AuctionRecoveryReturn,
        CommandId::from_uuid(Uuid::new_v4()),
    )
    .await
    .expect_err("已终结的恢复记录不得接管退件");
    assert!(matches!(error, auction_application::consignment::ConsignmentError::Unavailable));
    assert_quarantine_active(pool, quarantine_id, 2).await;
    assert_eq!(count(pool, "SELECT count(*) FROM public.mail_quarantine_releases").await, 0);
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.settlement_recovery_return_handoffs").await,
        0
    );
    pool.close().await;

    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let fixture = seed_recovery(pool, 7_510_061, RecoveryFixture::MainEquipmentCod).await;
    let (minute, _) = settlement_minute();
    let unrelated = Uuid::new_v4();
    let other_mail = insert_inbound(
        pool,
        "owner-handoff-bound",
        "settlement_recovery_returned",
        "main",
        "999",
        minute,
        format!("[RTS] Auction equipment delivery [{unrelated}]"),
        evidence(unrelated, 0, &[]),
    )
    .await;
    sqlx::query(
        "UPDATE public.settlement_recoveries
         SET return_mail_id = $2, return_external_mail_id = '999'
         WHERE id = $1",
    )
    .bind(fixture.recovery_id)
    .bind(other_mail)
    .execute(pool)
    .await
    .expect("把恢复记录预绑定到另一封退件");
    let repo = repository(&database).await;
    let (quarantine_id, observation_id) =
        create_quarantine(&database, &repo, LegacyAuctionOwner::WinnerEquipmentCod, false).await;
    let error = release(
        &repo,
        quarantine_id,
        observation_id,
        ReleaseKind::AuctionRecoveryReturn,
        CommandId::from_uuid(Uuid::new_v4()),
    )
    .await
    .expect_err("已绑定另一封退件的恢复记录不得接管");
    assert!(matches!(error, auction_application::consignment::ConsignmentError::Unavailable));
    assert_quarantine_active(pool, quarantine_id, 2).await;
    assert_eq!(count(pool, "SELECT count(*) FROM public.mail_quarantine_releases").await, 0);
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.settlement_recovery_return_handoffs").await,
        0
    );
    pool.close().await;

    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let fixture = seed_recovery(pool, 7_510_062, RecoveryFixture::MainEquipmentCod).await;
    let repo = repository(&database).await;
    let (_, observation_id) =
        create_quarantine(&database, &repo, LegacyAuctionOwner::WinnerEquipmentCod, false).await;
    let (minute, settled_at) = settlement_minute();
    let other_sale = insert_sale(pool, 7_510_063, settled_at, 1).await;
    let other_recovery =
        insert_recovery(pool, &other_sale, "main", other_sale.payment_obligation, None, settled_at)
            .await;
    let unrelated = Uuid::new_v4();
    let other_mail = insert_inbound(
        pool,
        "owner-handoff-taken",
        "suspected_system_rts",
        "main",
        MID,
        minute,
        format!("[RTS] Auction equipment delivery [{unrelated}]"),
        evidence(unrelated, 0, &[]),
    )
    .await;
    sqlx::query(
        "INSERT INTO public.settlement_recovery_return_handoffs
            (id, bot_uid, world_id, external_mid, return_mail_id, recovery_id, obligation_id,
             observation_id, accepted_at)
         VALUES ($1, '12345', 'main', $2, $3, $4, $5, $6, now())",
    )
    .bind(Uuid::new_v4())
    .bind(MID)
    .bind(other_mail)
    .bind(other_recovery)
    .bind(other_sale.payment_obligation)
    .bind(observation_id)
    .execute(pool)
    .await
    .expect("写入同一原信键的另一封接管");
    let error = repo
        .test_handoff_recovery_return(
            fixture.return_mail_id,
            fixture.recovery_id,
            fixture.payment_obligation_id,
            observation_id,
        )
        .await
        .expect_err("同一原信键已接管另一封时必须失败关闭");
    assert!(error.contains("同一原信键已接管另一封退件"), "实际错误：{error}");
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.settlement_recovery_return_handoffs").await,
        1
    );
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.settlement_recovery_return_consumptions").await,
        0
    );
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_evidence").await, 0);
    pool.close().await;
}

#[tokio::test]
async fn postgres_case_release_compatible_replay_keeps_obligation() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let fixture = seed_buyer_refund(pool, 7_510_070).await;
    let repo = repository(&database).await;
    let (quarantine_id, observation_id) =
        create_quarantine(&database, &repo, LegacyAuctionOwner::BuyerRefund, false).await;
    let first_command = CommandId::from_uuid(Uuid::new_v4());
    let first = release(
        &repo,
        quarantine_id,
        observation_id,
        ReleaseKind::AuctionReturnCase,
        first_command,
    )
    .await
    .expect("首次解除成功");
    assert_eq!(first.obligation_id, Some(fixture.obligation_id));
    let replay_command = CommandId::from_uuid(Uuid::new_v4());
    let replay = release(
        &repo,
        quarantine_id,
        observation_id,
        ReleaseKind::AuctionReturnCase,
        replay_command,
    )
    .await
    .expect("不同命令的相容解除重放成功");
    assert_eq!(replay.command_id, replay_command);
    assert_eq!(replay.schema_version, first.schema_version);
    assert_eq!(replay.operation, first.operation);
    assert_eq!(replay.disposition, first.disposition);
    assert_eq!(replay.quarantine_id, first.quarantine_id);
    assert_eq!(replay.release_id, first.release_id);
    assert_eq!(replay.decision_id, first.decision_id);
    assert_eq!(replay.decision_kind, first.decision_kind);
    assert_eq!(replay.listing_changes, first.listing_changes);
    assert_eq!(replay.obligation_id, first.obligation_id);
    assert_eq!(replay.return_mail_id, first.return_mail_id);
    assert_eq!(replay.return_evidence_id, first.return_evidence_id);
    assert_eq!(replay.recovery_id, first.recovery_id);
    assert_eq!(replay.recovery_handoff_id, first.recovery_handoff_id);
    assert_eq!(replay.return_intake_id, first.return_intake_id);
    assert_eq!(replay.original_round, first.original_round);
    assert_eq!(replay.take_work_id, first.take_work_id);
    assert_eq!(replay.take_delivery_id, first.take_delivery_id);
    pool.close().await;
}

#[tokio::test]
async fn postgres_case_handoff_rejects_existing_recovery_facts() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let fixture = seed_buyer_refund(pool, 7_510_080).await;
    let repo = repository(&database).await;
    let (quarantine_id, observation_id) =
        create_quarantine(&database, &repo, LegacyAuctionOwner::BuyerRefund, false).await;
    let (_, settled_at) = settlement_minute();
    let other_sale = insert_sale(pool, 7_510_081, settled_at, 1).await;
    let other_recovery =
        insert_recovery(pool, &other_sale, "main", other_sale.payment_obligation, None, settled_at)
            .await;
    sqlx::query(
        "INSERT INTO public.settlement_recovery_return_handoffs
            (id, bot_uid, world_id, external_mid, return_mail_id, recovery_id, obligation_id,
             observation_id, accepted_at)
         VALUES ($1, '12345', 'main', $2, $3, $4, $5, $6, now())",
    )
    .bind(Uuid::new_v4())
    .bind(MID)
    .bind(fixture.return_mail_id)
    .bind(other_recovery)
    .bind(other_sale.payment_obligation)
    .bind(observation_id)
    .execute(pool)
    .await
    .expect("写入既有恢复接管");
    let error = repo
        .test_handoff_return_case(fixture.return_mail_id, observation_id)
        .await
        .expect_err("已有恢复接管的退件不得由案件消费");
    assert!(!error.is_empty());
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_evidence").await, 0);
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_cases").await, 0);
    assert_quarantine_active(pool, quarantine_id, 2).await;
    pool.close().await;

    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let fixture = seed_buyer_refund(pool, 7_510_082).await;
    let repo = repository(&database).await;
    let (quarantine_id, observation_id) =
        create_quarantine(&database, &repo, LegacyAuctionOwner::BuyerRefund, false).await;
    let (_, settled_at) = settlement_minute();
    let other_sale = insert_sale(pool, 7_510_083, settled_at, 1).await;
    let other_recovery =
        insert_recovery(pool, &other_sale, "main", other_sale.payment_obligation, None, settled_at)
            .await;
    sqlx::query(
        "INSERT INTO public.settlement_recovery_return_consumptions
            (return_mail_id, recovery_id, recorded_at) VALUES ($1, $2, now())",
    )
    .bind(fixture.return_mail_id)
    .bind(other_recovery)
    .execute(pool)
    .await
    .expect("写入既有恢复消费");
    let error = repo
        .test_handoff_return_case(fixture.return_mail_id, observation_id)
        .await
        .expect_err("已有恢复消费的退件不得由案件消费");
    assert!(!error.is_empty());
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_evidence").await, 0);
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_cases").await, 0);
    assert_quarantine_active(pool, quarantine_id, 2).await;
    pool.close().await;
}

/// 隔离 → 旧 SellerPayout 案件交还的竞争：Admin 与自动复判并发只落一份解除、证据与案件，
/// 同命令重放不新增事实；交还不产生资产副作用。保护不变量：同一隔离只被解除一次。
#[tokio::test]
async fn postgres_seller_payout_release_race_commits_once() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let fixture = seed_seller_payout(pool, 7_500_070).await;
    let repo = repository(&database).await;
    let (quarantine_id, observation_id) =
        create_quarantine(&database, &repo, LegacyAuctionOwner::SellerPayout, false).await;
    let deliveries_before = count(pool, "SELECT count(*) FROM public.deliveries").await;
    let admin = CommandId::from_uuid(Uuid::new_v4());
    let automatic = CommandId::from_uuid(Uuid::new_v4());
    let (admin_result, automatic_result) = tokio::join!(
        release_retry(&repo, quarantine_id, observation_id, ReleaseKind::AuctionReturnCase, admin),
        release_retry(
            &repo,
            quarantine_id,
            observation_id,
            ReleaseKind::AuctionReturnCase,
            automatic
        ),
    );
    let admin_receipt = admin_result.expect("Admin 解除成功");
    let automatic_receipt = automatic_result.expect("自动复判得到同一结果");
    assert_eq!(admin_receipt.release_id, automatic_receipt.release_id);
    assert_eq!(admin_receipt.decision_id, automatic_receipt.decision_id);
    assert_eq!(admin_receipt.obligation_id, Some(fixture.obligation_id));
    assert_eq!(admin_receipt.return_mail_id, Some(fixture.return_mail_id));
    assert_eq!(count(pool, "SELECT count(*) FROM public.mail_quarantine_releases").await, 1);
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_evidence").await, 1);
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_cases").await, 1);
    assert_eq!(count(pool, "SELECT count(*) FROM public.mail_final_dispositions").await, 1);
    assert_return_case_written(pool, &fixture, SettlementReturnReason::ReturnedAssetsMatch).await;
    assert_no_asset_side_effects(pool, deliveries_before).await;
    let replay =
        release(&repo, quarantine_id, observation_id, ReleaseKind::AuctionReturnCase, admin)
            .await
            .expect("同命令重放");
    assert_eq!(replay.release_id, admin_receipt.release_id);
    assert_eq!(count(pool, "SELECT count(*) FROM public.mail_quarantine_releases").await, 1);
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_cases").await, 1);
    assert_eq!(count(pool, "SELECT count(*) FROM public.mail_final_dispositions").await, 1);
    pool.close().await;
}

/// 旧 SellerPayout 交还遇到冲突最终决定时整笔回滚、零残留，履约保持 enqueued；
/// 冲突消除后用新命令重试可一次提交。保护不变量：失败的解除不留下半个案件。
#[tokio::test]
async fn postgres_seller_payout_release_rolls_back_then_retry_commits() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let fixture = seed_seller_payout(pool, 7_500_071).await;
    let repo = repository(&database).await;
    let (quarantine_id, observation_id) =
        create_quarantine(&database, &repo, LegacyAuctionOwner::SellerPayout, false).await;
    let deliveries_before = count(pool, "SELECT count(*) FROM public.deliveries").await;
    // 故障注入：测试自建的触发器在案件写入时抛错（解除、决定与证据已先写入同一事务），
    // 之后可原样 DROP；生产约束与触发器不受影响。
    let maintenance = database.maintenance_store().await.expect("维护账号");
    let owner_pool = maintenance.postgres_pool();
    sqlx::query(
        "CREATE FUNCTION public.pit2_inject_case_failure() RETURNS trigger LANGUAGE plpgsql AS
         $$ BEGIN RAISE EXCEPTION 'pit2 injected failure'; END $$",
    )
    .execute(owner_pool)
    .await
    .expect("创建注入函数");
    sqlx::query(
        "CREATE TRIGGER pit2_inject_case_failure BEFORE INSERT ON public.settlement_return_cases
         FOR EACH ROW EXECUTE FUNCTION public.pit2_inject_case_failure()",
    )
    .execute(owner_pool)
    .await
    .expect("创建注入触发器");
    let error = release(
        &repo,
        quarantine_id,
        observation_id,
        ReleaseKind::AuctionReturnCase,
        CommandId::from_uuid(Uuid::new_v4()),
    )
    .await
    .expect_err("案件写入失败必须让整笔解除回滚");
    assert!(matches!(error, auction_application::consignment::ConsignmentError::Unavailable));
    assert_quarantine_active(pool, quarantine_id, 2).await;
    assert_eq!(count(pool, "SELECT count(*) FROM public.mail_final_dispositions").await, 0);
    assert_eq!(count(pool, "SELECT count(*) FROM public.mail_quarantine_releases").await, 0);
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_evidence").await, 0);
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_cases").await, 0);
    assert_eq!(
        count(
            pool,
            "SELECT count(*) FROM public.audit_records
             WHERE action = 'auction.fulfillment.manual_reconcile'"
        )
        .await,
        0
    );
    let fulfillment: (String, i64) = sqlx::query_as(
        "SELECT execution_state, state_version
         FROM public.settlement_obligation_fulfillments WHERE obligation_id = $1",
    )
    .bind(fixture.obligation_id)
    .fetch_one(pool)
    .await
    .expect("读取回滚后履约");
    assert_eq!(fulfillment, ("enqueued".to_owned(), 3));
    assert_no_asset_side_effects(pool, deliveries_before).await;

    // 撤掉注入后用新命令重试：必须一次提交。
    sqlx::query("DROP TRIGGER pit2_inject_case_failure ON public.settlement_return_cases")
        .execute(owner_pool)
        .await
        .expect("撤掉注入触发器");
    let receipt = release(
        &repo,
        quarantine_id,
        observation_id,
        ReleaseKind::AuctionReturnCase,
        CommandId::from_uuid(Uuid::new_v4()),
    )
    .await
    .expect("故障消除后重试提交");
    assert_eq!(receipt.return_mail_id, Some(fixture.return_mail_id));
    assert_eq!(count(pool, "SELECT count(*) FROM public.mail_quarantine_releases").await, 1);
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_return_cases").await, 1);
    assert_return_case_written(pool, &fixture, SettlementReturnReason::ReturnedAssetsMatch).await;
    assert_no_asset_side_effects(pool, deliveries_before).await;
    pool.close().await;
}
