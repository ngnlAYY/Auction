//! 清理文件族回执的安全读取、绑定校验与一次性审计消费。

#[cfg(unix)]
use std::{
    fs::{self, OpenOptions},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
};

#[cfg(unix)]
use auction_time::TimeDelta;
use auction_time::Timestamp;
#[cfg(unix)]
use sha2::{Digest, Sha256};
use sqlx::Postgres;
use uuid::Uuid;

use super::model::{
    RETENTION_PURGE_RECEIPT_MAX_AGE_SECONDS, RetentionPurgeApproval, RetentionPurgeError,
    RetentionPurgeFilesystemArtifact, RetentionPurgeFilesystemArtifactRef,
    RetentionPurgeFilesystemResult, RetentionPurgeOutcome, RetentionPurgeRequest,
};
use super::{AUDIT_STATEMENT_TIMEOUT, CLOCK_SKEW_SECONDS};
use crate::database::TrackedPool;
use crate::database::operations::audit_health::AuditWriteResult as _;

#[cfg(unix)]
pub(super) fn load_filesystem_artifact(
    reference: &RetentionPurgeFilesystemArtifactRef,
    request: &RetentionPurgeRequest,
    policy: &crate::database::maintenance::retention::inventory::RetentionPolicy,
    approval: &RetentionPurgeApproval,
) -> Result<(Uuid, RetentionPurgeFilesystemResult), RetentionPurgeError> {
    const MAX_ARTIFACT_BYTES: u64 = 64 * 1024;
    let invalid = || RetentionPurgeError::FilesystemInvalid;
    let path = &reference.path;
    if !path.is_absolute() {
        return Err(invalid());
    }
    let parent = path.parent().ok_or_else(invalid)?;
    let canonical_parent = fs::canonicalize(parent).map_err(|_| invalid())?;
    if canonical_parent != parent {
        return Err(invalid());
    }
    let parent_before = fs::symlink_metadata(parent).map_err(|_| invalid())?;
    if !parent_before.file_type().is_dir() || parent_before.mode() & 0o777 != 0o700 {
        return Err(invalid());
    }
    let before = fs::symlink_metadata(path).map_err(|_| invalid())?;
    if !before.file_type().is_file()
        || before.mode() & 0o777 != 0o600
        || before.nlink() != 1
        || before.uid() != parent_before.uid()
        || before.len() > MAX_ARTIFACT_BYTES
    {
        return Err(invalid());
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| invalid())?;
    let opened = file.metadata().map_err(|_| invalid())?;
    if !opened.is_file() || opened.dev() != before.dev() || opened.ino() != before.ino() {
        return Err(invalid());
    }
    let mut bytes = Vec::with_capacity(usize::try_from(before.len()).map_err(|_| invalid())?);
    (&file).take(MAX_ARTIFACT_BYTES + 1).read_to_end(&mut bytes).map_err(|_| invalid())?;
    let after = fs::symlink_metadata(path).map_err(|_| invalid())?;
    let parent_after = fs::symlink_metadata(parent).map_err(|_| invalid())?;
    if u64::try_from(bytes.len()).map_err(|_| invalid())? != before.len()
        || !after.file_type().is_file()
        || after.mode() & 0o777 != 0o600
        || after.nlink() != 1
        || after.uid() != opened.uid()
        || after.dev() != opened.dev()
        || after.ino() != opened.ino()
        || !parent_after.file_type().is_dir()
        || parent_after.dev() != parent_before.dev()
        || parent_after.ino() != parent_before.ino()
        || parent_after.uid() != parent_before.uid()
        || parent_after.mode() & 0o777 != 0o700
    {
        return Err(invalid());
    }
    let digest = Sha256::digest(&bytes);
    if digest.as_slice() != reference.sha256 {
        return Err(invalid());
    }
    let artifact: RetentionPurgeFilesystemArtifact =
        serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    let run_id = artifact.run_id;
    let directory_name = format!(".filesystem-purge-{run_id}");
    let filename = "artifact.json";
    let Some(approval_bytes) = approval.candidate_bytes else {
        return Err(invalid());
    };
    let Some(root_identity) = approval.filesystem_root_identity_sha256.as_deref() else {
        return Err(invalid());
    };
    let approval_token_sha256 =
        auction_application::core::lower_hex(&Sha256::digest(approval.approval_token.as_bytes()));
    let now = Timestamp::now();
    let expected_outcome = if artifact.failure_code.is_some() {
        RetentionPurgeOutcome::Failed
    } else if artifact.limit_reached {
        RetentionPurgeOutcome::PartialLimitReached
    } else {
        RetentionPurgeOutcome::Complete
    };
    let classified = artifact
        .retained_files
        .checked_add(artifact.write_unknown_held_files)
        .and_then(|count| count.checked_add(artifact.deleted_files))
        .ok_or(RetentionPurgeError::CountOverflow)?;
    let missing_directory_has_no_work = artifact.directory_present
        || (artifact.examined_files == 0
            && artifact.deleted_files == 0
            && artifact.deleted_bytes == 0
            && artifact.retained_files == 0
            && artifact.write_unknown_held_files == 0
            && artifact.write_unknown_over_hold_window_files == 0
            && artifact.unexpected_entries == 0
            && !artifact.limit_reached
            && !artifact.needs_follow_up
            && artifact.failure_code.is_none());
    let expected_follow_up =
        artifact.write_unknown_over_hold_window_files > 0 || artifact.unexpected_entries > 0;
    if artifact.format != "auction-retention-filesystem-purge-artifact.v1"
        || run_id.is_nil()
        || artifact.dry_run_run_id != request.dry_run_run_id
        || artifact.database_name != request.database_name
        || artifact.dataset_family != "mail_diagnostic_files"
        || artifact.approval_token_sha256 != approval_token_sha256
        || artifact.policy_version != policy.version
        || artifact.policy_sha256 != request.policy_sha256
        || artifact.filesystem_root_identity_sha256 != root_identity
        || artifact.approved_max_files
            != u64::try_from(approval.candidate_rows).map_err(|_| invalid())?
        || artifact.approved_max_files > 1_024
        || artifact.deleted_files > artifact.approved_max_files
        || artifact.deleted_bytes > approval_bytes
        || artifact.deleted_files > artifact.examined_files
        || classified > artifact.examined_files
        || artifact.write_unknown_over_hold_window_files > artifact.write_unknown_held_files
        || artifact.outcome != expected_outcome
        || artifact.needs_follow_up != expected_follow_up
        || artifact.failure_code.as_deref().is_some_and(|code| {
            !matches!(
                code,
                "evidence_file_changed"
                    | "evidence_delete_failed"
                    | "evidence_directory_sync_failed"
            )
        })
        || path.file_name().and_then(|name| name.to_str()) != Some(filename)
        || parent.file_name().and_then(|name| name.to_str()) != Some(directory_name.as_str())
        || artifact.completed_at < request.dry_run_created_at
        || artifact.completed_at > now + TimeDelta::seconds(CLOCK_SKEW_SECONDS)
        || now - artifact.completed_at > TimeDelta::seconds(RETENTION_PURGE_RECEIPT_MAX_AGE_SECONDS)
        || !missing_directory_has_no_work
    {
        return Err(invalid());
    }
    let artifact_sha256 = auction_application::core::lower_hex(&digest);
    let result = artifact.into_result(artifact_sha256);
    Ok((run_id, result))
}

