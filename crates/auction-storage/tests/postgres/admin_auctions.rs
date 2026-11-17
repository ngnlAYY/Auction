//! Admin 拍卖快照投影：排期修改、Admin 拍卖列表与 Admin lot 页共用同一份列清单与解码。
//!
//! 以运行账号创建 draft 场次并修改计划时间；排期命令基于锁内读取的快照返回结果，它必须与
//! 两条列表读取到的快照逐字段一致。结算报告的同一投影由 `settlement` 用例覆盖。创建与排期
//! 写入的实时事件必须能经 Public 回放读出：流内序号连续、负载通过公开注册表。
//!
//! 仅在 `test-db` feature 下编译，入口见 `mise run test-db`。
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use auction_application::admin::{
    AdminAuctionCreatePort, AdminAuctionCreateService, AdminAuctionListRequest, AdminAuctionReader,
    AdminAuctionSchedulePort, AdminAuctionScheduleService, AdminCreateAuctionCommand,
    AdminLotListRequest, AdminLotReader, AdminUpdateAuctionScheduleCommand,
};
use auction_application::auction::AuctionPolicyTemplate;
use auction_application::core::RequestHash;
use auction_application::realtime::{PublicRealtimeReader, PublicRealtimeRequest};
use auction_domain::{AuctionId, CommandId};
use chrono::{Duration, DurationRound, Utc};
use serde_json::json;
use uuid::Uuid;

use crate::support;

#[tokio::test]
async fn postgres_admin_auction_snapshot_matches_across_schedule_and_lists() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = Arc::new(database.store().await.expect("以运行账号连接"));
    // 与 Admin 装配根相同：直接注入创建与排期两个应用服务，经各自端口调用。
    let create: Arc<dyn AdminAuctionCreatePort> = Arc::new(AdminAuctionCreateService::new(
        Arc::clone(&store),
        AuctionPolicyTemplate::default(),
    ));
    let schedule: Arc<dyn AdminAuctionSchedulePort> =
        Arc::new(AdminAuctionScheduleService::new(Arc::clone(&store)));
    let base =
        Utc::now().duration_trunc(Duration::seconds(1)).expect("截断到秒") + Duration::days(2);
    let auction_id = AuctionId::from_uuid(Uuid::new_v4());
    let created = create
        .create(AdminCreateAuctionCommand {
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            auction_id,
            world_id: "main".to_owned(),
            sequence: 1,
            preview_scheduled_at: base,
            scheduled_open_at: base + Duration::hours(1),
            scheduled_end_at: base + Duration::days(1),
            expected_state_version: 0,
            actor_id: "admin".to_owned(),
            request_id: None,
            request_hash: RequestHash::new([1; 32]),
        })
        .await
        .expect("创建 draft 场次");

    let scheduled = schedule
        .update_schedule(AdminUpdateAuctionScheduleCommand {
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            auction_id,
            preview_scheduled_at: base + Duration::hours(2),
            scheduled_open_at: base + Duration::hours(3),
            scheduled_end_at: base + Duration::days(1) + Duration::hours(3),
            expected_state_version: created.auction.state_version,
            actor_id: "admin".to_owned(),
            request_id: None,
            request_hash: RequestHash::new([2; 32]),
        })
        .await
        .expect("修改计划时间");
    assert_eq!(scheduled.auction.preview_scheduled_at, base + Duration::hours(2));
    assert_eq!(scheduled.auction.state_version, created.auction.state_version + 1);
    // 三条读取路径共用同一个解码器：再用命令入参独立核对排期不会覆盖的字段，避免列名写错时
    // 三处同错仍相等。
    assert_eq!(scheduled.auction.id.as_uuid(), auction_id.as_uuid());
    assert_eq!(scheduled.auction.world_id, "main");
    assert_eq!(scheduled.auction.sequence, 1);
    assert_eq!(scheduled.auction.status, "draft");
    assert_eq!(scheduled.auction.scheduled_open_at, base + Duration::hours(3));
    // draft 的计划结束只在冻结策略里（policy_snapshot.scheduled_end_at），开拍时才写入列。
    assert_eq!(scheduled.auction.scheduled_end_at, None);
    assert_eq!(scheduled.auction.opened_at, None);
    assert_eq!(scheduled.auction.cancelled_at, None);

    // 两条事件经同一追加入口在各自命令事务内取号：序号从 1 连续，负载通过公开注册表。
    let page = store
        .list_public_events(PublicRealtimeRequest {
            auction_id: auction_id.as_uuid(),
            after: 0,
            page_size: 10,
        })
        .await
        .expect("读取场次实时事件")
        .expect("启用世界的场次可见");
    assert!(!page.refetch_required, "事件序号连续且负载符合注册表");
    let kinds: Vec<(i64, &str)> =
        page.events.iter().map(|event| (event.stream_sequence, event.kind.as_str())).collect();
    assert_eq!(kinds, [(1, "auction.created"), (2, "auction.schedule_updated")]);
    assert_eq!(
        page.events[0].payload,
        json!({
            "auction_id": auction_id.as_uuid(),
            "world_id": "main",
            "sequence": 1,
            "state_version": created.auction.state_version,
        })
    );
    assert_eq!(
        page.events[1].payload,
        json!({
            "auction_id": auction_id.as_uuid(),
            "world_id": "main",
            "scheduled_end_at": base + Duration::days(1) + Duration::hours(3),
            "state_version": scheduled.auction.state_version,
        })
    );

    let listed = store
        .list_admin_auctions(AdminAuctionListRequest {
            world_id: Some("main".to_owned()),
            status: None,
            limit: 10,
            offset: 0,
        })
        .await
        .expect("读取 Admin 拍卖列表");
    let entry = listed
        .entries
        .iter()
        .find(|entry| entry.auction.id.as_uuid() == auction_id.as_uuid())
        .expect("列表包含新场次");
    let lots = store
        .list_admin_lots(AdminLotListRequest { auction_id, limit: 10, offset: 0 })
        .await
        .expect("读取 Admin lot 页")
        .expect("场次存在");

    let expected = serde_json::to_value(&scheduled.auction).expect("序列化排期快照");
    assert_eq!(serde_json::to_value(&entry.auction).expect("序列化列表快照"), expected);
    assert_eq!(serde_json::to_value(&lots.auction).expect("序列化 lot 页快照"), expected);

    store.postgres_pool().close().await;
}
