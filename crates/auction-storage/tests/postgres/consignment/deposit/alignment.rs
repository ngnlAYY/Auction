//! 四层对齐回归：4A 构造回执、4M 落库、4D1 解码处分载荷、4W 严格解码同一份事实。

use auction_application::consignment::{
    ListingStateChange, ReleaseReceipt, ReservationReceipt, WithdrawDepositReceipt,
    WithdrawDisposition,
};
use auction_contracts::{
    ConsignmentDispositionV2, ConsignmentOperationV2, DepositReservationReceipt, ListingStateV2,
    QuarantineReleaseReceipt, WithdrawReceiptV2,
};
use auction_domain::CommandId;
use auction_domain::consignment::ListingState;
use auction_domain::consignment::ListingVersion;
use auction_domain::consignment::deposit::ReleaseKind;
use auction_integrations::mooglemail::{
    DispositionAction, ExpectedAttachmentPayload, ExpectedMailManifestPayload, MailDeliveryPayload,
    MailKeyPayload,
};
use auction_storage::TestDepositReceipt;
use auction_time::Timestamp;
use serde_json::Value;
use uuid::Uuid;

use super::support::{
    ACCEPTED, ISSUED, LISTING, START, USER_1, execute, listing_awaiting_deposit, mail_session,
    users_and_key,
};
use crate::support::TestDatabase;

const RECEIPT_COMMAND: &str = "d0000000-0000-4000-8000-000000000001";
const RECEIPT_RESERVATION: &str = "d0000000-0000-4000-8000-000000000002";
const DECISION: &str = "d0000000-0000-4000-8000-000000000003";
const WORK: &str = "d0000000-0000-4000-8000-000000000004";
const DELIVERY: &str = "d0000000-0000-4000-8000-000000000005";
const DELIVERY_COMMAND: &str = "d0000000-0000-4000-8000-000000000006";
const ATTEMPT: &str = "d0000000-0000-4000-8000-000000000007";
const WITHDRAW_REQUESTED_COMMAND: &str = "d0000000-0000-4000-8000-00000000000b";
const WITHDRAW_CLOSED_COMMAND: &str = "d0000000-0000-4000-8000-00000000000c";
const INTENT: &str = "d0000000-0000-4000-8000-00000000000d";
const RELEASE_COMMAND: &str = "d0000000-0000-4000-8000-00000000000e";
const QUARANTINE: &str = "d0000000-0000-4000-8000-00000000000f";
const RELEASE: &str = "d0000000-0000-4000-8000-000000000010";
const RELEASE_DECISION: &str = "d0000000-0000-4000-8000-000000000011";

