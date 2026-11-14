//! Admin runtime 配置命令账本的完整生命周期，以运行账号执行。
//!
//! pending → unknown → 重开 → applied 依次经过全部查询与 mutation 语句（它们共用同一份列清单）；
//! 已应用命令同 ID 重放返回原结果、离开未完成列表；请求摘要不符与重复写入都按冲突拒绝。
//!
//! 仅在 `test-db` feature 下编译，入口见 `just test-db`。
#![cfg(feature = "test-db")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use auction_application::admin::{
    ADMIN_RUNTIME_DIGEST_VERSION, AdminRuntimeAuditRecord, AdminRuntimeCommandError,
    AdminRuntimeCommandState, AdminRuntimeCommandStore, AdminRuntimeDocument,
    AdminRuntimeStoredCommand,
};
use auction_application::core::{ApplicationError, RequestHash};
use auction_domain::CommandId;
use auction_storage::DatabaseAdminRuntimeCommands;
use chrono::{DateTime, Duration, DurationRound, Utc};
use uuid::Uuid;

use crate::support;

fn pending_command(now: DateTime<Utc>) -> AdminRuntimeStoredCommand {
    let mut document = AdminRuntimeDocument::default();
    document.materialize_business_defaults();
    let expected_fingerprint = format!("sha256:{}", "0".repeat(64));
    let request_hash = document.request_hash(&expected_fingerprint).expect("计算请求摘要");
    AdminRuntimeStoredCommand {
        command_id: CommandId::from_uuid(Uuid::new_v4()),
        audit_id: Uuid::new_v4(),
        digest_version: ADMIN_RUNTIME_DIGEST_VERSION,
        request_hash,
        expected_fingerprint: expected_fingerprint.clone(),
        old_fingerprint: expected_fingerprint,
        new_fingerprint: document.fingerprint(),
        candidate_document: document,
        result_document: None,
        restart_required: true,
        actor_id: "admin".to_owned(),
        request_id: None,
        occurred_at: now,
        state: AdminRuntimeCommandState::Pending,
        unknown_category: None,
        created_at: now,
        updated_at: now,
        applied_at: None,
    }
}

fn is_conflict<T>(result: &Result<T, ApplicationError>) -> bool {
    matches!(result, Err(ApplicationError::AdminRuntimeCommand(AdminRuntimeCommandError::Conflict)))
}

#[tokio::test]
async fn postgres_admin_runtime_command_ledger_walks_the_full_lifecycle() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = Arc::new(database.store().await.expect("以运行账号连接"));
    let ledger = DatabaseAdminRuntimeCommands::new(Arc::clone(&store));
    let now = Utc::now().duration_trunc(Duration::seconds(1)).expect("截断到秒");
    let command = pending_command(now);
    let command_id = command.command_id;
    let request_hash = command.request_hash;

    ledger.insert_pending(command.clone()).await.expect("写入 pending 命令");
    let stored = ledger.find(command_id).await.expect("读取命令").expect("命令存在");
    assert_eq!(stored.state, AdminRuntimeCommandState::Pending);
    assert_eq!(stored.new_fingerprint, command.new_fingerprint);
    assert!(
        ledger
            .list_unresolved()
            .await
            .expect("读取未完成命令")
            .iter()
            .any(|unresolved| unresolved.command_id == command_id)
    );
    // 应用层每次请求生成新的审计 ID；同命令 ID 的并发写入在命令行主键上冲突。审计 ID 必须换新：
    // 审计主键冲突属于永久审计失败，会粘滞阻断同进程其他用例的就绪检查。
    let duplicate = AdminRuntimeStoredCommand { audit_id: Uuid::new_v4(), ..command.clone() };
    assert!(is_conflict(&ledger.insert_pending(duplicate).await), "重复写入必须冲突");

    let unknown = ledger
        .mark_unknown(
            command_id,
            request_hash,
            "current_unavailable".to_owned(),
            now + Duration::seconds(1),
        )
        .await
        .expect("标记结果未知");
    assert_eq!(unknown.state, AdminRuntimeCommandState::Unknown);
    assert_eq!(unknown.unknown_category.as_deref(), Some("current_unavailable"));
    assert!(is_conflict(
        &ledger
            .mark_unknown(
                command_id,
                RequestHash::new([9; 32]),
                "current_unavailable".to_owned(),
                now + Duration::seconds(1),
            )
            .await
    ));

    let reopened =
        ledger.reopen(command_id, request_hash, now + Duration::seconds(2)).await.expect("重开");
    assert_eq!(reopened.state, AdminRuntimeCommandState::Pending);
    assert_eq!(reopened.unknown_category, None);

    let audit = AdminRuntimeAuditRecord {
        id: command.audit_id,
        occurred_at: now + Duration::seconds(3),
        actor_id: "admin".to_owned(),
        request_id: None,
        command_id,
        old_fingerprint: command.old_fingerprint.clone(),
        new_fingerprint: command.new_fingerprint.clone(),
    };
    let applied = ledger
        .apply(command_id, request_hash, audit.clone(), now + Duration::seconds(3))
        .await
        .expect("应用命令");
    assert_eq!(applied.state, AdminRuntimeCommandState::Applied);
    assert_eq!(applied.applied_at, Some(now + Duration::seconds(3)));
    assert!(applied.result_document.is_some());
    let replay = ledger
        .apply(command_id, request_hash, audit, now + Duration::seconds(4))
        .await
        .expect("同 ID 重放");
    assert_eq!(replay.applied_at, applied.applied_at, "重放返回原结果");
    assert!(
        !ledger
            .list_unresolved()
            .await
            .expect("读取未完成命令")
            .iter()
            .any(|unresolved| unresolved.command_id == command_id),
        "已应用命令离开未完成列表"
    );

    store.postgres_pool().close().await;
}
