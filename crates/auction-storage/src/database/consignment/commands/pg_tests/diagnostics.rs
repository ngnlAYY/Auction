//! 检查寄售数据库错误及 tracing 事件的脱敏边界。

use std::sync::{Arc, Mutex};

use auction_application::consignment::ConsignmentError;
use sqlx::PgConnection;
use tracing::Dispatch;
use tracing::field::{Field, Visit};
use tracing::instrument::WithSubscriber;
use tracing::subscriber::NoSubscriber;

use super::fixture::{COMMAND, LISTING, SELLER, counts, register, seed};
use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::{WriteError, commit_error};

struct Capture(Arc<Mutex<Vec<String>>>);

struct Fields<'a>(&'a mut String);

impl Visit for Fields<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.push_str(&format!("{}={value:?} ", field.name()));
    }
}

impl tracing::Subscriber for Capture {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut fields = format!("{} ", event.metadata().target());
        event.record(&mut Fields(&mut fields));
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(fields);
    }
}

async fn error_detail_absent(connection: &mut PgConnection) -> Result<(), WriteError> {
    // 值以参数传递，即使 SQL 语句追踪开启也不会回显夹具标记。
    sqlx::query("SELECT set_config('consignment_test.fake_private', $1, false)")
        .bind("FAKE_DB_PRIVATE")
        .execute(&mut *connection)
        .await?;
    let sql = "DO $$ BEGIN RAISE unique_violation USING CONSTRAINT = 'fake_unknown_unique',
               DETAIL = current_setting('consignment_test.fake_private'),
               MESSAGE = current_setting('consignment_test.fake_private'); END $$";
    let convert = |raw| {
        let error = WriteError::from(raw);
        let rendered = format!("{error:?} {error}");
        assert!(rendered.contains("fake_unknown_unique"));
        assert!(rendered.contains("23505"));
        assert!(!rendered.contains("FAKE_DB_PRIVATE"));
        assert_eq!(error.into_application(), ConsignmentError::Unavailable);
    };
    let events = Arc::new(Mutex::new(Vec::new()));
    let raw_first = sqlx::query(sql)
        .execute(&mut *connection)
        .await
        .err()
        .unwrap_or_else(|| panic!("夹具 PostgreSQL 应返回唯一冲突"));
    let raw_second = sqlx::query(sql)
        .execute(&mut *connection)
        .await
        .err()
        .unwrap_or_else(|| panic!("夹具 PostgreSQL 应返回唯一冲突"));
    tracing::subscriber::with_default(Capture(events.clone()), || {
        convert(raw_first);
        tracing::callsite::rebuild_interest_cache();
        events.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
        convert(raw_second);
    });
    let events = events.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    assert!(
        events.iter().any(|entry| entry.contains("fake_unknown_unique") && entry.contains("23505"))
    );
    assert!(!events.iter().any(|entry| entry.contains("FAKE_DB_PRIVATE")));
    Ok(())
}

pub(super) async fn commit_unknown_sqlstate(store: &DatabaseStore) -> Result<(), WriteError> {
    let mut connection = store.postgres_pool().acquire().await?;
    let raw = sqlx::query("DO $$ BEGIN RAISE statement_completion_unknown; END $$")
        .execute(&mut *connection)
        .await
        .err()
        .unwrap_or_else(|| panic!("夹具应返回 SQLSTATE 40003"));
    assert!(matches!(commit_error(raw), WriteError::Application(ConsignmentError::OutcomeUnknown)));
    Ok(())
}

pub(super) async fn verify(store: &DatabaseStore) -> Result<(), WriteError> {
    let mut connection = store.postgres_pool().acquire().await?;
    error_detail_absent(&mut connection).await
}

pub(super) async fn capture_audit_failure<T>(operation: impl Future<Output = T>) -> T {
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = Dispatch::new(Capture(events.clone()));
    // 回调点的兴趣是进程级缓存。进程里只登记了一个订阅者时，tracing 按首次触发回调点的那个线程的
    // 缺省计算兴趣；同一测试二进制里并行的审计失败用例会在没有订阅者的线程里首次触发同一回调点，
    // 把兴趣缓存成 never，本用例的事件随之被丢弃。再登记一个与捕获同寿命的订阅者，tracing 就改按
    // 全部存活订阅者计算兴趣，捕获订阅者始终在内。
    let _other_dispatch = Dispatch::new(NoSubscriber::default());
    let result = operation.with_subscriber(capture).await;
    let events = events.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    assert!(
        events.iter().any(|entry| entry.starts_with("auction::storage::audit_health ")
            && entry.contains("constraint_violation")
            && entry.contains("blocks_classified_readiness=true")),
        "审计写入失败必须发出阻断就绪的健康信号"
    );
    result
}

pub(super) async fn commit_unknown_recovers_by_replay(
    store: &DatabaseStore,
    maintenance: &DatabaseStore,
) -> Result<(), WriteError> {
    seed(maintenance).await?;
    let pool = maintenance.postgres_pool();
    sqlx::raw_sql(
        "CREATE FUNCTION public.consignment_fake_commit_unknown() RETURNS trigger
         LANGUAGE plpgsql AS $$ BEGIN RAISE statement_completion_unknown; END; $$;
         CREATE CONSTRAINT TRIGGER consignment_fake_commit_unknown
         AFTER INSERT ON public.consignment_commands DEFERRABLE INITIALLY DEFERRED
         FOR EACH ROW EXECUTE FUNCTION public.consignment_fake_commit_unknown()",
    )
    .execute(pool)
    .await?;
    let result = register(store, COMMAND, LISTING, SELLER, "42").await;
    assert!(
        matches!(result, Err(WriteError::Application(ConsignmentError::OutcomeUnknown))),
        "实际提交返回 40003 时应报告结果未知：{result:?}"
    );
    assert_eq!(counts(pool).await?, (0, 0, 0, 0));
    sqlx::query("DROP TRIGGER consignment_fake_commit_unknown ON public.consignment_commands")
        .execute(pool)
        .await?;

    let committed = register(store, COMMAND, LISTING, SELLER, "42").await?;
    let replayed = register(store, COMMAND, LISTING, SELLER, "42").await?;
    assert_eq!(committed.status, 202);
    assert_eq!(committed.body["command_id"], COMMAND.to_string());
    assert_eq!(committed, replayed);
    assert_eq!(counts(pool).await?, (1, 1, 1, 1));
    Ok(())
}