/// 4A 的回执必须同时通过 4M 的 `c4_valid_v2_receipt` 与 4W 的严格解码；4D1 的寄售
/// 处置载荷必须同时通过 4M 的 `c4_delivery_consignment_payload_ck` 与 4D1 的解码。
#[tokio::test]
async fn postgres_consignment_v2_receipt_and_disposition_payload_align_across_layers() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    execute(pool, &users_and_key()).await.expect("写入用户与重放钥");
    execute(pool, &listing_awaiting_deposit()).await.expect("写入待寄挂单");
    execute(pool, &mail_session()).await.expect("写入邮件会话");

    let expires_at = Timestamp::parse_from_rfc3339("2026-01-01T12:00:00Z").expect("固定截止");
    let reservation = ReservationReceipt::issued(
        CommandId::from_uuid(Uuid::parse_str(RECEIPT_COMMAND).expect("规范命令 UUID")),
        Uuid::parse_str(LISTING).expect("规范挂单 UUID"),
        ListingVersion::new(3).expect("正版本"),
        ListingState::AwaitingDeposit,
        Uuid::parse_str(RECEIPT_RESERVATION).expect("规范预约 UUID"),
        expires_at,
    );
    let reservation_body = TestDepositReceipt::Reservation(reservation).body();
    let decoded: DepositReservationReceipt =
        serde_json::from_value(reservation_body.clone()).expect("4W 严格解码预约回执");
    assert_eq!(decoded.schema_version, 2);
    assert_eq!(decoded.operation, ConsignmentOperationV2::IssueReservation);
    assert_eq!(decoded.disposition, ConsignmentDispositionV2::ReservationIssued);
    assert_eq!(decoded.state, ListingStateV2::AwaitingDeposit);

    execute(
        pool,
        &format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_id, listing_id, result_status, result_code,
                 result_body, created_at)
             VALUES ('{RECEIPT_COMMAND}', 2, 'public_user', '{USER_1}', 'issue_reservation',
                     '{LISTING}', decode(repeat('d1', 32), 'hex'), 'listing', '{LISTING}',
                     '{LISTING}', 201, 'ok', '{reservation_body}', '{ISSUED}');
             INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{LISTING}', 3, '{RECEIPT_COMMAND}', 'public_user', 'reservation_issued',
                     'awaiting_deposit', 'awaiting_deposit', NULL, 101, '{ISSUED}');
             UPDATE public.consignment_listings
             SET state_version=3, updated_at='{ISSUED}'
             WHERE id='{LISTING}';"
        ),
    )
    .await
    .expect("4M 接受 4A 预约回执");

    let withdrawal_requested = WithdrawDepositReceipt::new(
        CommandId::from_uuid(
            Uuid::parse_str(WITHDRAW_REQUESTED_COMMAND).expect("规范撤回命令 UUID"),
        ),
        WithdrawDisposition::WithdrawalRequested,
        Uuid::parse_str(LISTING).expect("规范挂单 UUID"),
        ListingVersion::new(4).expect("正版本"),
        ListingState::DepositAccepted,
        Some(Uuid::parse_str(INTENT).expect("规范意愿 UUID")),
    );
    withdrawal_requested.validate_shape().expect("4A 撤回意愿回执形状");
    let withdrawal_requested_body = TestDepositReceipt::Withdraw(withdrawal_requested).body();
    let decoded: WithdrawReceiptV2 =
        serde_json::from_value(withdrawal_requested_body.clone()).expect("4W 严格解码撤回意愿回执");
    match decoded {
        WithdrawReceiptV2::WithdrawalRequested {
            schema_version,
            operation,
            state,
            intent_id,
            ..
        } => {
            assert_eq!(schema_version, 2);
            assert_eq!(operation, ConsignmentOperationV2::Withdraw);
            assert_eq!(state, ListingStateV2::DepositAccepted);
            assert_eq!(intent_id, Uuid::parse_str(INTENT).expect("规范意愿 UUID"));
        }
        other => panic!("撤回意愿回执不得解成其他处置：{other:?}"),
    }
    execute(
        pool,
        &format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_id, listing_id, result_status, result_code,
                 result_body, created_at)
             VALUES ('{WITHDRAW_REQUESTED_COMMAND}', 2, 'public_user', '{USER_1}', 'withdraw',
                     '{LISTING}', decode(repeat('d5', 32), 'hex'), 'listing', '{LISTING}',
                     '{LISTING}', 202, 'ok', '{withdrawal_requested_body}', '{ISSUED}');"
        ),
    )
    .await
    .expect("4M 接受 4A 撤回意愿回执");

    let withdrawal_closed = WithdrawDepositReceipt::new(
        CommandId::from_uuid(Uuid::parse_str(WITHDRAW_CLOSED_COMMAND).expect("规范关闭命令 UUID")),
        WithdrawDisposition::Closed,
        Uuid::parse_str(LISTING).expect("规范挂单 UUID"),
        ListingVersion::new(4).expect("正版本"),
        ListingState::Closed,
        None,
    );
    withdrawal_closed.validate_shape().expect("4A 未代管撤回回执形状");
    let withdrawal_closed_body = TestDepositReceipt::Withdraw(withdrawal_closed).body();
    let decoded: WithdrawReceiptV2 =
        serde_json::from_value(withdrawal_closed_body.clone()).expect("4W 严格解码关闭回执");
    match decoded {
        WithdrawReceiptV2::Closed { schema_version, operation, state, .. } => {
            assert_eq!(schema_version, 2);
            assert_eq!(operation, ConsignmentOperationV2::Withdraw);
            assert_eq!(state, ListingStateV2::Closed);
        }
        other => panic!("关闭回执不得解成其他处置：{other:?}"),
    }
    execute(
        pool,
        &format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_id, listing_id, result_status, result_code,
                 result_body, created_at)
             VALUES ('{WITHDRAW_CLOSED_COMMAND}', 2, 'public_user', '{USER_1}', 'withdraw',
                     '{LISTING}', decode(repeat('d6', 32), 'hex'), 'listing', '{LISTING}',
                     '{LISTING}', 200, 'ok', '{withdrawal_closed_body}', '{ISSUED}');
             INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{LISTING}', 4, '{WITHDRAW_CLOSED_COMMAND}', 'public_user', 'closed',
                     'awaiting_deposit', 'closed', 'seller_cancelled', 101, '{ISSUED}');
             UPDATE public.consignment_listings
             SET state='closed', reason_code='seller_cancelled', state_version=4,
                 updated_at='{ISSUED}'
             WHERE id='{LISTING}';
             UPDATE public.consignment_deposit_reservations
             SET state='retired', retired_at='{ISSUED}', reason='cancelled',
                 last_command_id='{WITHDRAW_CLOSED_COMMAND}'
             WHERE id='{RECEIPT_RESERVATION}';"
        ),
    )
    .await
    .expect("4M 接受 4A 关闭回执");

    let mut release = ReleaseReceipt::released(
        CommandId::from_uuid(Uuid::parse_str(RELEASE_COMMAND).expect("规范解除命令 UUID")),
        Uuid::parse_str(QUARANTINE).expect("规范隔离 UUID"),
        Uuid::parse_str(RELEASE).expect("规范解除 UUID"),
        Uuid::parse_str(RELEASE_DECISION).expect("规范决定 UUID"),
        ReleaseKind::Accept,
    );
    release.listing_changes = vec![ListingStateChange {
        listing_id: Uuid::parse_str(LISTING).expect("规范挂单 UUID"),
        listing_version: 6,
        state: ListingState::DepositAccepted,
    }];
    release.validate_shape().expect("4A 解除隔离接受回执形状");
    let release_body = TestDepositReceipt::Release(release).body();
    let decoded: QuarantineReleaseReceipt =
        serde_json::from_value(release_body.clone()).expect("4W 严格解码解除隔离回执");
    match decoded {
        QuarantineReleaseReceipt::Accept {
            schema_version,
            operation,
            disposition,
            listing_changes,
            ..
        } => {
            assert_eq!(schema_version, 2);
            assert_eq!(operation, ConsignmentOperationV2::ReleaseQuarantine);
            assert_eq!(disposition, ConsignmentDispositionV2::QuarantineReleased);
            assert_eq!(listing_changes.len(), 1);
            assert_eq!(listing_changes[0].listing_id, Uuid::parse_str(LISTING).expect("挂单 UUID"));
            assert_eq!(listing_changes[0].state, ListingStateV2::DepositAccepted);
            assert_eq!(listing_changes[0].listing_version.get(), 6);
        }
        _ => panic!("解除隔离接受回执必须解成 accept"),
    }
    execute(
        pool,
        &format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_id, listing_id, result_status, result_code,
                 result_body, created_at)
             VALUES ('{RELEASE_COMMAND}', 2, 'admin', 'fake-admin', 'release_quarantine',
                     '{QUARANTINE}', decode(repeat('d7', 32), 'hex'), 'quarantine',
                     '{QUARANTINE}', NULL, 200, 'ok', '{release_body}', '{ACCEPTED}');"
        ),
    )
    .await
    .expect("4M 接受 4A 解除隔离回执");

    let payload = serde_json::to_value(MailDeliveryPayload {
        schema_version: 2,
        operation: DispositionAction::Take.as_str().to_owned(),
        world: "main".to_owned(),
        external_mail_id: "555".to_owned(),
        template: None,
        result_code: None,
        body: None,
        user_key_user_id: None,
        credits: 0,
        cod: 0,
        attachment_ids: vec!["42".to_owned()],
        mail_key: Some(MailKeyPayload {
            bot_uid: "12345".to_owned(),
            world_id: "main".to_owned(),
            external_mid: "555".to_owned(),
        }),
        decision_id: Some(Uuid::parse_str(DECISION).expect("规范决定 UUID")),
        work_id: Some(Uuid::parse_str(WORK).expect("规范工作 UUID")),
        attempt_id: Some(Uuid::parse_str(ATTEMPT).expect("规范尝试 UUID")),
        expected_manifest: Some(ExpectedMailManifestPayload {
            sender: "FakeSellerOne".to_owned(),
            recipient: "FakeBot".to_owned(),
            subject: "consign".to_owned(),
            body_digest: "66".repeat(32),
            manifest_digest: "68".repeat(32),
            credits: 0,
            cod: 0,
            received_minute_raw: "2026-01-01 03:00".to_owned(),
            attachments: vec![ExpectedAttachmentPayload {
                ordinal: 1,
                name: "Fixture Blade".to_owned(),
                canonical_eid: Some("42".to_owned()),
            }],
        }),
        expected_return: None,
    })
    .expect("序列化 4D1 寄售处置载荷");
    execute(
        pool,
        &format!(
            "INSERT INTO public.deliveries
                (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
                 status, fencing_version, attempt_count, created_at, updated_at)
             VALUES ('{DELIVERY}', '{DELIVERY_COMMAND}', decode(repeat('d2', 32), 'hex'),
                     'consignment_disposition', 'consignment:{DECISION}:{WORK}', 'FakeBot',
                     '{payload}'::jsonb, 'prepared', 1, 0, '{START}', '{START}');"
        ),
    )
    .await
    .expect("4M 接受 4D1 处分载荷");

    let claim = auction_application::delivery::DeliveryWorkClaim {
        delivery_id: Uuid::parse_str(DELIVERY).expect("规范投递 UUID"),
        command_id: Uuid::parse_str(DELIVERY_COMMAND).expect("规范投递命令 UUID"),
        request_hash: auction_application::core::RequestHash::new([0xd3; 32]),
        origin_kind: "consignment_disposition".to_owned(),
        origin_id: format!("consignment:{DECISION}:{WORK}"),
        recipient: "FakeBot".to_owned(),
        payload: payload.clone(),
        lease_token: Uuid::parse_str("d0000000-0000-4000-8000-000000000008").expect("租约 UUID"),
        lease_until: Timestamp::parse_from_rfc3339("2026-01-01T03:00:00Z").expect("租约截止"),
        fencing_version: 1,
        attempt_count: 0,
        worker_id: "fixture-worker".to_owned(),
        claimed_at: Timestamp::parse_from_rfc3339("2026-01-01T03:00:00Z").expect("领取时刻"),
        created_at: Timestamp::parse_from_rfc3339("2026-01-01T03:00:00Z").expect("创建时刻"),
    };
    let request = auction_integrations::mooglemail::decode_consignment_disposition_request(&claim)
        .expect("4D1 解码同一份载荷");
    assert_eq!(request.action(), DispositionAction::Take);

    let mut broken = payload;
    broken["decision_id"] = Value::String("not-a-uuid".to_owned());
    let error = execute(
        pool,
        &format!(
            "INSERT INTO public.deliveries
                (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
                 status, fencing_version, attempt_count, created_at, updated_at)
             VALUES ('d0000000-0000-4000-8000-000000000009',
                     'd0000000-0000-4000-8000-00000000000a',
                     decode(repeat('d4', 32), 'hex'), 'consignment_disposition',
                     'consignment:{DECISION}:{WORK}', 'FakeBot', '{broken}'::jsonb,
                     'prepared', 1, 0, '{START}', '{START}');"
        ),
    )
    .await
    .expect_err("非规范 decision_id 必须被 4M 拒绝");
    let error = error.as_database_error().expect("期望服务端 SQLSTATE");
    assert_eq!(error.code().as_deref(), Some("23514"));
    assert_eq!(error.constraint(), Some("c4_delivery_consignment_payload_ck"));
}
