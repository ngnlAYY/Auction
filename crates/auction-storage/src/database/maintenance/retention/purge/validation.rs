//! 清理请求的逐数据族校验：只认新鲜演练回执与政策 v2 的闭集谓词，产出可执行的批准清单。

use chrono::{DateTime, TimeDelta, Utc};

use crate::database::maintenance::retention::inventory::{
    RetentionPolicy, retention_policy_sha256,
};

use super::model::{
    RETENTION_PURGE_DRY_RUN_MAX_DURATION_SECONDS, RETENTION_PURGE_MAX_BATCH_SIZE,
    RETENTION_PURGE_MIN_BATCH_SIZE, RETENTION_PURGE_POLICY_VERSION,
    RETENTION_PURGE_RECEIPT_MAX_AGE_SECONDS, RetentionPurgeError, RetentionPurgeRequest,
    retention_purge_approval_token,
};
use super::tables::{PURGEABLE_TABLES, cutoff_rule_id};
use super::{CLOCK_SKEW_SECONDS, ValidatedFamily, ValidatedTable, add};

pub(super) fn validate_request(
    request: &RetentionPurgeRequest,
    policy: &RetentionPolicy,
) -> Result<Vec<ValidatedFamily>, RetentionPurgeError> {
    if !crate::database::valid_confirmed_database_name(&request.database_name) {
        return Err(RetentionPurgeError::InvalidDatabaseName);
    }
    if request.dry_run_run_id.is_nil() {
        return Err(RetentionPurgeError::InvalidRunId);
    }
    if !(RETENTION_PURGE_MIN_BATCH_SIZE..=RETENTION_PURGE_MAX_BATCH_SIZE)
        .contains(&request.batch_size)
    {
        return Err(RetentionPurgeError::InvalidBatchSize);
    }
    if policy.version != RETENTION_PURGE_POLICY_VERSION
        || policy.format != "auction-retention-policy.v2"
        || !policy.deletion_requires_dry_run_and_operator_approval
    {
        return Err(RetentionPurgeError::PolicyInvalid);
    }
    let compiled_sha = retention_policy_sha256();
    if request.policy_sha256 != compiled_sha {
        return Err(RetentionPurgeError::PolicyMismatch);
    }
    let now = Utc::now();
    let skew =
        TimeDelta::try_seconds(CLOCK_SKEW_SECONDS).ok_or(RetentionPurgeError::InvalidRequest)?;
    if request.dry_run_created_at > now + skew {
        return Err(RetentionPurgeError::ReceiptInFuture);
    }
    if now - request.dry_run_created_at
        > TimeDelta::try_seconds(RETENTION_PURGE_RECEIPT_MAX_AGE_SECONDS)
            .ok_or(RetentionPurgeError::InvalidRequest)?
    {
        return Err(RetentionPurgeError::ReceiptStale);
    }
    if request.dry_run_collected_at > request.dry_run_created_at + skew
        || request.dry_run_created_at - request.dry_run_collected_at
            > TimeDelta::try_seconds(RETENTION_PURGE_DRY_RUN_MAX_DURATION_SECONDS)
                .ok_or(RetentionPurgeError::InvalidRequest)?
    {
        return Err(RetentionPurgeError::InvalidRequest);
    }
    if request.approvals.is_empty() {
        return Err(RetentionPurgeError::ApprovalInvalid);
    }

    let mut seen_families = std::collections::BTreeSet::new();
    let mut seen_tables = std::collections::BTreeSet::new();
    let mut validated = Vec::with_capacity(request.approvals.len());
    for approval in &request.approvals {
        if !seen_families.insert(approval.dataset_family.clone()) {
            return Err(RetentionPurgeError::ApprovalInvalid);
        }
        let family = policy
            .families
            .iter()
            .find(|family| family.dataset_family == approval.dataset_family)
            .ok_or(RetentionPurgeError::UnknownFamily)?;
        if family.policy_state == "retain_forever" {
            return Err(RetentionPurgeError::RetainForeverFamily);
        }
        let retention = family.retention.as_ref().ok_or(RetentionPurgeError::PolicyInvalid)?;
        let mut validated_tables = Vec::new();
        let mut table_binding = Vec::new();
        let mut sum_rows = 0_i64;
        match retention.kind.as_str() {
            "database_rows" => {
                if approval.candidate_bytes.is_some() {
                    return Err(RetentionPurgeError::CandidateCountInvalid);
                }
                for table in &family.tables {
                    let Some(table_name) = purgeable_table(table) else {
                        return Err(RetentionPurgeError::TableNotPurgeable);
                    };
                    if !seen_tables.insert(table_name.to_owned()) {
                        return Err(RetentionPurgeError::ApprovalInvalid);
                    }
                    let rule_ids = retention
                        .rules
                        .iter()
                        .filter(|rule| rule.table.as_deref() == Some(table_name))
                        .map(|rule| rule.rule_id.clone())
                        .collect::<Vec<_>>();
                    if rule_ids.is_empty() {
                        return Err(RetentionPurgeError::PolicyInvalid);
                    }
                    let approved = approval
                        .tables
                        .iter()
                        .find(|approved| approved.table == table_name)
                        .ok_or(RetentionPurgeError::ApprovalInvalid)?;
                    if approved.candidate_rows < 0
                        || approved.rule_ids != rule_ids
                        || approval
                            .tables
                            .iter()
                            .filter(|approved| approved.table == table_name)
                            .count()
                            != 1
                    {
                        return Err(RetentionPurgeError::CandidateCountInvalid);
                    }
                    let cutoff_rule =
                        cutoff_rule_id(table_name).ok_or(RetentionPurgeError::TableNotPurgeable)?;
                    let duration_days = retention
                        .rules
                        .iter()
                        .find(|rule| rule.rule_id == cutoff_rule)
                        .map(|rule| rule.duration_days)
                        .ok_or(RetentionPurgeError::PolicyInvalid)?;
                    let cutoff = retention_cutoff(request.dry_run_collected_at, duration_days)
                        .ok_or(RetentionPurgeError::InvalidRequest)?;
                    sum_rows = add(sum_rows, approved.candidate_rows)?;
                    table_binding.push((table_name.to_owned(), approved.candidate_rows));
                    validated_tables.push(ValidatedTable {
                        table: table_name,
                        rule_ids,
                        candidate_rows: approved.candidate_rows,
                        cutoff,
                    });
                }
                if approval.tables.len() != validated_tables.len()
                    || approval.candidate_rows != sum_rows
                {
                    return Err(RetentionPurgeError::CandidateCountInvalid);
                }
            }
            "filesystem" => {
                if !approval.tables.is_empty() || approval.candidate_bytes.is_none() {
                    return Err(RetentionPurgeError::FilesystemInvalid);
                }
                if approval.candidate_rows < 0 {
                    return Err(RetentionPurgeError::CandidateCountInvalid);
                }
                sum_rows = approval.candidate_rows;
                match &request.filesystem {
                    Some(result)
                        if result.deleted_files <= approval.candidate_rows.max(0) as u64 => {}
                    _ => return Err(RetentionPurgeError::FilesystemInvalid),
                }
            }
            _ => return Err(RetentionPurgeError::PolicyInvalid),
        }
        let expected = retention_purge_approval_token(
            &request.database_name,
            request.dry_run_run_id,
            &compiled_sha,
            &approval.dataset_family,
            approval.candidate_rows,
            approval.candidate_bytes,
            &table_binding,
        );
        if expected != approval.approval_token {
            return Err(RetentionPurgeError::ApprovalTokenMismatch);
        }
        validated.push(ValidatedFamily {
            dataset_family: approval.dataset_family.clone(),
            retention_kind: retention.kind.clone(),
            candidate_rows: sum_rows,
            candidate_bytes: approval.candidate_bytes,
            tables: validated_tables,
        });
    }
    if request.filesystem.is_some()
        && !validated.iter().any(|family| family.retention_kind == "filesystem")
    {
        return Err(RetentionPurgeError::FilesystemInvalid);
    }
    Ok(validated)
}

pub(super) fn purgeable_table(table: &str) -> Option<&'static str> {
    PURGEABLE_TABLES.iter().copied().find(|candidate| *candidate == table)
}

pub(super) fn retention_cutoff(now: DateTime<Utc>, duration_days: i64) -> Option<DateTime<Utc>> {
    if duration_days < 0 {
        return None;
    }
    TimeDelta::try_days(duration_days).and_then(|delta| now.checked_sub_signed(delta))
}
