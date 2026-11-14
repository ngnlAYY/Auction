//! 验收：出站通知模板的修订事实在真实 PostgreSQL 上只追加、可原样重放，并按版本 CAS。
//!
//! 被测连接是运行账号：证明最小权限下（只追加表只有 `INSERT`）修订、审计与读取都能完成。
//! 入口见 `just test-db`。

#![cfg(feature = "test-db")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use auction_application::mail_templates::{
    LiveMailTemplates, MailTemplateCommand, MailTemplateOperation, MailTemplateRefreshService,
    MailTemplateSet, MailTemplateStore, MailTemplateStoreError,
};
use auction_storage::DatabaseStore;
use uuid::Uuid;

use crate::support;

const PAYOUT_KEY: &str = "settlement_seller_payout";
const PAYOUT: &str = "Sold {item_name}.\nPayout: {amount} Credits\nReference: {reference}";

fn command(id: u128, key: &str, expected_revision: i64, body: Option<&str>) -> MailTemplateCommand {
    MailTemplateCommand {
        command_id: Uuid::from_u128(id),
        template_key: key.to_owned(),
        expected_revision,
        operation: if body.is_some() {
            MailTemplateOperation::Set
        } else {
            MailTemplateOperation::Reset
        },
        body: body.map(str::to_owned),
        actor_id: "admin".to_owned(),
        request_id: Some("req-mail-template".to_owned()),
    }
}

async fn count(store: &DatabaseStore, sql: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
        .fetch_one(store.postgres_pool())
        .await
        .expect("读取计数")
}

#[tokio::test]
async fn postgres_mail_template_revisions_are_append_only_replayable_and_versioned() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");

    // 新库没有任何修订：所有模板都使用内置默认正文。
    assert!(store.latest_mail_templates().await.expect("读取最新修订").is_empty());

    // 首次保存从版本 0 开始，事实与审计同事务写入。
    let first = command(1, PAYOUT_KEY, 0, Some(PAYOUT));
    let accepted = store.write_mail_template(first.clone()).await.expect("首次保存");
    assert_eq!((accepted.revision, accepted.body.as_deref()), (1, Some(PAYOUT)));
    assert_eq!(accepted.actor_id, "admin");
    let audits = "SELECT count(*) FROM public.audit_records
                  WHERE action = 'mail_template.set' AND entity_type = 'mail_template'
                    AND entity_id = 'settlement_seller_payout' AND request_id = 'req-mail-template'";
    assert_eq!(count(&store, audits).await, 1);
    // 审计只记身份与版本，不复制正文。
    assert_eq!(
        count(
            &store,
            "SELECT count(*) FROM public.audit_records
             WHERE action = 'mail_template.set' AND metadata::text LIKE '%Payout:%'"
        )
        .await,
        0
    );

    // 同 ID 同请求原样重放：返回原事实，不追加第二条事实或审计。
    let replayed = store.write_mail_template(first.clone()).await.expect("原样重放");
    assert_eq!(replayed, accepted);
    assert_eq!(count(&store, "SELECT count(*) FROM public.mail_template_events").await, 1);
    assert_eq!(count(&store, audits).await, 1);

    // 同 ID 不同请求、以及基于过期版本的新命令，都是冲突。
    let changed = MailTemplateCommand { body: Some(format!("{PAYOUT}\nThanks.")), ..first.clone() };
    assert_eq!(
        store.write_mail_template(changed).await.unwrap_err(),
        MailTemplateStoreError::Conflict
    );
    assert_eq!(
        store.write_mail_template(command(2, PAYOUT_KEY, 0, Some(PAYOUT))).await.unwrap_err(),
        MailTemplateStoreError::Conflict
    );

    // 不满足模板规则的正文与未知模板键在写入前被拒绝，不留下任何事实。
    assert!(matches!(
        store
            .write_mail_template(command(3, PAYOUT_KEY, 1, Some("Payout: {amount} Credits")))
            .await,
        Err(MailTemplateStoreError::InvalidBody(_))
    ));
    assert_eq!(
        store
            .write_mail_template(command(4, "no_such_template", 0, Some(PAYOUT)))
            .await
            .unwrap_err(),
        MailTemplateStoreError::UnknownTemplate
    );
    assert_eq!(count(&store, "SELECT count(*) FROM public.mail_template_events").await, 1);

    // 另一个模板键有自己的版本序列；读取每个键只返回最新一条。
    let pm_body = "Outbid: {item_name} {lot_code} {amount}\nNotification: {notification_id}.";
    store
        .write_mail_template(command(5, "lot_watch_pm", 0, Some(pm_body)))
        .await
        .expect("保存另一个模板");
    let updated = format!("{PAYOUT}\nThank you.");
    let second = store
        .write_mail_template(command(6, PAYOUT_KEY, 1, Some(&updated)))
        .await
        .expect("基于最新版本保存");
    assert_eq!(second.revision, 2);
    let mut latest = store.latest_mail_templates().await.expect("读取最新修订");
    latest.sort_by(|left, right| left.template_key.cmp(&right.template_key));
    assert_eq!(
        latest
            .iter()
            .map(|event| (event.template_key.as_str(), event.revision, event.body.as_deref()))
            .collect::<Vec<_>>(),
        [("lot_watch_pm", 1, Some(pm_body)), (PAYOUT_KEY, 2, Some(updated.as_str()))]
    );

    // 生效快照：数据库里的正文叠加到内置默认正文上，未修订的模板保持默认。
    let live = LiveMailTemplates::default();
    let shared: Arc<dyn MailTemplateStore> = Arc::new(store.clone());
    let refresher = MailTemplateRefreshService::new(Arc::clone(&shared), live.clone());
    let refreshed = refresher.refresh().await.expect("刷新快照");
    assert!(refreshed.changed && refreshed.skipped.is_empty());
    let defaults = MailTemplateSet::default();
    assert_eq!(live.current().body(PAYOUT_KEY), Some(updated.as_str()));
    assert_eq!(live.current().body("lot_watch_pm"), Some(pm_body));
    assert_eq!(live.current().body("lot_watch"), defaults.body("lot_watch"));
    // 内容没有变化的刷新不替换快照。
    assert!(!refresher.refresh().await.expect("再次刷新").changed);

    // 恢复默认也是一条事实：版本继续递增，生效正文回到内置默认。
    let reset = store.write_mail_template(command(7, PAYOUT_KEY, 2, None)).await.expect("恢复默认");
    assert_eq!(
        (reset.revision, reset.operation, reset.body.as_deref()),
        (3, MailTemplateOperation::Reset, None)
    );
    assert_eq!(
        count(
            &store,
            "SELECT count(*) FROM public.audit_records WHERE action = 'mail_template.reset'"
        )
        .await,
        1
    );
    assert!(refresher.refresh().await.expect("恢复默认后刷新").changed);
    assert_eq!(live.current().body(PAYOUT_KEY), defaults.body(PAYOUT_KEY));
    assert_eq!(live.current().body("lot_watch_pm"), Some(pm_body));

    // 历史事实原样保留：三条收款模板修订加一条私信模板修订。
    assert_eq!(count(&store, "SELECT count(*) FROM public.mail_template_events").await, 4);

    store.postgres_pool().close().await;
}

