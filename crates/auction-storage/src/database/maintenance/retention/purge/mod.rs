//! 保留清理的有界执行：只接受一次新鲜只读演练回执派生的逐数据族批准，按政策 v2 的闭集
//! 谓词分批删除运行态行与加密诊断文件。审计、资金、业务事实、有效/永久会话、未解决失败
//! 与写入未知诊断永不在删除范围；每批一个事务，失败时已提交批次保留并在回执中精确计数。

use auction_time::Timestamp;
use uuid::Uuid;

use crate::database::DatabaseStore;
use crate::database::maintenance::retention::inventory::{
    load_embedded_retention_policy, retention_policy_sha256,
};

pub(super) mod database;
mod filesystem;
mod model;
mod tables;
mod validation;
use self::database::{
    add, append_audit, delete_batch, execution_rank, log_purge_failure, purge_failure_code,
    verify_target,
};
use self::filesystem::{consume_filesystem_artifact, load_filesystem_artifact};
use self::tables::{RemainingCandidates, count_remaining, delete_statement};
pub use model::*;
use validation::validate_request;

const STATEMENT_TIMEOUT: &str = "30s";
const LOCK_TIMEOUT: &str = "2s";
const AUDIT_STATEMENT_TIMEOUT: &str = "10s";
/// 演练回执时间允许的时钟偏移。
const CLOCK_SKEW_SECONDS: i64 = 120;

struct ValidatedTable {
    table: &'static str,
    rule_ids: Vec<String>,
    candidate_rows: i64,
    cutoff: Timestamp,
}

struct ValidatedFamily {
    dataset_family: String,
    retention_kind: String,
    candidate_rows: i64,
    candidate_bytes: Option<u64>,
    tables: Vec<ValidatedTable>,
}

impl DatabaseStore {
    /// 在任何变更前完成全部预校验：政策/令牌/计数/新鲜度与数据库身份、迁移前缀。
    /// 文件族预检只验证批准与政策绑定；执行入口要求实际文件清理生成的 artifact reference。
    /// CLI 通过根身份已绑定的批准令牌证明其预检时使用的配置根。
    pub async fn preflight_retention_purge(
        &self,
        request: &RetentionPurgeRequest,
    ) -> Result<(), RetentionPurgeError> {
        let policy = load_embedded_retention_policy()
            .map_err(|_: crate::RetentionInventoryError| RetentionPurgeError::PolicyInvalid)?;
        let _ = validate_request(request, &policy)?;
        verify_target(self, &request.database_name).await
    }

