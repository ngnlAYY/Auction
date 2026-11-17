//! 结算退件 Take 的外部时序（MoogleMail 回环替身 + 真实投递 transport）：第一次领取 POST 在客户端
//! 超时后才在远端生效（结果未知），巡检重排后的第二次执行与之交错。
//!
//! - 旧 POST 在新预检读取之后才生效：新预检看到附件仍在并发出第二次 POST，但远端附件只被取走
//!   一次，第二次以领取后页面的回执证明完成——本地只会消费这一份证明（领回与返还各一次由存储
//!   用例覆盖）；
//! - 旧 POST 在新预检之前生效：新预检看到附件已取走、不可领取，失败关闭且不再发出 POST；
//! - 第一次执行在领取 POST 已发出、远端尚未回应时被投递 Worker 放弃（`request_timeout` 在 select
//!   中落选）：被放弃的 POST 由客户端走完并持有在途锁，重排后的新预检等远端回应后才读取，看到附件
//!   已取走即失败关闭，同样不发出第二次 POST。
//!
//! 另有一条不涉及时序的用例：执行会话里同一 MID 不是原退件（再次领回前凭据换成了另一个账号）时，
//! 预检失败关闭，不发出领取 POST。
//!
//! 页面取自已脱敏的真实已领取详情（taken-3125358），只替换发件人、主题与正文；领取前页面另按
//! 已脱敏 Sent 详情（sentdetail-3125364）的附件列表结构写入 90 Credits 与领取按钮。结算退件 Take
//! 只以持久凭据身份执行，凭据集合按数据库来源（持久 ID、版本与运行 owner）建立。

// 测试夹具的固定值失败即测试失败；不对测试装配伪造回退。
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;

use auction_application::admin::{AdminCredentialRuntimeOwnership, AdminCredentialRuntimeWriter};
use auction_application::core::{ApplicationError, ApplicationFuture, RequestHash};
use auction_application::delivery::{DeliveryTransport, DeliveryWorkClaim};
use auction_application::mail::{MailLoginKey, MailLoginKeyReader};
use auction_application::settlement::MailRuntimeBinding;
use auction_integrations::mooglemail::{
    MoogleMailClient, MoogleMailClientConfig, MoogleMailCookie, MoogleMailCredentialSet,
    MoogleMailDeliveryTransport, MoogleMailRuntimeCredential,
};
use auction_test_support::mooglemail::{Fault, MailDouble};
use auction_time::Timestamp;
use serde_json::json;
use uuid::Uuid;

const TAKEN: &str = include_str!("fixtures/mooglemail/taken-3125358.html");
const MID: &str = "3127001";
const ORIGINAL: &str = "Your auction payout is attached.\n\nAmount: 90 Credits";
const SUFFIX: &str = "\n\nThis message was returned from Recovery seller, kupo!";

struct NoLoginKeys;
impl MailLoginKeyReader for NoLoginKeys {
    fn read(&self, _user_id: Uuid) -> ApplicationFuture<'_, MailLoginKey> {
        Box::pin(async { Err(ApplicationError::CapabilityUnavailable("login key")) })
    }
}

/// 本用例不触发远端登录失效；回写只为满足数据库来源集合的构造前提。
struct NoInvalidation;
impl AdminCredentialRuntimeWriter for NoInvalidation {
    fn mark_invalid(&self, _credential_id: Uuid, _now: Timestamp) -> ApplicationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

/// 始终有效的邮件运行 owner（持久 generation 1），满足冻结 Take 凭据身份的前提。
struct ValidOwner;
impl AdminCredentialRuntimeOwnership for ValidOwner {
    fn check(&self) -> ApplicationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }

    fn is_valid(&self) -> bool {
        true
    }

    fn mail_runtime_binding(&self) -> Option<MailRuntimeBinding> {
        Some(MailRuntimeBinding { generation: 1 })
    }

    fn revoke_internal_use(&self) {}
}

fn subject() -> String {
    "[RTS] Auction seller payout [5b1c2a9e-1f0d-4c6b-9a7e-3d2f1e0c9b8a]".to_owned()
}

/// 真实已领取详情页，只替换发件人、主题与正文。
fn with_body(body: &str) -> String {
    let start = TAKEN.find("<textarea").expect("正文开始");
    let open_end = start + TAKEN[start..].find('>').expect("正文标签结束") + 1;
    let close = TAKEN.find("</textarea>").expect("正文结束");
    format!("{}{body}{}", &TAKEN[..open_end], &TAKEN[close..])
        .replacen("value=\"Pnfcgarl\"", "value=\"MoogleMail\"", 1)
        .replacen("value=\"auction\"", &format!("value=\"{}\"", subject()), 1)
}