#[tokio::test]
async fn postgres_mail_template_events_reject_rewrites_at_the_trigger_level() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let maintenance = database.maintenance_store().await.expect("以维护账号连接");
    store.write_mail_template(command(1, PAYOUT_KEY, 0, Some(PAYOUT))).await.expect("首次保存");

    // 表 owner 不受 GRANT 限制，仍被只追加触发器拒绝（55000）。
    for sql in [
        "UPDATE public.mail_template_events SET body = 'rewritten'",
        "DELETE FROM public.mail_template_events",
        "TRUNCATE public.mail_template_events",
    ] {
        let error = sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()))
            .execute(maintenance.postgres_pool())
            .await
            .expect_err("只追加表不得改写");
        assert_eq!(
            error.as_database_error().and_then(sqlx::error::DatabaseError::code).as_deref(),
            Some("55000"),
            "{sql}"
        );
    }

    // 数据库约束兜底：set 必须带正文，版本必须紧接期望版本。
    for sql in [
        "INSERT INTO public.mail_template_events
            (command_id, digest_version, request_digest, template_key, revision,
             expected_revision, operation, body, actor_id, audit_id, result_payload, recorded_at)
         SELECT gen_random_uuid(), 1, decode(repeat('00', 32), 'hex'), 'lot_watch', 1, 0, 'set',
                NULL, 'admin', id, '{}'::jsonb, now()
         FROM public.audit_records LIMIT 1",
        "INSERT INTO public.mail_template_events
            (command_id, digest_version, request_digest, template_key, revision,
             expected_revision, operation, body, actor_id, audit_id, result_payload, recorded_at)
         SELECT gen_random_uuid(), 1, decode(repeat('00', 32), 'hex'), 'lot_watch', 5, 0, 'reset',
                NULL, 'admin', id, '{}'::jsonb, now()
         FROM public.audit_records LIMIT 1",
    ] {
        let error = sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()))
            .execute(maintenance.postgres_pool())
            .await
            .expect_err("违反约束的事实不得写入");
        assert_eq!(
            error.as_database_error().and_then(sqlx::error::DatabaseError::code).as_deref(),
            Some("23514"),
            "{sql}"
        );
    }

    store.postgres_pool().close().await;
    maintenance.postgres_pool().close().await;
}
