//! 寄售主题验收（R4 §4.1 第 3 条）：寄售系统退件不被判给拍卖原案件，留在共享隔离；拍卖主题不受影响。
//!
//! 保护不变量 6：义务来源不是拍卖时失败关闭，寄售退件不能被吞成拍卖案件。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use auction_application::consignment::{
    DecideEnvelopeCommand, EnvelopeStore, LegacyAuctionOwnerResolver,
};
use auction_application::core::RequestHash;
use auction_application::mail::QuarantineRecheckRound;
use auction_domain::consignment::OriginalMailKey;
use auction_domain::consignment::deposit::{
    EnvelopeCapabilities, EnvelopeDecision, EnvelopeObservation, LegacyAuctionOwner,
    LockedEnvelopeFacts, QuarantineReason,
};
use auction_time::Timestamp;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::contracts::fixture::{Graph, OBLIGATION_3, seed_main_custody};
use super::super::contracts::source_guard::insert_system_return;
use super::super::deposit::envelopes::{repository, worker_command};
use super::super::deposit::owner_handoff::count;
use super::super::deposit::owner_recheck::{resolver_service, round_request};
use super::super::deposit::support::{DECIDE, SESSION};
use crate::recovery_returns::{insert_sale, settlement_minute};
use crate::support::TestDatabase;

fn quarantine_command(
    key: OriginalMailKey,
    observation: EnvelopeObservation,
    index: u128,
) -> DecideEnvelopeCommand {
    DecideEnvelopeCommand {
        command_id: worker_command(&format!("acceptance-consignment-rts-{index}")),
        request_hash: RequestHash::new([0x52; 32]),
        mail_key: key.clone(),
        facts: LockedEnvelopeFacts {
            mail_key: Some(key),
            observation,
            reservations: Vec::new(),
            envelope_seller_user_id: None,
            capabilities: EnvelopeCapabilities { safe_original_return: false, take_ready: false },
            read_failure: false,
            already_decided: false,
            now: Timestamp::UNIX_EPOCH,
        },
        decision: EnvelopeDecision::Quarantine(QuarantineReason::SuspectedSystemRts),
    }
}

/// 隔离需要原信行；会话沿用寄售夹具里已认证的 Bot 会话。
async fn seed_original(pool: &PgPool, key: &OriginalMailKey) {
    sqlx::query(
        "INSERT INTO public.mail_originals
            (bot_uid, world_id, external_mid, first_session_id, received_minute_raw,
             received_timezone, first_seen_at, season_window_id)
         VALUES ($1, 'main', $2, $3, '2026-01-01T03:00', 'UTC', '2026-01-01T03:00:00Z', NULL)",
    )
    .bind(key.authenticated_bot_uid())
    .bind(key.external_mid())
    .bind(Uuid::parse_str(SESSION).unwrap())
    .execute(pool)
    .await
    .expect("写入原信");
}

async fn assert_no_auction_case(pool: &PgPool) {
    for query in [
        "SELECT count(*) FROM public.mail_quarantine_releases",
        "SELECT count(*) FROM public.settlement_return_evidence",
        "SELECT count(*) FROM public.settlement_return_cases",
        "SELECT count(*) FROM public.settlement_recovery_return_handoffs",
    ] {
        assert_eq!(count(pool, query).await, 0, "{query}");
    }
}

/// 寄售主题的系统 RTS 经重检仍留在共享隔离，零案件零交接；同库里拍卖主题的退款退件仍被判给拍卖 owner。
#[tokio::test]
async fn postgres_consignment_subject_rts_stays_in_shared_quarantine() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("以维护账号连接");
    let pool = maintenance.postgres_pool();
    seed_main_custody(pool).await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(Graph::sale_main().sql(DECIDE)))
        .execute(pool)
        .await
        .expect("合同图");
    let store = database.store().await.expect("运行账号");
    let repo = repository(&database).await;

    // 反例：寄售规范主题，以及套着拍卖主题的寄售买家退款义务 ID（只有来源守卫能拦住后者）。
    let work_before = count(pool, "SELECT count(*) FROM public.mail_disposition_work").await;
    let subjects = [
        format!("[RTS] Consignment buyer refund [{OBLIGATION_3}]"),
        format!("[RTS] Auction buyer refund [{OBLIGATION_3}]"),
    ];
    for (index, subject) in subjects.iter().enumerate() {
        let index = u128::try_from(index).unwrap() + 1;
        let (key, observation) = insert_system_return(pool, index, OBLIGATION_3, subject).await;
        seed_original(pool, &key).await;
        repo.decide_envelope(quarantine_command(key, observation.clone(), index))
            .await
            .expect("首次隔离");
        let service = resolver_service(&repo, store.clone(), observation);
        let round = service.run_once(round_request(pool).await).await.expect("重检保持隔离");
        assert!(matches!(round, QuarantineRecheckRound::Appended { .. }), "{subject}");
    }
    let released =
        count(pool, "SELECT count(*) FROM public.mail_quarantines WHERE state <> 'active'");
    assert_eq!(released.await, 0, "寄售退件不被拍卖 owner 解除");
    assert_no_auction_case(pool).await;
    // 夹具里的代管 Take 工作是既有的 1 行；重检不得再新增。
    assert_eq!(count(pool, "SELECT count(*) FROM public.mail_disposition_work").await, work_before);

    // 正例：同库里的拍卖退款主题仍判给拍卖原案件 owner。
    let (_, settled_at) = settlement_minute();
    let sale = insert_sale(pool, 7_520_001, settled_at, 1).await;
    let auction_subject = format!("[RTS] Auction buyer refund [{}]", sale.refund_obligation);
    let (auction_key, auction_observation) =
        insert_system_return(pool, 3, &sale.refund_obligation.to_string(), &auction_subject).await;
    let owner = store.resolve(&auction_key, &auction_observation).await.expect("旧 owner 判定");
    assert_eq!(owner, Some(LegacyAuctionOwner::BuyerRefund));
}