/// 领取前：退件正文、90 Credits 附件与领取按钮，脚本声明一条可领取附件。
fn claimable_page() -> String {
    with_body(&format!("{ORIGINAL}{SUFFIX}"))
        .replacen(
            "<div id=\"mmail_attachbuttons\"></div>",
            "<div id=\"mmail_attachbuttons\"><img src=\"/y/mmail/attach_take.png\" \
             onclick=\"mooglemail.remove_attachment(0)\" /></div>",
            1,
        )
        .replacen(
            "<div id=\"mmail_attachinfo\">\n\t\t</div>",
            "<div id=\"mmail_attachinfo\"><div id=\"mmail_attachcount\"><div class=\"fc4 fac fcb\">\
             <div>1 / 10 items attached</div></div></div><div id=\"mmail_attachlist\"><div>\
             <div>90 Credits</div><div></div></div></div></div>",
            1,
        )
        .replacen("var attach_count = 0;", "var attach_count = 1;", 1)
        .replacen("var mail_state = 2;", "var mail_state = 1;", 1)
}

/// 领取后：正文末尾追加一行领取回执，附件已清空。
fn taken_page() -> String {
    with_body(&format!("{ORIGINAL}{SUFFIX}\n\nRemoved attachment: 90 Credits"))
}

fn transport(server: &MailDouble) -> MoogleMailDeliveryTransport {
    // 第一次 POST 在客户端超时后才得到响应，结果对客户端未知。
    transport_with_timeout(server, Duration::from_millis(400))
}

/// `request_timeout` 为单次请求的客户端 HTTP 预算。
fn transport_with_timeout(
    server: &MailDouble,
    request_timeout: Duration,
) -> MoogleMailDeliveryTransport {
    let client = MoogleMailClient::with_config(MoogleMailClientConfig {
        endpoint: server.endpoint(),
        maximum_response_bytes: 8 * 1024 * 1024,
        minimum_request_interval: Duration::from_millis(1),
        request_timeout,
    })
    .expect("loopback client");
    let credentials = MoogleMailCredentialSet::from_runtime_credentials(
        vec![MoogleMailRuntimeCredential {
            credential_id: Uuid::from_u128(0x7e7),
            state_version: 1,
            world: None,
            player_name: None,
            cookie: MoogleMailCookie::new("ipb_member_id=fixture; ipb_pass_hash=fixture")
                .expect("fixture cookie"),
        }],
        Arc::new(NoInvalidation),
        Some(Arc::new(ValidOwner)),
    )
    .expect("数据库来源凭据集合");
    MoogleMailDeliveryTransport::new(client, credentials, Arc::new(NoLoginKeys))
}

/// 结算退件 Take 的投递领取；每次执行都是新的 claim（重排后的新 fence）。
fn take_claim(case: Uuid, fencing_version: i64) -> DeliveryWorkClaim {
    let now = Timestamp::now();
    DeliveryWorkClaim {
        delivery_id: case,
        command_id: Uuid::from_u128(0x5e7),
        request_hash: RequestHash::new([7; 32]),
        origin_kind: "settlement_return_action".to_owned(),
        origin_id: format!("settlement_return:{case}"),
        recipient: "MoogleMail".to_owned(),
        payload: json!({
            "schema_version": 1, "operation": "take", "world": "main", "external_mail_id": MID,
            "template": null, "result_code": null, "user_key_user_id": null,
            "credits": 90, "cod": 0, "attachment_ids": [], "body": null,
        }),
        lease_token: Uuid::new_v4(),
        lease_until: now + auction_time::Duration::minutes(2),
        fencing_version,
        attempt_count: u32::try_from(fencing_version).expect("尝试次数"),
        worker_id: "returns-timing".to_owned(),
        claimed_at: now,
        created_at: now,
    }
}

/// 以投递 Worker 的顺序执行一次 Take：只读准备（取得原会话）后发出动作。
async fn execute(
    transport: &MoogleMailDeliveryTransport,
    claim: DeliveryWorkClaim,
) -> Result<serde_json::Value, (bool, String)> {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let prepared = transport
        .prepare_authorized(claim, deadline)
        .await
        .map_err(|error| (error.outcome_unknown, error.failure_code))?;
    prepared
        .into_send()
        .await
        .map(|result| result.result_metadata)
        .map_err(|error| (error.outcome_unknown, error.failure_code))
}

async fn double() -> MailDouble {
    let server = MailDouble::start().await.expect("loopback server");
    server.script_take_mail(MID, claimable_page(), taken_page());
    server
}

/// 旧 POST 在新预检读取之后才生效：第二次执行照常发出 POST，远端附件只取走一次，第二次以回执
/// 证明完成；本地至多一个尝试得到成功证明。
#[tokio::test]
async fn late_take_completing_after_the_new_preflight_takes_once() {
    let server = double().await;
    let transport = transport(&server);
    let case = Uuid::new_v4();
    server.delay_next_take(Duration::from_secs(2));

    let first = execute(&transport, take_claim(case, 1)).await;
    let (unknown, _) = first.expect_err("第一次 POST 在客户端超时");
    assert!(unknown, "POST 已发出后的超时是结果未知，不是确定失败");
    assert_eq!(server.take_counts(), (0, 1), "迟到的领取尚未生效");

    let second = execute(&transport, take_claim(case, 2)).await.expect("第二次以回执证明完成");
    assert_eq!(second["proof"], "action_removed_or_success_page");
    assert_eq!(second["expected_credits"], 90);
    assert_eq!(server.take_counts(), (1, 2), "远端附件只被取走一次");
}

