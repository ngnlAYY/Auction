//! 站点文档（规则、关于）的发布与读取：两种文档按（种类，语言）各自独立。
//!
//! 只经公开端口驱动：发布走 `AdminRulesPort`，读取走 `PublicRulesReader` 与
//! `AdminRulesReader`，全部以运行账号连接。仅在 `test-db` feature 下编译，入口见 `just test-db`。

#![cfg(feature = "test-db")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use crate::support;

use auction_application::admin::{
    AdminRulesError, AdminRulesPort, AdminRulesPublishCommand, AdminRulesReader, AdminRulesRequest,
};
use auction_application::core::{ApplicationError, Clock, RequestHash};
use auction_application::public::{PublicRulesReader, PublicRulesRequest, SiteDocumentKind};
use auction_domain::CommandId;
use auction_storage::{DatabaseAdminRulesCommands, DatabaseStore};
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

fn port(store: &DatabaseStore) -> DatabaseAdminRulesCommands<SystemClock> {
    DatabaseAdminRulesCommands::new(Arc::new(store.clone()), Arc::new(SystemClock))
}

/// 摘要域与 HTTP 适配器一致：两种文档各用一个。
fn command(
    kind: SiteDocumentKind,
    command_id: Uuid,
    expected_revision: i64,
    title: &str,
    markdown: &str,
) -> AdminRulesPublishCommand {
    let domain = match kind {
        SiteDocumentKind::Rules => "admin_rules_publish",
        SiteDocumentKind::About => "admin_about_publish",
    };
    let request = json!({
        "command_id": command_id,
        "expected_revision": expected_revision,
        "locale": "en",
        "title": title,
        "markdown": markdown,
    });
    AdminRulesPublishCommand {
        command_id: CommandId::from_uuid(command_id),
        kind,
        expected_revision,
        locale: "en".to_owned(),
        title: title.to_owned(),
        markdown: markdown.to_owned(),
        actor_id: "ops".to_owned(),
        request_id: None,
        request_hash: RequestHash::from_request(domain, command_id, &request).expect("摘要"),
    }
}

async fn public_markdown(store: &DatabaseStore, kind: SiteDocumentKind) -> Option<(i64, String)> {
    store
        .get_public_rules(PublicRulesRequest { kind, locale: "en".to_owned() })
        .await
        .expect("公开读取")
        .map(|document| (document.revision, document.markdown))
}

#[tokio::test]
async fn postgres_site_documents_are_independent_per_kind() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号连接");
    let port = port(&store);

    // 新库里两种文档都不存在；“关于”缺失不影响规则。
    assert_eq!(public_markdown(&store, SiteDocumentKind::Rules).await, None);
    assert_eq!(public_markdown(&store, SiteDocumentKind::About).await, None);

    let rules = port
        .publish(command(SiteDocumentKind::Rules, Uuid::new_v4(), 0, "Rules", "bid fairly"))
        .await
        .expect("发布规则");
    assert_eq!(rules.revision, 1);
    assert_eq!(public_markdown(&store, SiteDocumentKind::About).await, None);

    // 同一语言的“关于”从自己的版本 0 开始，不受规则已有版本影响。
    let about = port
        .publish(command(SiteDocumentKind::About, Uuid::new_v4(), 0, "About", "who runs this"))
        .await
        .expect("发布关于");
    assert_eq!(about.revision, 1);
    let about = port
        .publish(command(SiteDocumentKind::About, Uuid::new_v4(), 1, "About", "second edition"))
        .await
        .expect("更新关于");
    assert_eq!(about.revision, 2);

    assert_eq!(
        public_markdown(&store, SiteDocumentKind::Rules).await,
        Some((1, "bid fairly".to_owned()))
    );
    assert_eq!(
        public_markdown(&store, SiteDocumentKind::About).await,
        Some((2, "second edition".to_owned()))
    );
    let admin_about = store
        .get_admin_rules(AdminRulesRequest {
            kind: SiteDocumentKind::About,
            locale: "en".to_owned(),
        })
        .await
        .expect("管理端读取")
        .expect("关于已发布");
    assert_eq!((admin_about.revision, admin_about.published_by.as_str()), (2, "ops"));

    // 版本 CAS 按文档各自判定：用规则的版本号去更新“关于”会被拒绝，并报告“关于”的当前版本。
    let stale = port
        .publish(command(SiteDocumentKind::About, Uuid::new_v4(), 1, "About", "stale write"))
        .await
        .expect_err("过期版本必须被拒绝");
    assert!(
        format!("{stale:?}").contains("current_revision: 2"),
        "应报告关于文档的当前版本，实际：{stale:?}"
    );
    store.postgres_pool().close().await;
}

#[tokio::test]
async fn postgres_site_document_command_replays_only_within_its_kind() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号连接");
    let port = port(&store);
    let command_id = Uuid::new_v4();

    let first = port
        .publish(command(SiteDocumentKind::About, command_id, 0, "About", "hello"))
        .await
        .expect("首次发布");
    // 同一命令原样重放返回原结果，不产生新版本。
    let replay = port
        .publish(command(SiteDocumentKind::About, command_id, 0, "About", "hello"))
        .await
        .expect("原样重放");
    assert_eq!(replay, first);
    assert_eq!(
        public_markdown(&store, SiteDocumentKind::About).await,
        Some((1, "hello".to_owned()))
    );

    // 同一 command_id 改投规则文档：不是同一请求，必须冲突，也不能借此写出规则。
    let crossed = port
        .publish(command(SiteDocumentKind::Rules, command_id, 0, "About", "hello"))
        .await
        .expect_err("跨文档种类重放必须冲突");
    assert!(
        matches!(crossed, ApplicationError::AdminRules(AdminRulesError::CommandConflict)),
        "实际：{crossed:?}"
    );
    assert_eq!(public_markdown(&store, SiteDocumentKind::Rules).await, None);
    store.postgres_pool().close().await;
}

#[tokio::test]
async fn postgres_site_document_publish_audits_by_kind() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号连接");
    let port = port(&store);
    port.publish(command(SiteDocumentKind::Rules, Uuid::new_v4(), 0, "Rules", "r"))
        .await
        .expect("发布规则");
    port.publish(command(SiteDocumentKind::About, Uuid::new_v4(), 0, "About", "a"))
        .await
        .expect("发布关于");

    let rows = sqlx::query(
        "SELECT action, entity_type, entity_id FROM public.audit_records
         WHERE action IN ('rules.publish', 'about.publish') ORDER BY action",
    )
    .fetch_all(store.postgres_pool())
    .await
    .expect("读取审计");
    let facts = rows
        .iter()
        .map(|row| {
            (
                row.get::<String, _>("action"),
                row.get::<String, _>("entity_type"),
                row.get::<String, _>("entity_id"),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        facts,
        vec![
            ("about.publish".to_owned(), "about_document".to_owned(), "en".to_owned()),
            ("rules.publish".to_owned(), "rules_document".to_owned(), "en".to_owned()),
        ]
    );
    store.postgres_pool().close().await;
}
