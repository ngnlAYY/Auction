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
use auction_application::public::{
    PUBLIC_RULES_LOCALES, PublicRulesReader, PublicRulesRequest, SiteDocumentKind,
};
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
async fn postgres_fresh_database_has_preset_rules_in_every_public_locale() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号连接");
    for locale in PUBLIC_RULES_LOCALES {
        let document = store
            .get_public_rules(PublicRulesRequest {
                kind: SiteDocumentKind::Rules,
                locale: (*locale).to_owned(),
            })
            .await
            .expect("公开读取")
            .unwrap_or_else(|| panic!("{locale} 应有预置规则"));
        // 预置文档就是该语言自己的文档（不是英文回退），从版本 1 开始，正文以一级标题开头。
        assert_eq!(document.locale, *locale);
        assert_eq!(document.revision, 1, "locale={locale}");
        document.validate().expect("预置文档应通过应用层校验");
        assert!(document.markdown.starts_with("# "), "locale={locale}");
        let admin = store
            .get_admin_rules(AdminRulesRequest {
                kind: SiteDocumentKind::Rules,
                locale: (*locale).to_owned(),
            })
            .await
            .expect("管理端读取")
            .expect("预置规则");
        admin.validate().expect("管理端快照应通过校验");
        assert_eq!(admin.published_by, "system");
    }
    // 只预置规则；“关于”仍由运营者编写。
    assert_eq!(public_markdown(&store, SiteDocumentKind::About).await, None);
    store.postgres_pool().close().await;
}

#[tokio::test]
async fn postgres_site_documents_are_independent_per_kind() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号连接");
    let port = port(&store);

    // 新库自带预置规则（版本 1），“关于”不存在；两者互不影响。
    assert_eq!(public_markdown(&store, SiteDocumentKind::About).await, None);

    // 从预置版本接续：必须带上版本 1，带 0 会被当成“新建已存在的文档”而拒绝。
    let created_again = port
        .publish(command(SiteDocumentKind::Rules, Uuid::new_v4(), 0, "Rules", "bid fairly"))
        .await
        .expect_err("预置规则已存在，不能从版本 0 新建");
    assert!(
        format!("{created_again:?}").contains("current_revision: 1"),
        "应报告预置规则的当前版本，实际：{created_again:?}"
    );
    let rules = port
        .publish(command(SiteDocumentKind::Rules, Uuid::new_v4(), 1, "Rules", "bid fairly"))
        .await
        .expect("更新规则");
    assert_eq!(rules.revision, 2);
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
        Some((2, "bid fairly".to_owned()))
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
    // 规则仍是预置的版本 1，没有被这次冲突的命令改写。
    let rules = public_markdown(&store, SiteDocumentKind::Rules).await.expect("预置规则");
    assert_eq!(rules.0, 1);
    assert_ne!(rules.1, "hello");
    store.postgres_pool().close().await;
}

#[tokio::test]
async fn postgres_site_document_publish_audits_by_kind() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("运行账号连接");
    let port = port(&store);
    port.publish(command(SiteDocumentKind::Rules, Uuid::new_v4(), 1, "Rules", "r"))
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
