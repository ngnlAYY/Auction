//! 检查寄售数据库错误及 tracing 事件的脱敏边界。

use std::sync::{Arc, Mutex};

use auction_application::consignment::ConsignmentError;
use sqlx::PgConnection;
use tracing::field::{Field, Visit};

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
        let mut fields = String::new();
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
