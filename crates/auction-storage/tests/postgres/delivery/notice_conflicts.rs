//! 零资产通知（卖方汇总、换季提醒、投稿回执）与资产冲突门的 PostgreSQL 18 用例。
//!
//! 内部履约与装备损失放行前会扫描全库出站投递，只有经闭集校验证明与目标装备无关的载荷
//! 才能排除。回归：扫描曾漏掉卖方汇总与换季提醒两类来源，投稿回执又因携带冻结正文被当作
//! 无法识别的普通通知；库里只要有一封这类通知，内部履约就判 `Unproven`、装备损失快照被
//! `conflicting_asset_operation` 阻断。三类通知经真实入队校验落库后必须被排除；载荷被改成
//! 非规范写法时无法证明无关，仍按冲突失败关闭。
//!
//! 仅在 `test-db` feature 下编译，入口见 `mise run test-db`。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use auction_application::core::{ApplicationError, RepositoryError, RequestHash};
use auction_application::mail::{
    MailDeliveryCommand, MailDeliveryEnqueuer, MailDeliveryOperation, MailDeliveryOrigin,
    MailNotificationTemplate, season_reminder_mail_id,
};
use auction_domain::CommandId;
use auction_storage::{DatabaseStore, asset_conflict_gate_passes};
use auction_time::Timestamp;
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
    let boundary = Timestamp::from_ymd_hms(2026, 11, 1, 9, 0, 0).expect("合法边界");
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

/// 与入站处理器同形的投稿回执：mail_intake 来源、规范 origin_id 与录入后冻结的正文。
fn submission_receipt() -> MailDeliveryCommand {
    MailDeliveryCommand {
        command_id: CommandId::from_uuid(Uuid::new_v4()),
        request_hash: RequestHash::new([9; 32]),
        origin: MailDeliveryOrigin::MailIntake,
        origin_id: "main:4243:send".to_owned(),
        operation: MailDeliveryOperation::Send,
        world_id: "main".to_owned(),
        external_mail_id: "4243".to_owned(),
        recipient: "FixtureSeller".to_owned(),
        template: Some(MailNotificationTemplate::AuctionSubmissionReceipt),
        result_code: Some("accepted".to_owned()),
        user_key_user_id: None,
        credits: 0,
        cod: 0,
        attachment_ids: Vec::new(),
        notification_body: Some("Fixture submission receipt.".to_owned()),
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

/// 回归：投稿回执是唯一携带冻结正文的 mail_intake 通知，经真实入队校验落库后不得阻断资产冲突门；
/// 结果码被改出模板闭集时无法证明是回执，仍按冲突失败关闭。
#[tokio::test]
async fn postgres_submission_receipt_does_not_block_the_asset_conflict_gate() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let admin = database.maintenance_store().await.expect("以维护账号连接");

    let receipt = store.enqueue(submission_receipt()).await.expect("投稿回执经真实校验入队");
    assert!(gate_passes(&store).await, "投稿回执不携带装备，不得阻断资产冲突门");

    sqlx::query(
        "UPDATE public.deliveries
         SET payload = jsonb_set(payload, '{result_code}', '\"registered\"'::jsonb)
         WHERE id = $1",
    )
    .bind(receipt.delivery_id)
    .execute(admin.postgres_pool())
    .await
    .expect("把回执结果码改出闭集");
    assert!(!gate_passes(&store).await, "闭集外的回执结果码必须按冲突失败关闭");

    store.postgres_pool().close().await;
    admin.postgres_pool().close().await;
}

/// 旧版投递行的摘要未携带正文，但载荷已冻结正文。重放时只接受原封回执。
#[tokio::test]
async fn postgres_submission_receipt_replays_legacy_digest_with_exact_body() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let mut command = submission_receipt();
    let prefix = format!(
        concat!(
            r#"{{"command_kind":"mail_delivery","aggregate_id":"{}","#,
            r#""request":{{"operation":"send","world_id":"main","external_mail_id":"4243","#,
            r#""recipient":"FixtureSeller","template":"auction_submission_receipt","#,
            r#""result_code":"accepted","user_key_user_id":null,"attachment_ids":[]"#,
        ),
        command.command_id,
    );
    let digest = |body: Option<&str>| {
        let mut request = prefix.clone();
        if let Some(body) = body {
            request.push_str(",\"notification_body\":");
            request.push_str(&serde_json::to_string(body).expect("正文 JSON"));
        }
        request.push_str("}}");
        RequestHash::from_canonical_bytes(request.as_bytes())
    };
    let legacy_digest = digest(None);
    command.request_hash = legacy_digest;
    let stored = store.enqueue(command.clone()).await.expect("旧版摘要与冻结载荷入队");

    command.request_hash = digest(command.notification_body.as_deref());
    assert_ne!(command.request_hash, legacy_digest);
    let current = store.enqueue(command.clone()).await.expect("带正文的新摘要重放旧版回执");
    assert_eq!(current.delivery_id, stored.delivery_id);

    command.notification_body = Some("Changed submission receipt.".to_owned());
    command.request_hash = digest(command.notification_body.as_deref());
    let error = store.enqueue(command).await.expect_err("同一命令的回执正文不能改变");
    assert!(matches!(
        error,
        ApplicationError::Repository(RepositoryError::Failed(message))
            if message == "MoogleMail delivery 命令摘要冲突"
    ));

    store.postgres_pool().close().await;
}
