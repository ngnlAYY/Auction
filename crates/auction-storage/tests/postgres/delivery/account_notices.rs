//! 账号回信（rekey）经真实存储入队校验与回信预算写入 outbox 的 PostgreSQL 18 用例。
//!
//! 入队校验拒绝合法回信时，邮件处理器只能整封重试，游标停在该信之前，同一 world 的入站
//! 邮箱随之停摆；因此必须经 `DatabaseStore` 的真实入队路径证明 rekey 回信可以落库。
//!
//! 仅在 `test-db` feature 下编译，入口见 `mise run test-db`。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use auction_application::core::RequestHash;
use auction_application::mail::{
    MailDeliveryCommand, MailDeliveryEnqueuer, MailDeliveryOperation, MailDeliveryOrigin,
    MailNoticeEnqueue, MailNoticeLimit, MailNotificationTemplate,
};
use auction_domain::CommandId;
use auction_time::{Duration, Timestamp};
use uuid::Uuid;

use crate::support;

fn rekey_command(
    template: MailNotificationTemplate,
    result_code: &str,
    user_key_user_id: Option<Uuid>,
) -> MailDeliveryCommand {
    MailDeliveryCommand {
        command_id: CommandId::from_uuid(Uuid::new_v4()),
        request_hash: RequestHash::new([5; 32]),
        origin: MailDeliveryOrigin::MailIntake,
        origin_id: "main:4242:send".to_owned(),
        operation: MailDeliveryOperation::Send,
        world_id: "main".to_owned(),
        external_mail_id: "4242".to_owned(),
        recipient: "RekeyPlayer".to_owned(),
        template: Some(template),
        result_code: Some(result_code.to_owned()),
        user_key_user_id,
        credits: 0,
        cod: 0,
        attachment_ids: Vec::new(),
        notification_body: None,
    }
}

fn rekey_limit() -> MailNoticeLimit {
    MailNoticeLimit {
        templates: &["rekey_key", "rekey_result"],
        since: Timestamp::now() - Duration::hours(24),
        maximum: 3,
    }
}

/// 回归：rekey_key 回信带用户 key 引用，必须经入队校验写成 account 类 pending 投递；
/// 同一命令重放返回原 delivery（不变量 1）。
#[tokio::test]
async fn postgres_rekey_key_notice_enqueues_with_user_key_reference() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let user_id = Uuid::new_v4();
    let command = rekey_command(MailNotificationTemplate::RekeyKey, "rekeyed", Some(user_id));

    let outcome =
        store.enqueue_within_limit(command.clone(), rekey_limit()).await.expect("rekey 回信入队");
    let MailNoticeEnqueue::Enqueued(result) = outcome else {
        panic!("预算内的 rekey 回信不得被抑制");
    };
    let (status, payload_user, template, category): (
        String,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = sqlx::query_as(
        "SELECT status, payload ->> 'user_key_user_id', payload_template,
                public.delivery_send_category(origin_kind, payload)
         FROM public.deliveries WHERE id = $1",
    )
    .bind(result.delivery_id)
    .fetch_one(store.postgres_pool())
    .await
    .expect("读取 rekey 投递");
    assert_eq!(status, "pending");
    assert_eq!(payload_user, Some(user_id.to_string()));
    assert_eq!(template.as_deref(), Some("rekey_key"));
    assert_eq!(category.as_deref(), Some("account"));

    let replay = store.enqueue_within_limit(command, rekey_limit()).await.expect("同命令重放");
    assert_eq!(replay, MailNoticeEnqueue::Enqueued(result));

    // 反例：rekey_result 不得携带用户 key 引用，入队前即失败关闭且不写行。
    let invalid =
        rekey_command(MailNotificationTemplate::RekeyResult, "not_registered", Some(user_id));
    let invalid_command_id = invalid.command_id;
    assert!(store.enqueue_within_limit(invalid, rekey_limit()).await.is_err());
    let written: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM public.deliveries WHERE command_id = $1)")
            .bind(invalid_command_id.as_uuid())
            .fetch_one(store.postgres_pool())
            .await
            .expect("读取被拒命令");
    assert!(!written, "被拒的 rekey_result 不得写入 outbox");
    store.postgres_pool().close().await;
}
