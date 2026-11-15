//! 投递可用性闸门回归：六种通知来源在凭据缺失或账号忙时都必须释放 claim；以及发送前解码
//! 与渲染按模板表放行：表内每个通知都能发出，越界的来源、结果码、key 引用、正文与世界都无效。

use std::{sync::Arc, time::Duration};

use auction_application::admin::AdminCredentialRuntimeWriter;
use auction_application::core::ApplicationFuture;
use auction_application::core::{ApplicationError, RequestHash};
use auction_application::delivery::{DeliveryTransport, DeliveryWorkClaim};
use auction_application::mail::{
    EquipmentRejectionReason, MailDeliveryOrigin, MailLoginKey, MailLoginKeyReader,
    MailNotificationField, MailNotificationTemplate,
};
use auction_test_support::mooglemail::{Fault, MailDouble};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::clients::mooglemail::transport::{decode_payload, equipment_rejection_copy};
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
    let alert_batch = Uuid::new_v4();
    let seller_summary_command = Uuid::new_v4();
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
            format!("operations_alert:{alert_batch}"),
            Uuid::new_v4(),
            send_payload(
                "main",
                &format!("operations-alert:{alert_batch}"),
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
            seller_summary_command,
            send_payload(
                "main",
                &seller_summary_command.to_string(),
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
    let seller_summary_command = Uuid::new_v4();
    vec![
        // 卖方汇总只允许主世界。
        claim(
            "seller_summary",
            "seller_summary:fixture".to_owned(),
            seller_summary_command,
            send_payload(
                "isekai",
                &seller_summary_command.to_string(),
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

/// 渲染登录 key 回信用的固定 canonical key；只存在于本测试进程。
struct FixtureLoginKeys;

impl MailLoginKeyReader for FixtureLoginKeys {
    fn read(&self, _user_id: Uuid) -> ApplicationFuture<'_, MailLoginKey> {
        Box::pin(async {
            MailLoginKey::from_encoded(&"A".repeat(43)).map_err(ApplicationError::from)
        })
    }
}

/// 按来源生成满足 origin_id / external_mail_id 形状的通知 claim，模板相关字段取自模板表，
/// 只让模板约束决定解码结论。
fn bound_claim(
    template: MailNotificationTemplate,
    origin: MailDeliveryOrigin,
    result_code: &str,
) -> DeliveryWorkClaim {
    let binding = template.binding();
    let world = binding.world.unwrap_or("main");
    let command_id = Uuid::new_v4();
    let (origin_id, external_mail_id) = match origin {
        MailDeliveryOrigin::MailIntake => (format!("{world}:4242:send"), "4242".to_owned()),
        MailDeliveryOrigin::SettlementRecoveryAction => {
            (format!("recovery:{}", Uuid::new_v4()), "4242".to_owned())
        }
        MailDeliveryOrigin::OperationsAlert => {
            let batch_id = Uuid::new_v4();
            (format!("operations_alert:{batch_id}"), format!("operations-alert:{batch_id}"))
        }
        MailDeliveryOrigin::LotWatch => (format!("lot_watch:{command_id}"), command_id.to_string()),
        MailDeliveryOrigin::SellerSummary => {
            (format!("seller_summary:{}:alice", Uuid::new_v4()), command_id.to_string())
        }
        MailDeliveryOrigin::SeasonReminder => (
            "season_reminder:1793523600:alice".to_owned(),
            "season-end:2026-11-01T09:00:00Z".to_owned(),
        ),
    };
    let body = (binding.body == MailNotificationField::Required).then_some("Frozen notice body");
    let mut payload = send_payload(world, &external_mail_id, template.as_str(), result_code, body);
    if binding.user_key == MailNotificationField::Required {
        payload["user_key_user_id"] = json!(Uuid::new_v4());
    }
    claim(origin.as_str(), origin_id, command_id, payload)
}

/// 发送前解码逐模板按模板表放行：闭集内每个结果码都能解码，来源、结果码、key 引用、冻结正文
/// 与固定世界任一越界都判为无效载荷。
#[test]
fn send_decoding_follows_the_template_binding() {
    for template in MailNotificationTemplate::ALL {
        let binding = template.binding();
        for code in binding.result_codes {
            let claim = bound_claim(template, binding.origin, code);
            assert!(decode_payload(&claim).is_ok(), "{template:?}/{code}");
        }
        let code = binding.result_codes[0];
        let valid = || bound_claim(template, binding.origin, code);
        let mut invalid = MailDeliveryOrigin::ALL
            .into_iter()
            .filter(|origin| *origin != binding.origin)
            .map(|origin| ("other origin", bound_claim(template, origin, code)))
            .collect::<Vec<_>>();
        let mut claim = valid();
        claim.payload["result_code"] = json!("unknown_result_code");
        invalid.push(("unknown result code", claim));
        let mut claim = valid();
        claim.payload["result_code"] = Value::Null;
        invalid.push(("missing result code", claim));
        let mut claim = valid();
        claim.payload["user_key_user_id"] = match binding.user_key {
            MailNotificationField::Required => Value::Null,
            MailNotificationField::Forbidden => json!(Uuid::new_v4()),
        };
        invalid.push(("flipped user key", claim));
        let mut claim = valid();
        claim.payload["body"] = match binding.body {
            MailNotificationField::Required => Value::Null,
            MailNotificationField::Forbidden => json!("Injected body"),
        };
        invalid.push(("flipped body", claim));
        if binding.user_key == MailNotificationField::Required {
            // 回归：发送端曾只要求存在 key 引用，nil 用户 ID 要到读取 key 时才失败。
            let mut claim = valid();
            claim.payload["user_key_user_id"] = json!(Uuid::nil());
            invalid.push(("nil user key", claim));
        }
        if binding.body == MailNotificationField::Required {
            let mut claim = valid();
            claim.payload["body"] = json!(" padded body ");
            invalid.push(("padded body", claim));
        }
        if let Some(world) = binding.world {
            let mut claim = valid();
            claim.payload["world"] = json!(if world == "main" { "isekai" } else { "main" });
            invalid.push(("other world", claim));
        }
        for (case, claim) in invalid {
            assert!(decode_payload(&claim).is_err(), "{template:?}: {case}");
        }
    }
}

/// 拒收文案表与模板表的结果码闭集相等：闭集内每个结果码都解析为拒收原因，且原因逐项等于
/// 枚举闭集；文案表的键就是枚举（穷举 match），每个原因的说明非空且互不相同。
#[test]
fn equipment_rejection_copy_matches_the_bound_result_codes() {
    let bound = MailNotificationTemplate::AuctionEquipmentRejected
        .binding()
        .result_codes
        .iter()
        .map(|code| EquipmentRejectionReason::from_code(code))
        .collect::<Vec<_>>();
    assert_eq!(bound, EquipmentRejectionReason::ALL.map(Some));
    let mut reasons = std::collections::HashSet::new();
    for reason in EquipmentRejectionReason::ALL {
        let (reason_text, next_steps) = equipment_rejection_copy(reason);
        assert!(!reason_text.is_empty() && !next_steps.is_empty(), "{reason:?} 缺少说明文案");
        assert!(reasons.insert(reason_text), "{reason:?} 的说明与其他原因重复");
    }
}

/// 领取与退回 claim：来源身份由调用方给出，模板字段全空。
fn asset_claim(
    origin: MailDeliveryOrigin,
    operation: &str,
    origin_id: String,
    command_id: Uuid,
    external_mail_id: &str,
    attachment_ids: &[&str],
) -> DeliveryWorkClaim {
    claim(
        origin.as_str(),
        origin_id,
        command_id,
        json!({
            "schema_version": 1,
            "operation": operation,
            "world": "main",
            "external_mail_id": external_mail_id,
            "template": null,
            "result_code": null,
            "body": null,
            "user_key_user_id": null,
            "credits": 0,
            "cod": 0,
            "attachment_ids": attachment_ids,
        }),
    )
}

/// 发送前解码与入队、资产冲突排除同一口径：非规范 UUID、全零与 33 位附件 ID、超长
/// external_mail_id、闭集外的来源 × 操作组合与未由命令 ID 派生的卖方汇总邮件 ID 都是无效载荷；
/// 同形的规范值照常解码。
#[test]
fn send_decoding_rejects_shapes_outside_the_unified_judgement() {
    let recovery_id = Uuid::new_v4();
    let reminder = || {
        let mut claim = bound_claim(
            MailNotificationTemplate::RecoveryPaymentReminder,
            MailDeliveryOrigin::SettlementRecoveryAction,
            "payment_overdue",
        );
        claim.origin_id = format!("recovery:{recovery_id}");
        claim
    };
    assert!(decode_payload(&reminder()).is_ok());
    for origin_id in [
        format!("recovery:{}", recovery_id.to_string().to_uppercase()),
        format!("recovery:{}", recovery_id.simple()),
        format!("recovery:urn:uuid:{recovery_id}"),
        format!("recovery:{}", Uuid::nil()),
    ] {
        let mut claim = reminder();
        claim.origin_id = origin_id;
        assert!(decode_payload(&claim).is_err(), "{}", claim.origin_id);
    }
    let mut alert = bound_claim(
        MailNotificationTemplate::OperationsAlert,
        MailDeliveryOrigin::OperationsAlert,
        "operations_alert",
    );
    assert!(decode_payload(&alert).is_ok());
    let upper_batch = Uuid::new_v4().to_string().to_uppercase();
    alert.origin_id = format!("operations_alert:{upper_batch}");
    alert.payload["external_mail_id"] = json!(format!("operations-alert:{upper_batch}"));
    assert!(decode_payload(&alert).is_err(), "大写批次 UUID 必须判为无效载荷");
    let mut summary = bound_claim(
        MailNotificationTemplate::AuctionSellerSummary,
        MailDeliveryOrigin::SellerSummary,
        "summary",
    );
    assert!(decode_payload(&summary).is_ok());
    summary.payload["external_mail_id"] = json!(Uuid::new_v4().to_string());
    assert!(decode_payload(&summary).is_err(), "卖方汇总邮件 ID 必须由命令 ID 派生");

    let take = |attachments: &[&str]| {
        asset_claim(
            MailDeliveryOrigin::MailIntake,
            "take",
            "main:4242:take".to_owned(),
            Uuid::new_v4(),
            "4242",
            attachments,
        )
    };
    assert!(decode_payload(&take(&["007", &"9".repeat(32)])).is_ok());
    for attachments in [&["000"][..], &["0"], &[&"1".repeat(33)], &["5", "5"]] {
        assert!(decode_payload(&take(attachments)).is_err(), "{attachments:?}");
    }
    for (length, valid) in [(256, true), (257, false)] {
        let mail_id = "7".repeat(length);
        let claim = asset_claim(
            MailDeliveryOrigin::MailIntake,
            "return",
            format!("main:{mail_id}:return"),
            Uuid::new_v4(),
            &mail_id,
            &["4242"],
        );
        assert_eq!(decode_payload(&claim).is_ok(), valid, "{length} 字节 external_mail_id");
    }

    let recovery = format!("recovery:{recovery_id}");
    let recovery_take = asset_claim(
        MailDeliveryOrigin::SettlementRecoveryAction,
        "take",
        recovery.clone(),
        Uuid::new_v4(),
        "4242",
        &["100", "200"],
    );
    assert!(decode_payload(&recovery_take).is_ok());
    let unsorted = asset_claim(
        MailDeliveryOrigin::SettlementRecoveryAction,
        "take",
        recovery.clone(),
        Uuid::new_v4(),
        "4242",
        &["200", "100"],
    );
    assert!(decode_payload(&unsorted).is_err(), "恢复领取附件必须升序");
    let lot_watch = Uuid::new_v4();
    let summary_command = Uuid::new_v4();
    let alert_batch = Uuid::new_v4();
    for (origin, operation, origin_id, command_id, external_mail_id) in [
        (
            MailDeliveryOrigin::SettlementRecoveryAction,
            "return",
            recovery,
            Uuid::new_v4(),
            "4242".to_owned(),
        ),
        (
            MailDeliveryOrigin::LotWatch,
            "take",
            format!("lot_watch:{lot_watch}"),
            lot_watch,
            lot_watch.to_string(),
        ),
        (
            MailDeliveryOrigin::SellerSummary,
            "return",
            format!("seller_summary:{summary_command}:alice"),
            summary_command,
            summary_command.to_string(),
        ),
        (
            MailDeliveryOrigin::SeasonReminder,
            "take",
            "season_reminder:1793523600:alice".to_owned(),
            Uuid::new_v4(),
            "season-end:2026-11-01T09:00:00Z".to_owned(),
        ),
        (
            MailDeliveryOrigin::OperationsAlert,
            "take",
            format!("operations_alert:{alert_batch}"),
            Uuid::new_v4(),
            format!("operations-alert:{alert_batch}"),
        ),
    ] {
        let claim =
            asset_claim(origin, operation, origin_id, command_id, &external_mail_id, &["4242"]);
        assert!(decode_payload(&claim).is_err(), "{origin:?}/{operation}");
    }
}

/// 已删除的论坛回执与其他表外模板一样，发送前解码判为无效载荷。
#[test]
fn retired_forum_notice_is_an_invalid_payload() {
    let mut claim = bound_claim(
        MailNotificationTemplate::AuctionNoAttachments,
        MailDeliveryOrigin::MailIntake,
        "auction_no_attachments",
    );
    claim.payload["template"] = json!("forum_auction_notice");
    claim.payload["result_code"] = json!("forum_auction_notice");
    assert!(decode_payload(&claim).is_err());
}

/// 模板表以发送端实际能发出的集合为准：每个模板的每个结果码都能完成解码与渲染（冻结正文
/// 模板直接发送创建时的正文），表内不允许出现入队成功却只能在发送前判为无效载荷的组合。
#[tokio::test]
async fn every_bound_notification_renders_for_send() {
    let transport = MoogleMailDeliveryTransport::new(
        offline_client(),
        MoogleMailCredentialSet::new(COOKIE).expect("fixture cookie"),
        Arc::new(FixtureLoginKeys),
    );
    for template in MailNotificationTemplate::ALL {
        let binding = template.binding();
        for code in binding.result_codes {
            let rendered =
                transport.render_send(&bound_claim(template, binding.origin, code)).await;
            assert!(matches!(rendered, Ok(Some(_))), "{template:?}/{code}: {:?}", rendered.err());
        }
    }
}
