//! Admin runtime 配置命令账本的完整生命周期，以运行账号执行。
//!
//! pending → unknown → 重开 → applied 依次经过全部查询与 mutation 语句（它们共用同一份列清单）；
//! 已应用命令同 ID 重放返回原结果、离开未完成列表；请求摘要不符与重复写入都按冲突拒绝。
//!
//! 命令身份经账本往返：真实更新用例接数据库账本与内存文件，同 ID 省略与显式阈值互判冲突、原样
//! 重试不回写文件；升级前程序按当时的列与摘要编码写下的版本 1 行仍按原口径重放；摘要版本闭集
//! 只有 1、2。
//!
//! 仅在 `test-db` feature 下编译，入口见 `mise run test-db`。
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use auction_application::admin::{
    ADMIN_RUNTIME_DIGEST_VERSION, ADMIN_RUNTIME_LEGACY_DIGEST_VERSION, AdminRuntimeAuditRecord,
    AdminRuntimeCommandError, AdminRuntimeCommandState, AdminRuntimeCommandStore,
    AdminRuntimeConfigError, AdminRuntimeConfigService, AdminRuntimeDocument,
    AdminRuntimeFileStore, AdminRuntimeReplacement, AdminRuntimeSnapshot,
    AdminRuntimeStoredCommand, AdminRuntimeUpdateCommand,
};
use auction_application::core::{ApplicationError, ApplicationFuture, Clock, RequestHash};
use auction_domain::CommandId;
use auction_storage::DatabaseAdminRuntimeCommands;
use auction_time::{Duration, Timestamp};
use sqlx::PgPool;
use uuid::Uuid;

use crate::support;

fn pending_command(now: Timestamp) -> AdminRuntimeStoredCommand {
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
    let now = Timestamp::now().duration_trunc(Duration::seconds(1)).expect("截断到秒");
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

/// 内存 runtime 文件：替换按指纹 fence 校验并计数，供断言重放不回写文件。
struct MemoryFiles {
    document: Mutex<AdminRuntimeDocument>,
    writes: AtomicUsize,
}

impl MemoryFiles {
    fn snapshot(&self) -> AdminRuntimeSnapshot {
        let document = self.document.lock().expect("文件锁").clone();
        AdminRuntimeSnapshot {
            fingerprint: document.fingerprint(),
            document,
            restart_required: false,
        }
    }

    fn writes(&self) -> usize {
        self.writes.load(Ordering::SeqCst)
    }
}

impl AdminRuntimeFileStore for MemoryFiles {
    type WorkflowGuard = ();

    fn acquire_workflow_lock(&self) -> ApplicationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }

    fn validate_candidate(
        &self,
        _document: &AdminRuntimeDocument,
        _now: Timestamp,
    ) -> Result<(), ApplicationError> {
        Ok(())
    }

    fn current(&self) -> ApplicationFuture<'_, AdminRuntimeSnapshot> {
        Box::pin(async { Ok(self.snapshot()) })
    }

    fn current_locked<'a>(&'a self, _guard: &'a ()) -> ApplicationFuture<'a, AdminRuntimeSnapshot> {
        Box::pin(async { Ok(self.snapshot()) })
    }

    fn replace_locked<'a>(
        &'a self,
        _guard: &'a (),
        expected_fingerprint: String,
        document: AdminRuntimeDocument,
    ) -> ApplicationFuture<'a, AdminRuntimeReplacement> {
        Box::pin(async move {
            let mut current = self.document.lock().expect("文件锁");
            let old_fingerprint = current.fingerprint();
            assert_eq!(old_fingerprint, expected_fingerprint, "替换必须按指纹 fence");
            let new_fingerprint = document.fingerprint();
            *current = document;
            self.writes.fetch_add(1, Ordering::SeqCst);
            Ok(AdminRuntimeReplacement { old_fingerprint, new_fingerprint })
        })
    }
}

struct WallClock;

impl Clock for WallClock {
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }
}

