//! 投递可用性闸门回归：六种通知来源在凭据缺失或账号忙时都必须释放 claim。

use std::{sync::Arc, time::Duration};

use auction_application::admin::AdminCredentialRuntimeWriter;
use auction_application::auction::ApplicationFuture;
use auction_application::core::{ApplicationError, RequestHash};
use auction_application::delivery::{DeliveryTransport, DeliveryWorkClaim};
use auction_application::mail::{MailLoginKey, MailLoginKeyReader};
use auction_test_support::mooglemail::{Fault, MailDouble};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::clients::mooglemail::transport::decode_payload;
use crate::clients::mooglemail::{
    MoogleMailClient, MoogleMailClientConfig, MoogleMailCookie, MoogleMailCredentialSet,
    MoogleMailDeliveryTransport, MoogleMailErrorCategory, MoogleMailWorld,
};

const COOKIE: &str = "ipb_member_id=fixture; ipb_pass_hash=fixture";

/// 闸门判定不读取登录 key；被调用即失败关闭。
struct NoLoginKeys;

impl MailLoginKeyReader for NoLoginKeys {
    fn read(&self, _user_id: Uuid) -> ApplicationFuture<'_, MailLoginKey> {
        Box::pin(async { Err(ApplicationError::CapabilityUnavailable("login key")) })
    }
}

/// 空凭据集合没有可回写的条目；只为满足数据库来源集合的构造前提。
struct NoInvalidation;