    /// 执行一次已批准的保留清理。预校验失败不产生任何变更并返回错误；一旦开始执行，
    /// 无论完成、部分完成还是中途失败都返回回执，并由审计事实固定结果。
    pub async fn execute_retention_purge(
        &self,
        request: RetentionPurgeRequest,
    ) -> Result<RetentionPurgeReport, RetentionPurgeError> {
        let policy = load_embedded_retention_policy()
            .map_err(|_: crate::RetentionInventoryError| RetentionPurgeError::PolicyInvalid)?;
        let validated = validate_request(&request, &policy)?;
        verify_target(self, &request.database_name).await?;
        let mut filesystem_artifact_consume_failed = false;
        let filesystem = if let Some(family) =
            validated.iter().find(|family| family.retention_kind == "filesystem")
        {
            let approval = request
                .approvals
                .iter()
                .find(|approval| approval.dataset_family == family.dataset_family)
                .ok_or(RetentionPurgeError::FilesystemInvalid)?;
            let reference = request
                .filesystem_artifact
                .as_ref()
                .ok_or(RetentionPurgeError::FilesystemInvalid)?;
            let reference = reference.clone();
            let request_for_read = request.clone();
            let policy_for_read = policy.clone();
            let approval_for_read = approval.clone();
            let (run_id, result) = tokio::task::spawn_blocking(move || {
                load_filesystem_artifact(
                    &reference,
                    &request_for_read,
                    &policy_for_read,
                    &approval_for_read,
                )
            })
            .await
            .map_err(|error| {
                tracing::warn!(
                    cancelled = error.is_cancelled(),
                    panicked = error.is_panic(),
                    "读取保留清理文件回执的后台任务失败"
                );
                RetentionPurgeError::FilesystemInvalid
            })??;
            if let Err(error) =
                consume_filesystem_artifact(self.pool(), run_id, &request, &result).await
            {
                if matches!(&error, RetentionPurgeError::FilesystemArtifactReplay) {
                    return Err(error);
                }
                log_purge_failure(&error, "保留清理文件回执消费失败");
                filesystem_artifact_consume_failed = true;
            }
            Some((run_id, result))
        } else {
            if request.filesystem_artifact.is_some() {
                return Err(RetentionPurgeError::FilesystemInvalid);
            }
            None
        };
        // 先清理已解决的推送失败，再清理事件：删除失败记录可能解除事件的 RESTRICT 引用，
        // 使同一批批准上限内的事件候选取值单调收敛；反向顺序会留下只能靠下一轮演练清理的行。
        let mut order = (0..validated.len()).collect::<Vec<_>>();
        order.sort_by_key(|index| execution_rank(&validated[*index].dataset_family));

        let run_id = filesystem.as_ref().map_or_else(Uuid::new_v4, |(run_id, _)| *run_id);
        let started_at = Timestamp::now();
        // 文件族可能已由调用方在预校验之后删除；即使后续数据库批次失败，也必须如实计入。
        let mut mutation_executed =
            filesystem.as_ref().is_some_and(|(_, result)| result.deleted_files > 0);
        let mut failure_code: Option<&'static str> =
            filesystem_artifact_consume_failed.then_some("filesystem_artifact_audit_failed");
        let mut families = Vec::with_capacity(validated.len());
        let mut skipped = Vec::new();
        let mut unknown_remaining = Vec::new();

        for index in &order {
            let family = &validated[*index];
            if family.retention_kind != "filesystem" && failure_code.is_some() {
                skipped.push(family.dataset_family.clone());
                continue;
            }
            if family.retention_kind == "filesystem" {
                // 文件族已在调用方按批准上限执行；这里只把实际 unlink 计数并入回执与审计。
                let result = filesystem
                    .as_ref()
                    .map(|(_, result)| result)
                    .ok_or(RetentionPurgeError::FilesystemInvalid)?;
                if result.failure_code.is_some() {
                    failure_code.get_or_insert("filesystem_purge_failed");
                }
                let outcome = if result.failure_code.is_some() {
                    RetentionPurgeOutcome::Failed
                } else if result.limit_reached {
                    RetentionPurgeOutcome::PartialLimitReached
                } else {
                    RetentionPurgeOutcome::Complete
                };
                families.push(RetentionPurgeFamilyResult {
                    dataset_family: family.dataset_family.clone(),
                    retention_kind: family.retention_kind.clone(),
                    approved_rows: family.candidate_rows,
                    approved_bytes: family.candidate_bytes,
                    deleted_rows: i64::try_from(result.deleted_files).map_err(
                        |_: std::num::TryFromIntError| RetentionPurgeError::CountOverflow,
                    )?,
                    deleted_bytes: i64::try_from(result.deleted_bytes).map_err(
                        |_: std::num::TryFromIntError| RetentionPurgeError::CountOverflow,
                    )?,
                    tables: Vec::new(),
                    outcome,
                });
                continue;
            }

            let mut table_results = Vec::with_capacity(family.tables.len());
            let mut family_outcome = RetentionPurgeOutcome::Complete;
            for table in &family.tables {
                let Some(statement) = delete_statement(table.table) else {
                    failure_code = Some("table_not_purgeable");
                    break;
                };
                let mut deleted_rows = 0_i64;
                let mut deleted_row_bytes = 0_i64;
                let mut batches = 0_u32;
                let mut table_failure = None;
                let mut remaining_limit = table.candidate_rows;
                while remaining_limit > 0 {
                    let limit = remaining_limit.min(i64::from(request.batch_size));
                    match delete_batch(self, statement, table.cutoff, limit).await {
                        Ok((rows, bytes)) => {
                            if rows == 0 {
                                break;
                            }
                            mutation_executed = true;
                            batches = batches.saturating_add(1);
                            deleted_rows = add(deleted_rows, rows)?;
                            deleted_row_bytes = add(deleted_row_bytes, bytes)?;
                            remaining_limit -= rows;
                        }
                        Err(error) => {
                            // 回执只留失败码；SQLSTATE 等安全投影只在日志中保留。
                            log_purge_failure(&error, "保留清理删除批次失败");
                            table_failure = Some(error);
                            break;
                        }
                    }
                }
                let remaining = match count_remaining(self.pool(), table.table, table.cutoff).await
                {
                    Ok(counts) => counts,
                    Err(error) => {
                        // 保留首个失败：删除批次失败后的计数失败（常见于同一次连接中断）不得覆盖
                        // 删除阶段的失败码。剩余量未知时字段仍按 0 填充（不改 v1 字段类型），
                        // 并在备注中写明。
                        log_purge_failure(&error, "保留清理剩余候选计数失败");
                        if table_failure.is_none() {
                            table_failure = Some(error);
                        }
                        unknown_remaining.push(table.table);
                        RemainingCandidates { remaining: 0, blocked: 0 }
                    }
                };
                if remaining.remaining > 0 {
                    family_outcome = RetentionPurgeOutcome::PartialLimitReached;
                }
                table_results.push(RetentionPurgeTableResult {
                    table: table.table.to_owned(),
                    rule_ids: table.rule_ids.clone(),
                    approved_rows: table.candidate_rows,
                    deleted_rows,
                    deleted_row_bytes,
                    batches,
                    remaining_candidate_rows: remaining.remaining,
                    blocked_candidate_rows: remaining.blocked,
                });
                if let Some(error) = table_failure {
                    failure_code = Some(purge_failure_code(&error));
                    break;
                }
            }
            if failure_code.is_some() {
                family_outcome = RetentionPurgeOutcome::Failed;
            }
            let deleted_rows = table_results
                .iter()
                .try_fold(0_i64, |total, table| add(total, table.deleted_rows))?;
            let deleted_bytes = table_results
                .iter()
                .try_fold(0_i64, |total, table| add(total, table.deleted_row_bytes))?;
            families.push(RetentionPurgeFamilyResult {
                dataset_family: family.dataset_family.clone(),
                retention_kind: family.retention_kind.clone(),
                approved_rows: family.candidate_rows,
                approved_bytes: family.candidate_bytes,
                deleted_rows,
                deleted_bytes,
                tables: table_results,
                outcome: family_outcome,
            });
        }

        // 顶层计数只汇总数据库行；文件族的文件数/字节只在 `filesystem` 与族结果中报告。
        let (deleted_rows, deleted_row_bytes) = families
            .iter()
            .filter(|family| family.retention_kind == "database_rows")
            .try_fold((0_i64, 0_i64), |(rows, bytes), family| {
                Ok::<_, RetentionPurgeError>((
                    add(rows, family.deleted_rows)?,
                    add(bytes, family.deleted_bytes)?,
                ))
            })?;
        let outcome = if failure_code.is_some() {
            RetentionPurgeOutcome::Failed
        } else if families
            .iter()
            .any(|family| family.outcome == RetentionPurgeOutcome::PartialLimitReached)
        {
            RetentionPurgeOutcome::PartialLimitReached
        } else {
            RetentionPurgeOutcome::Complete
        };
        let mut notes = vec![
            "只删除政策 v2 运行态数据族的候选；审计、资金、业务历史、身份与配置事实永不删除"
                .to_owned(),
            "有效会话（含永久会话）、未解决失败与写入未知诊断永不在删除范围".to_owned(),
            "候选边界为演练快照时间减期限；已注销会话立即候选".to_owned(),
            "每批一个事务；失败时已提交批次保留，回执与审计记录精确计数".to_owned(),
            "执行顺序先清理已解决的推送失败、再清理事件，使同轮解除引用的事件也可在批准上限内删除"
                .to_owned(),
            "deleted_row_bytes 为删除时 pg_column_size 的行载荷字节数，不是物理页面大小".to_owned(),
        ];
        if !skipped.is_empty() {
            notes.push(format!("失败后跳过的数据族：{}", skipped.join(", ")));
        }
        if !unknown_remaining.is_empty() {
            notes.push(format!(
                "剩余候选计数失败的表：{}；其剩余与阻断计数记为 0，实际未知",
                unknown_remaining.join(", ")
            ));
        }

        let mut report = RetentionPurgeReport {
            format: RETENTION_PURGE_RECEIPT_FORMAT,
            run_id,
            dry_run_run_id: request.dry_run_run_id,
            database_name: request.database_name.clone(),
            policy_version: policy.version,
            policy_sha256: retention_policy_sha256(),
            batch_size: request.batch_size,
            started_at,
            finished_at: Timestamp::now(),
            outcome,
            failure_code,
            families,
            deleted_rows,
            deleted_row_bytes,
            filesystem: filesystem.map(|(_, result)| result),
            audit_id: None,
            mutation_executed,
            deletion_authorized: true,
            notes,
        };
        match append_audit(self.pool(), &report, request.operator_identity.as_ref()).await {
            Ok(audit_id) => report.audit_id = Some(audit_id),
            Err(error) => {
                // 审计追加失败不得静默：回执标记失败，日志只记阶段与 SQLSTATE 等安全投影。
                log_purge_failure(&error, "保留清理审计追加失败");
                report.outcome = RetentionPurgeOutcome::Failed;
                report.failure_code = Some("audit_append_failed");
            }
        }
        report.finished_at = Timestamp::now();
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::tables::{PURGEABLE_TABLES, delete_statement, remaining_statement};
    use super::validation::purgeable_table;

    #[test]
    fn allowlist_is_exactly_the_runtime_families() {
        assert_eq!(
            PURGEABLE_TABLES,
            [
                "service_heartbeats",
                "realtime_events",
                "realtime_failures",
                "public_sessions",
                "admin_sessions",
            ]
        );
        for protected in [
            "audit_records",
            "bids",
            "compensation_ledger",
            "lot_settlements",
            "users",
            "user_login_key_envelopes",
            "deliveries",
            "inbound_mail",
        ] {
            assert!(purgeable_table(protected).is_none(), "{protected}");
            assert!(delete_statement(protected).is_none(), "{protected}");
            assert!(remaining_statement(protected).is_none(), "{protected}");
        }
    }

    #[test]
    fn every_purgeable_table_has_guarded_delete_and_count() {
        for table in PURGEABLE_TABLES {
            let delete = delete_statement(table).expect("delete statement");
            assert!(delete.contains("pg_column_size(t)"), "{table}");
            assert!(delete.contains("LIMIT $2"), "{table}");
            assert!(remaining_statement(table).is_some(), "{table}");
        }
        let events = delete_statement("realtime_events").expect("events");
        assert!(events.contains("realtime_failures"));
        assert!(events.contains("replay_of_event_id"));
        let failures = delete_statement("realtime_failures").expect("failures");
        assert!(failures.contains("resolved_at IS NOT NULL"));
        for sessions in ["public_sessions", "admin_sessions"] {
            let statement = delete_statement(sessions).expect("sessions");
            assert!(statement.contains("revoked_at IS NOT NULL"), "{sessions}");
            assert!(statement.contains("expires_at IS NOT NULL"), "{sessions}");
        }
    }
}