/// 阈值按参数省略或显式写出；扫描间隔按参数改写，保证每次保存都改变文件。
fn document_with(threshold: Option<u64>, scan_interval_seconds: u64) -> AdminRuntimeDocument {
    let mut document = AdminRuntimeDocument::default();
    document.materialize_business_defaults();
    if let Some(operations) = &mut document.operations {
        operations.alerts.fulfillment_collection_overdue_age_seconds = threshold;
        operations.alerts.scan_interval_seconds = scan_interval_seconds;
    }
    document
}

fn threshold(document: &AdminRuntimeDocument) -> Option<u64> {
    document
        .operations
        .as_ref()
        .and_then(|operations| operations.alerts.fulfillment_collection_overdue_age_seconds)
}

fn update(
    command_id: CommandId,
    expected_fingerprint: &str,
    document: AdminRuntimeDocument,
) -> AdminRuntimeUpdateCommand {
    AdminRuntimeUpdateCommand {
        command_id,
        expected_fingerprint: expected_fingerprint.to_owned(),
        request_hash: document.request_hash(expected_fingerprint).expect("请求摘要"),
        document,
        actor_id: "admin".to_owned(),
        request_id: None,
    }
}

fn is_command_conflict(result: &Result<AdminRuntimeSnapshot, ApplicationError>) -> bool {
    matches!(result, Err(ApplicationError::AdminRuntime(AdminRuntimeConfigError::CommandConflict)))
}

/// 按升级前程序的列与写法插入一条已应用的版本 1 账本行：摘要按候选计算，候选即原始请求。
async fn insert_legacy_row(
    pool: &PgPool,
    command_id: CommandId,
    digest_version: i16,
    expected_fingerprint: &str,
    candidate: &AdminRuntimeDocument,
) -> Result<(), sqlx::Error> {
    let audit_id = Uuid::new_v4();
    let at = Timestamp::now().duration_trunc(Duration::seconds(1)).expect("截断到秒")
        - Duration::days(1);
    sqlx::query(
        "INSERT INTO public.audit_records
            (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id, result,
             source, metadata)
         VALUES ($1, $2, 'admin', 'admin', 'config.runtime_update', 'runtime_config', 'current',
                 'success', 'admin', '{}'::jsonb)",
    )
    .bind(audit_id)
    .bind(at)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO public.runtime_config_admin_commands
            (command_id, digest_version, request_digest, state, expected_fingerprint,
             old_fingerprint, new_fingerprint, candidate_document, restart_required, audit_id,
             actor_id, request_id, occurred_at, unknown_category, created_at, updated_at,
             applied_at)
         VALUES ($1, $2, $3, 'applied', $4, $4, $5, $6, TRUE, $7, 'admin', NULL, $8, '', $8, $8,
                 $8)",
    )
    .bind(command_id.as_uuid())
    .bind(digest_version)
    .bind(
        candidate
            .legacy_request_hash(expected_fingerprint)
            .expect("旧版摘要")
            .as_bytes()
            .as_slice(),
    )
    .bind(expected_fingerprint)
    .bind(candidate.fingerprint())
    .bind(serde_json::to_vec(candidate).expect("编码旧候选"))
    .bind(audit_id)
    .bind(at)
    .execute(pool)
    .await?;
    Ok(())
}