impl AdminCredentialRuntimeWriter for NoInvalidation {
    fn mark_invalid(&self, _credential_id: Uuid, _now: DateTime<Utc>) -> ApplicationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

fn client(endpoint: url::Url) -> MoogleMailClient {
    MoogleMailClient::with_config(MoogleMailClientConfig {
        endpoint,
        maximum_response_bytes: 8 * 1024 * 1024,
        minimum_request_interval: Duration::from_millis(1),
        request_timeout: Duration::from_secs(5),
    })
    .expect("loopback client")
}

fn offline_client() -> MoogleMailClient {
    // 闸门只读进程内状态，不发网络；回环地址只为通过 endpoint 校验。
    client(url::Url::parse("http://127.0.0.1:9/").expect("loopback endpoint"))
}

fn transport(
    client: MoogleMailClient,
    credentials: MoogleMailCredentialSet,
) -> MoogleMailDeliveryTransport {
    MoogleMailDeliveryTransport::new(client, credentials, Arc::new(NoLoginKeys))
}

fn claim(
    origin_kind: &str,
    origin_id: String,
    command_id: Uuid,
    payload: Value,
) -> DeliveryWorkClaim {
    let now = Utc::now();
    DeliveryWorkClaim {
        delivery_id: Uuid::new_v4(),
        command_id,
        request_hash: RequestHash::new([7; 32]),
        origin_kind: origin_kind.to_owned(),
        origin_id,
        recipient: "FixtureRecipient".to_owned(),
        payload,
        lease_token: Uuid::new_v4(),
        lease_until: now,
        fencing_version: 1,
        attempt_count: 0,
        worker_id: "fixture-worker".to_owned(),
        claimed_at: now,
        created_at: now,
    }
}

fn send_payload(
    world: &str,
    external_mail_id: &str,
    template: &str,
    result_code: &str,
    body: Option<&str>,
) -> Value {
    json!({
        "schema_version": 1,
        "operation": "send",
        "world": world,
        "external_mail_id": external_mail_id,
        "template": template,
        "result_code": result_code,
        "body": body,
        "user_key_user_id": null,
        "credits": 0,
        "cod": 0,
        "attachment_ids": [],
    })
}

/// 通知来源闭集中每一种来源的合法 payload；顺序与 `is_notification_origin` 一致。
fn notification_claims() -> Vec<DeliveryWorkClaim> {
    let body = Some("fixture notification body");
    let lot_watch_command = Uuid::new_v4();
    vec![
        claim(
            "mail_intake",
            "main:3125357:send".to_owned(),
            Uuid::new_v4(),
            send_payload("main", "3125357", "register_result", "blacklisted", None),
        ),
        claim(
            "settlement_recovery_action",
            format!("recovery:{}", Uuid::new_v4()),
            Uuid::new_v4(),
            send_payload(
                "main",
                "recovery-reminder-fixture",
                "recovery_payment_reminder",
                "payment_overdue",
                None,
            ),
        ),
        claim(
            "operations_alert",
            "operations_alert:fixture".to_owned(),
            Uuid::new_v4(),
            send_payload(
                "main",
                "operations-alert:fixture",
                "operations_alert",
                "operations_alert",
                body,
            ),
        ),
        claim(
            "lot_watch",
            format!("lot_watch:{lot_watch_command}"),
            lot_watch_command,
            send_payload("main", &lot_watch_command.to_string(), "lot_watch", "outbid", body),
        ),
        claim(
            "seller_summary",
            "seller_summary:fixture".to_owned(),
            Uuid::new_v4(),
            send_payload(
                "main",
                "seller-summary:fixture",
                "auction_seller_summary",
                "summary",
                body,
            ),
        ),
        claim(
            "season_reminder",
            "season_reminder:fixture".to_owned(),
            Uuid::new_v4(),
            send_payload(
                "isekai",
                "season-end:2026-11-01T09:00:00Z",
                "isekai_season_reminder",
                "season_end",
                body,
            ),
        ),
    ]
}

/// 非法载荷必须进入正式发送路径确定失败，闸门不能把它们永久留在 pending。
fn invalid_claims() -> Vec<DeliveryWorkClaim> {
    vec![
        // 卖方汇总只允许主世界。
        claim(
            "seller_summary",
            "seller_summary:fixture".to_owned(),
            Uuid::new_v4(),
            send_payload(
                "isekai",
                "seller-summary:fixture",
                "auction_seller_summary",
                "summary",
                Some("fixture notification body"),
            ),
        ),
        // 换季提醒必须携带冻结正文。
        claim(
            "season_reminder",
            "season_reminder:fixture".to_owned(),
            Uuid::new_v4(),
            send_payload(
                "isekai",
                "season-end:2026-11-01T09:00:00Z",
                "isekai_season_reminder",
                "season_end",
                None,
            ),
        ),
        // 换季提醒的冻结边界必须是秒精度 UTC 时刻，与入队校验和发送围栏同口径。
        claim(
            "season_reminder",
            "season_reminder:fixture".to_owned(),
            Uuid::new_v4(),
            send_payload(
                "isekai",
                "season-end:fixture",
                "isekai_season_reminder",
                "season_end",
                Some("fixture notification body"),
            ),
        ),
        claim("unknown_origin", "fixture".to_owned(), Uuid::new_v4(), json!({})),
    ]
}

/// 旧结算 Return 不依赖凭据：闸门放行后由正式路径隔离为人工核对。
fn settlement_return_claim() -> DeliveryWorkClaim {
    claim(
        "settlement",
        Uuid::new_v4().to_string(),
        Uuid::new_v4(),
        json!({
            "schema_version": 1,
            "operation": "return",
            "world": "main",
            "external_mail_id": "3125357",
        }),
    )
}

fn assert_fixtures_decode(claims: &[DeliveryWorkClaim]) {
    for claim in claims {
        assert!(decode_payload(claim).is_ok(), "{} 夹具必须是合法 payload", claim.origin_kind);
    }
}

#[test]
fn gate_admits_every_notification_origin_when_credentials_and_account_are_ready() {
    let claims = notification_claims();
    assert_fixtures_decode(&claims);
    let transport =
        transport(offline_client(), MoogleMailCredentialSet::new(COOKIE).expect("fixture cookie"));

    for claim in &claims {
        assert!(transport.available(claim), "{} 在凭据就绪时应放行", claim.origin_kind);
    }
}

#[test]
fn gate_releases_every_notification_origin_without_credentials() {
    let claims = notification_claims();
    assert_fixtures_decode(&claims);
    let credentials = MoogleMailCredentialSet::from_runtime_credentials(
        Vec::new(),
        Arc::new(NoInvalidation),
        None,
    )
    .expect("empty runtime credential set");
    let transport = transport(offline_client(), credentials);

    for claim in &claims {
        assert!(!transport.available(claim), "{} 在凭据缺失时必须释放 claim", claim.origin_kind);
    }
    for claim in invalid_claims() {
        assert!(transport.available(&claim), "非法 {} 必须进入正式失败路径", claim.origin_kind);
    }
    assert!(transport.available(&settlement_return_claim()));
}

#[tokio::test]
async fn gate_releases_every_notification_origin_while_account_is_busy() {
    let claims = notification_claims();
    assert_fixtures_decode(&claims);
    let server = MailDouble::start().await.expect("loopback server");
    let client = client(server.endpoint());
    let transport =
        transport(client.clone(), MoogleMailCredentialSet::new(COOKIE).expect("fixture cookie"));
    let cookie = MoogleMailCookie::new(COOKIE).expect("fixture cookie");
    server.set_fault(Some(Fault::AccountBusy));
    // 两个世界各命中一次战斗页，打开各自的熔断窗口；克隆客户端共享同一熔断状态。
    for world in [MoogleMailWorld::Main, MoogleMailWorld::Isekai] {
        let target = client.inbox_page(world, 0).expect("inbox target");
        let error = client.get(&target, &cookie).await.expect_err("battle page");
        assert_eq!(error.category, MoogleMailErrorCategory::AccountBusy);
    }
    let requests = server.read_requests();

    for claim in &claims {
        assert!(!transport.available(claim), "{} 在账号忙时必须释放 claim", claim.origin_kind);
    }
    for claim in invalid_claims() {
        assert!(transport.available(&claim), "非法 {} 必须进入正式失败路径", claim.origin_kind);
    }
    assert!(transport.available(&settlement_return_claim()));
    // 闸门只读进程内熔断状态，不能消耗窗口内的只读探测。
    assert_eq!(server.read_requests(), requests);
}