/// 旧 POST 在新预检之前生效：新预检看到附件已取走、不可领取，失败关闭且不再发出 POST。
#[tokio::test]
async fn late_take_completing_before_the_new_preflight_blocks_a_second_post() {
    let server = double().await;
    let transport = transport(&server);
    let case = Uuid::new_v4();
    server.delay_next_take(Duration::from_secs(2));

    let first = execute(&transport, take_claim(case, 1)).await;
    assert!(first.expect_err("第一次 POST 在客户端超时").0);
    server.complete_deferred_take();
    assert_eq!(server.take_counts(), (1, 1));

    let (_, failure_code) =
        execute(&transport, take_claim(case, 2)).await.expect_err("新预检看到附件已取走");
    assert_eq!(failure_code, "preflight_unproven", "预检失败关闭，不发出写入");
    assert_eq!(server.take_counts(), (1, 1), "没有第二次 POST，也没有第二次取走");
}

/// 远端已登记到领取 POST（按到达登记，不论远端是否处理完）；5 秒内不成立即失败。
async fn wait_for_take_post(server: &MailDouble) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !server.requests().iter().any(|request| request.starts_with("POST ")) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("领取 POST 应在 5 秒内到达远端");
}

/// 投递 Worker 在领取 POST 已发出、远端尚未回应时放弃发送（`dispatch.rs` 的 `request_timeout` 或
/// 租约失效在 select 中落选，丢弃发送 future）：被放弃的 POST 持有在途锁到远端回应，重排后的新预检
/// 不与之重叠，读到附件已取走即失败关闭；远端只收到并执行一次领取。
#[tokio::test]
async fn take_abandoned_after_its_post_is_sent_keeps_the_rerun_preflight_waiting() {
    let server = double().await;
    // 客户端 HTTP 预算远长于远端处理时长：放弃由投递 Worker 裁决，不是客户端超时。
    let transport = transport_with_timeout(&server, Duration::from_secs(5));
    let case = Uuid::new_v4();
    // 远端在独立任务里处理每个请求 600ms，客户端断开不提前结束处理。
    server.set_fault(Some(Fault::DetachedDelay(Duration::from_millis(600))));

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let prepared = transport
        .prepare_authorized(take_claim(case, 1), deadline)
        .await
        .map_err(|error| error.failure_code)
        .expect("第一次执行完成只读准备");
    tokio::select! {
        biased;
        result = prepared.into_send() => {
            panic!("领取应在远端回应前被放弃：{:?}", result.map(|_| ()).map_err(|error| error.failure_code));
        }
        () = wait_for_take_post(&server) => {}
    }
    // 只影响之后到达的请求：已发出的领取仍按原故障在远端处理。
    server.set_fault(None);

    let (_, failure_code) =
        execute(&transport, take_claim(case, 2)).await.expect_err("新预检看到附件已取走");
    assert_eq!(failure_code, "preflight_unproven", "预检失败关闭，不发出写入");
    assert_eq!(server.take_counts(), (1, 1), "远端只收到并执行一次领取 POST");
    assert_eq!(server.max_concurrent_requests(), 1, "新预检不得与远端仍在处理的领取重叠");
}

/// Admin 再次领回的准入不比较凭据版本，新一轮执行用的是当时的会话；若凭据已换成另一个账号，同一
/// MID 在那个会话里不是原退件（这里是发件人不同）。预检按发件人、可领取与冻结资产逐项核对，
/// 对不上即失败关闭：不发出任何领取 POST，也没有附件被取走；失败按结果未知归类，案件回到 Take
/// 结果未知，只能由新会话的观察再推进，不会被当成确定失败或自动重试。
#[tokio::test]
async fn take_in_a_session_showing_another_mail_fails_closed_without_a_post() {
    let server = MailDouble::start().await.expect("loopback server");
    let foreign = claimable_page().replacen("value=\"MoogleMail\"", "value=\"Another Player\"", 1);
    assert_ne!(foreign, claimable_page(), "夹具必须真的换了发件人");
    server.script_take_mail(MID, foreign, taken_page());
    let transport = transport(&server);

    let (unknown, failure_code) = execute(&transport, take_claim(Uuid::new_v4(), 1))
        .await
        .expect_err("不是原退件的页面不能通过预检");
    assert_eq!(failure_code, "preflight_unproven");
    assert!(unknown, "预检不成立按结果未知收口，不当作确定失败");
    assert_eq!(server.take_counts(), (0, 0), "没有领取 POST，也没有附件被取走");
}