/// 不变量 1 经数据库账本往返：账本保存原始请求摘要（版本 2），同 ID 先显式阈值、再省略阈值判为
/// 冲突，原样重试返回原结果且不回写文件；省略阈值的请求摘要不同于补回后候选的摘要。升级前程序
/// 写下的版本 1 行照旧可读、可校验，原样重试返回原结果，显式写出不同阈值冲突。摘要版本闭集
/// 只放宽到 1、2。
#[tokio::test]
async fn postgres_admin_runtime_replay_identity_survives_the_ledger() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = Arc::new(database.store().await.expect("以运行账号连接"));
    let pool = store.postgres_pool();
    let ledger = Arc::new(DatabaseAdminRuntimeCommands::new(Arc::clone(&store)));
    let files = Arc::new(MemoryFiles {
        document: Mutex::new(document_with(Some(86_400), 60)),
        writes: AtomicUsize::new(0),
    });
    let service = AdminRuntimeConfigService::new(
        Arc::clone(&ledger),
        Arc::clone(&files),
        Arc::new(WallClock),
    );

    let initial = files.snapshot().fingerprint;
    let explicit = CommandId::from_uuid(Uuid::new_v4());
    let saved = service
        .update(update(explicit, &initial, document_with(Some(3_600), 120)))
        .await
        .expect("显式阈值保存");
    assert_eq!(threshold(&saved.document), Some(3_600));
    assert!(
        is_command_conflict(
            &service.update(update(explicit, &initial, document_with(None, 120))).await
        ),
        "同 ID 改为省略阈值必须冲突"
    );
    let replayed = service
        .update(update(explicit, &initial, document_with(Some(3_600), 120)))
        .await
        .expect("原样重试返回原结果");
    assert_eq!(replayed, saved);
    assert_eq!(files.writes(), 1, "冲突与重放都不写文件");

    // 省略阈值的新命令：账本记原始请求摘要，候选补回当前文件的 3600，只经 new_fingerprint 绑定。
    let request = document_with(None, 180);
    let omitted = CommandId::from_uuid(Uuid::new_v4());
    let resolved = service
        .update(update(omitted, &saved.fingerprint, request.clone()))
        .await
        .expect("省略阈值保存");
    assert_eq!(threshold(&resolved.document), Some(3_600), "省略表示保持当前值");
    let row = ledger.find(omitted).await.expect("读取账本").expect("账本行存在");
    assert_eq!(row.digest_version, ADMIN_RUNTIME_DIGEST_VERSION);
    assert_eq!(row.request_hash, request.request_hash(&saved.fingerprint).expect("请求摘要"));
    assert_ne!(
        row.request_hash,
        resolved.document.request_hash(&saved.fingerprint).expect("候选摘要"),
        "补回后的候选不充当请求身份"
    );
    assert_eq!(row.new_fingerprint, resolved.fingerprint);
    assert!(
        is_command_conflict(
            &service
                .update(update(omitted, &saved.fingerprint, document_with(Some(3_600), 180)))
                .await
        ),
        "同 ID 改为显式写出与当前文件相同的阈值也是不同请求"
    );
    assert_eq!(files.writes(), 2);

    // 升级前程序写下的版本 1 行：候选即原始请求，不含长期未领取阈值。
    let pre_upgrade_file = document_with(None, 60).fingerprint();
    let legacy_request = document_with(None, 120);
    let legacy = CommandId::from_uuid(Uuid::new_v4());
    insert_legacy_row(
        pool,
        legacy,
        i16::try_from(ADMIN_RUNTIME_LEGACY_DIGEST_VERSION).expect("版本号"),
        &pre_upgrade_file,
        &legacy_request,
    )
    .await
    .expect("写入升级前的账本行");
    let replayed = service
        .update(update(legacy, &pre_upgrade_file, legacy_request.clone()))
        .await
        .expect("旧命令原样重试返回原结果");
    assert_eq!(replayed.document, legacy_request);
    assert_eq!(replayed.fingerprint, legacy_request.fingerprint());
    assert!(
        is_command_conflict(
            &service
                .update(update(legacy, &pre_upgrade_file, document_with(Some(3_600), 120)))
                .await
        ),
        "旧命令同 ID 改为显式阈值仍冲突"
    );
    assert_eq!(files.writes(), 2, "旧命令重放不回写文件");
    assert_eq!(files.snapshot().document, resolved.document, "当前文件保持最后一次保存");

    let unknown_version = insert_legacy_row(
        pool,
        CommandId::from_uuid(Uuid::new_v4()),
        3,
        &pre_upgrade_file,
        &legacy_request,
    )
    .await
    .expect_err("摘要版本闭集只有 1、2");
    assert_eq!(
        unknown_version.as_database_error().and_then(sqlx::error::DatabaseError::code).as_deref(),
        Some("23514")
    );

    pool.close().await;
}
