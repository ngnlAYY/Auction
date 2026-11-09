//! 发送基线在邮件替身上的回归。
//!
//! main 世界 Sent 第 0 页带有指向未采集页的 Next 链接；把页数上限设为 1 即复现“Sent 超过
//! `maximum_pages`”。旧的完整排除集（`full`，以及由旧路径裁决的 `shadow`）在发送前失败关闭，
//! 这即发送基线方案；`watermark` 只读第 0 页即可取得临时边界，发送后只把边界之上的新邮件当作候选。
//!
//! 覆盖限制：已采集的 Sent 详情都带附件，替身无法回放附件写入后的读回，因此不在这里跑完整的
//! `send()` 成功证明；候选详情的逐字段比对由 `verify_candidates` 的既有路径负责。

use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    time::Duration,
};

use auction_application::mail::MailScanWatermark;
use auction_application::sent_baseline::{
    SentBaselineEvent, SentBaselineMode, SentBaselineObserver, SentBaselinePhase,
    SentBaselineReason,
};
use auction_test_support::mooglemail::{Fault, MailDouble};
use chrono::{TimeZone, Utc};
use tokio::time::Instant;

use super::SentBaselineKind;
use crate::clients::mooglemail::{
    MoogleMailClient, MoogleMailClientConfig, MoogleMailCookie, MoogleMailErrorCategory,
    MoogleMailMessageFields, MoogleMailSendRequest, MoogleMailSender, MoogleMailWorld,
    client::SentBoundary,
};

const WORLD: MoogleMailWorld = MoogleMailWorld::Main;
/// fixture Sent 第 0 页最新一行（2026-11-02 04:34）。
const NEWEST: &str = "3125366";
/// 第二行（2026-11-02 04:25）。
const SECOND: &str = "3125364";

#[derive(Default)]
struct Recorder(Mutex<Vec<SentBaselineEvent>>);

impl SentBaselineObserver for Recorder {
    fn observe(&self, _world: &str, event: SentBaselineEvent) {
        self.0.lock().expect("recorder lock").push(event);
    }
}

impl Recorder {
    fn fallbacks(&self) -> Vec<(SentBaselinePhase, SentBaselineReason)> {
        self.0
            .lock()
            .expect("recorder lock")
            .iter()
            .filter_map(|event| match event {
                SentBaselineEvent::Fallback { phase, reason } => Some((*phase, *reason)),
                _ => None,
            })
            .collect()
    }
}

fn make_sender(
    server: &MailDouble,
    mode: SentBaselineMode,
    observer: Arc<Recorder>,
) -> MoogleMailSender {
    let client = MoogleMailClient::with_config(MoogleMailClientConfig {
        endpoint: server.endpoint(),
        maximum_response_bytes: 8 * 1024 * 1024,
        minimum_request_interval: Duration::from_millis(1),
        request_timeout: Duration::from_secs(5),
    })
    .expect("loopback client");
    // 页数上限 1：fixture 的 Sent 第 0 页还有下一页，完整枚举必然超限。
    MoogleMailSender::new(client)
        .with_send_verification(1, Duration::from_secs(10))
        .expect("verification bounds")
        .with_sent_baseline(mode, observer)
}

fn cookie() -> MoogleMailCookie {
    MoogleMailCookie::new("ipb_member_id=fixture; ipb_pass_hash=fixture").expect("fixture cookie")
}

fn sent_requests(server: &MailDouble) -> Vec<String> {
    server.requests().into_iter().filter(|request| request.contains("filter=sent")).collect()
}

fn boundary_mid(kind: &SentBaselineKind) -> &str {
    match kind {
        SentBaselineKind::Watermark(SentBoundary::Mail { mail_id, .. }) => mail_id,
        _ => panic!("watermark 模式应取得数值 MID 边界"),
    }
}

#[tokio::test]
async fn full_and_shadow_baselines_fail_closed_when_sent_exceeds_page_limit() {
    for mode in [SentBaselineMode::Full, SentBaselineMode::Shadow] {
        let server = MailDouble::start().await.expect("mail double");
        let sender = make_sender(&server, mode, Arc::default());
        let error = sender
            .prepare_sent_baseline(&cookie(), WORLD)
            .await
            .err()
            .expect("D3：完整排除集超限必须在发送前失败关闭");
        assert_eq!(error.category, MoogleMailErrorCategory::SentScanPageLimit, "{mode:?}");
        assert!(
            !server.requests().iter().any(|request| request.contains("page=1")),
            "{mode:?}：超限在请求未采集页之前裁决"
        );
    }
}

#[tokio::test]
async fn watermark_baseline_takes_boundary_from_first_page_beyond_page_limit() {
    let server = MailDouble::start().await.expect("mail double");
    let sender = make_sender(&server, SentBaselineMode::Watermark, Arc::default());
    let cookie = cookie();
    let baseline =
        sender.prepare_sent_baseline(&cookie, WORLD).await.expect("水位边界不依赖完整枚举");

    assert_eq!(boundary_mid(&baseline.kind), NEWEST);
    assert_eq!(sent_requests(&server), ["GET /?s=Bazaar&ss=mm&filter=sent"]);
    // 边界只成为下一轮前缀起点，不是完整排除集。
    assert!(baseline.full_seen().is_none());
    let checkpoint = sender.sent_scan_checkpoint(&cookie, WORLD).expect("边界写入内存检查点");
    assert_eq!(checkpoint.watermark.expect("watermark").mail_id, NEWEST);
}

