//! 零资产玩家通知（卖方汇总、换季提醒）与资产冲突门的 PostgreSQL 18 用例。
//!
//! 内部履约与装备损失放行前会扫描全库出站投递，只有经闭集校验证明与目标装备无关的载荷
//! 才能排除。回归：扫描曾漏掉这两类来源，库里只要有一封卖方汇总或换季提醒，内部履约就判
//! `Unproven`、装备损失快照被 `conflicting_asset_operation` 阻断。两类通知经真实入队校验
//! 落库后必须被排除；载荷被改成非规范写法时无法证明无关，仍按冲突失败关闭。
//!
//! 仅在 `test-db` feature 下编译，入口见 `just test-db`。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use auction_application::core::RequestHash;
use auction_application::mail::{
    MailDeliveryCommand, MailDeliveryEnqueuer, MailDeliveryOperation, MailDeliveryOrigin,
    MailNotificationTemplate, season_reminder_mail_id,
};
use auction_domain::CommandId;
use auction_storage::{DatabaseStore, asset_conflict_gate_passes};
use chrono::{TimeZone, Utc};
use uuid::Uuid;

use crate::support;

/// 冲突门的目标装备；夹具投递都不携带装备，任何 EID 都应得到同一结论。
const TARGET_EID: &str = "424242";

fn seller_summary() -> MailDeliveryCommand {
    let command_id = Uuid::new_v4();
    MailDeliveryCommand {
        command_id: CommandId::from_uuid(command_id),
        request_hash: RequestHash::new([7; 32]),
        origin: MailDeliveryOrigin::SellerSummary,
        origin_id: format!("seller_summary:{}:FixtureSeller", Uuid::new_v4()),
        operation: MailDeliveryOperation::Send,
        world_id: "main".to_owned(),
        external_mail_id: command_id.to_string(),
        recipient: "FixtureSeller".to_owned(),
        template: Some(MailNotificationTemplate::AuctionSellerSummary),
        result_code: Some("summary".to_owned()),
        user_key_user_id: None,
        credits: 0,
        cod: 0,
        attachment_ids: Vec::new(),
        notification_body: Some("Fixture seller summary.".to_owned()),
    }
}

fn season_reminder() -> MailDeliveryCommand {
    let boundary = Utc.with_ymd_and_hms(2026, 11, 1, 9, 0, 0).single().expect("合法边界");
    MailDeliveryCommand {
        command_id: CommandId::from_uuid(Uuid::new_v4()),
        request_hash: RequestHash::new([8; 32]),
        origin: MailDeliveryOrigin::SeasonReminder,
        origin_id: format!("season_reminder:{}:FixtureBuyer", boundary.timestamp()),
        operation: MailDeliveryOperation::Send,
        world_id: "isekai".to_owned(),
        external_mail_id: season_reminder_mail_id(boundary),
        recipient: "FixtureBuyer".to_owned(),
        template: Some(MailNotificationTemplate::IsekaiSeasonReminder),
        result_code: Some("season_end".to_owned()),
        user_key_user_id: None,
        credits: 0,
        cod: 0,
        attachment_ids: Vec::new(),
        notification_body: Some("Fixture season reminder.".to_owned()),
    }
}

async fn gate_passes(store: &DatabaseStore) -> bool {
    asset_conflict_gate_passes(store, TARGET_EID).await.expect("冲突门只读判定")
}

/// 规范的卖方汇总与换季提醒不阻断资产冲突门；非规范的换季边界无法证明无关，仍阻断。
#[tokio::test]
async fn postgres_player_notices_do_not_block_the_asset_conflict_gate() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let admin = database.maintenance_store().await.expect("以维护账号连接");
    assert!(gate_passes(&store).await, "空库必须放行");

    store.enqueue(seller_summary()).await.expect("卖方汇总经真实校验入队");
    assert!(gate_passes(&store).await, "卖方汇总不携带装备，不得阻断资产冲突门");

    let reminder = store.enqueue(season_reminder()).await.expect("换季提醒经真实校验入队");
    assert!(gate_passes(&store).await, "换季提醒不携带装备，不得阻断资产冲突门");

    sqlx::query(
        "UPDATE public.deliveries
         SET payload = jsonb_set(payload, '{external_mail_id}',
                                 '\"season-end:2026-11-01T09:00:00+00:00\"'::jsonb)
         WHERE id = $1",
    )
    .bind(reminder.delivery_id)
    .execute(admin.postgres_pool())
    .await
    .expect("把换季边界改成非规范写法");
    assert!(!gate_passes(&store).await, "非规范的换季边界必须按冲突失败关闭");

    store.postgres_pool().close().await;
    admin.postgres_pool().close().await;
}