#[cfg(not(unix))]
pub(super) fn load_filesystem_artifact(
    _: &RetentionPurgeFilesystemArtifactRef,
    _: &RetentionPurgeRequest,
    _: &crate::database::maintenance::retention::inventory::RetentionPolicy,
    _: &RetentionPurgeApproval,
) -> Result<(Uuid, RetentionPurgeFilesystemResult), RetentionPurgeError> {
    Err(RetentionPurgeError::FilesystemInvalid)
}

pub(super) async fn consume_filesystem_artifact(
    pool: &TrackedPool<Postgres>,
    execution_id: Uuid,
    request: &RetentionPurgeRequest,
    filesystem: &RetentionPurgeFilesystemResult,
) -> Result<(), RetentionPurgeError> {
    let mut transaction = pool
        .begin()
        .await
        .map_err(|source| RetentionPurgeError::Query { stage: "artifact_begin", source })?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "SET LOCAL statement_timeout = '{AUDIT_STATEMENT_TIMEOUT}'"
    )))
    .execute(&mut *transaction)
    .await
    .map_err(|source| RetentionPurgeError::Query { stage: "artifact_timeout", source })?;
    let lock_key = format!("auction.retention.purge.artifact:{}", filesystem.artifact_sha256);
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(&lock_key)
        .execute(&mut *transaction)
        .await
        .map_err(|source| RetentionPurgeError::Query { stage: "artifact_lock", source })?;
    let consumed: bool = sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM public.audit_records
             WHERE (action = 'auction.retention.purge.artifact_consumed'
                    AND metadata->>'artifact_sha256' = $1)
                OR (action = 'auction.retention.purge'
                    AND metadata #>> '{filesystem,artifact_sha256}' = $1)
         )",
    )
    .bind(&filesystem.artifact_sha256)
    .fetch_one(&mut *transaction)
    .await
    .map_err(|source| RetentionPurgeError::Query { stage: "artifact_replay_check", source })?;
    if consumed {
        let _ = transaction.rollback().await;
        return Err(RetentionPurgeError::FilesystemArtifactReplay);
    }
    let metadata = auction_application::core::cli_audit_metadata(
        request.operator_identity.as_ref(),
        serde_json::json!({
            "database_name": request.database_name,
            "dry_run_run_id": request.dry_run_run_id,
            "execution_id": execution_id,
            "artifact_sha256": filesystem.artifact_sha256,
            "filesystem": filesystem,
        }),
    );
    sqlx::query(
        "INSERT INTO public.audit_records
             (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
         VALUES ($1, 'system', 'auction-cli', 'auction.retention.purge.artifact_consumed',
                 'retention_purge_artifact', $2, 'success', 'system', $3)",
    )
    .bind(Uuid::new_v4())
    .bind(&filesystem.artifact_sha256)
    .bind(metadata)
    .execute(&mut *transaction)
    .await
    .record_audit_failure()
    .map_err(|source| RetentionPurgeError::Query { stage: "artifact_consume", source })?;
    transaction
        .commit()
        .await
        .map_err(|source| RetentionPurgeError::Commit { stage: "artifact_commit", source })
}