#[tokio::test]
async fn watermark_baseline_survives_stale_anchor_and_due_daily_verification() {
    // 锚点时间与当前列表不符：前缀起点被否定，回退最新分钟组后仍取得边界。
    let server = MailDouble::start().await.expect("mail double");
    let sender = make_sender(&server, SentBaselineMode::Watermark, Arc::default());
    let cookie = cookie();
    let stale = MailScanWatermark {
        mail_id: SECOND.to_owned(),
        observed_at: Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).single().expect("time"),
    };
    sender.remember_sent_scan(&cookie, WORLD, Some(stale.clone()), true).await;
    let baseline = sender.prepare_sent_baseline(&cookie, WORLD).await.expect("锚点失效不阻断发送");
    assert_eq!(boundary_mid(&baseline.kind), NEWEST);
    assert!(!server.requests().iter().any(|request| request.contains("page=1")));

    // 从未完整校验：每日完整校验先尝试并超限，只告警、不阻断，随后照常取得边界。
    let server = MailDouble::start().await.expect("mail double");
    let sender = make_sender(&server, SentBaselineMode::Watermark, Arc::default());
    sender.remember_sent_scan(&cookie, WORLD, Some(stale), false).await;
    let baseline =
        sender.prepare_sent_baseline(&cookie, WORLD).await.expect("每日校验超限不阻断发送");
    assert_eq!(boundary_mid(&baseline.kind), NEWEST);
    // 同一读取轮次内第 0 页只请求一次，完整校验与边界共用；只读第 0 页即完成。
    assert_eq!(sent_requests(&server), ["GET /?s=Bazaar&ss=mm&filter=sent"]);
    assert!(!server.requests().iter().any(|request| request.contains("page=1")));
    let checkpoint = sender.sent_scan_checkpoint(&cookie, WORLD).expect("checkpoint");
    // 原检查点从未完整校验；时间被推进说明每日校验确实尝试过（超限后记录本次尝试）。
    assert!(checkpoint.last_full_scan_at.is_some(), "失败的每日校验也推进下一次校验时间");
}

#[tokio::test]
async fn watermark_after_send_selects_only_mail_above_the_boundary() {
    let server = MailDouble::start().await.expect("mail double");
    let sender = make_sender(&server, SentBaselineMode::Watermark, Arc::default());
    let cookie = cookie();
    // 发送前最新一封尚未出现。
    server.remove_sent(NEWEST);
    let baseline = sender.prepare_sent_baseline(&cookie, WORLD).await.expect("发送前边界");
    assert_eq!(boundary_mid(&baseline.kind), SECOND);

    // POST 之后新邮件出现在列表顶部；写后核对建立 fresh 轮次。
    server.restore_sent(NEWEST);
    let request = MoogleMailSendRequest {
        world: WORLD,
        message: MoogleMailMessageFields {
            recipient: "Wbukb qesil".to_owned(),
            subject: "Auction equipment delivery [8628598".to_owned(),
            body: "fixture".to_owned(),
        },
        credits: 0,
        cod: 0,
        equipment_ids: Vec::new(),
        category: None,
    };
    let round = sender.client.read_round(&cookie, WORLD, true).expect("fresh round");
    let candidates = sender
        .observe_new_sent(
            &request,
            &cookie,
            &baseline,
            &round,
            Instant::now() + Duration::from_secs(10),
        )
        .await
        .expect("写后候选不依赖完整枚举");

    assert_eq!(candidates.mids, HashSet::from([NEWEST.to_owned()]), "边界及更旧的邮件都不是候选");
    assert_eq!(candidates.candidate_ids(&request), HashSet::from([NEWEST]));
    assert!(!server.requests().iter().any(|request| request.contains("page=1")));
}

#[tokio::test]
async fn watermark_baseline_fails_closed_when_sent_cannot_be_read() {
    let server = MailDouble::start().await.expect("mail double");
    server.set_fault(Some(Fault::ProtocolDrift));
    let recorder = Arc::new(Recorder::default());
    let sender = make_sender(&server, SentBaselineMode::Watermark, Arc::clone(&recorder));
    let error = sender
        .prepare_sent_baseline(&cookie(), WORLD)
        .await
        .err()
        .expect("读不到 Sent 时不能授权发送");
    assert_ne!(error.category, MoogleMailErrorCategory::SentScanPageLimit);
    // 边界失败后回退完整枚举，同样失败才返回；回退本身留下闭集事件。
    let fallbacks = recorder.fallbacks();
    assert_eq!(fallbacks.len(), 1, "只回退一次");
    assert!(matches!(
        fallbacks[0],
        (SentBaselinePhase::BeforeSend, SentBaselineReason::ReadUnavailable)
    ));
}
