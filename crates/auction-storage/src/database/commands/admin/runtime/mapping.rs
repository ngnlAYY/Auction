//! PostgreSQL runtime 命令账本行到应用安全模型的映射。

#[cfg(feature = "postgres")]
use auction_application::admin::{
    AdminRuntimeCommandState, AdminRuntimeDocument, AdminRuntimeStoredCommand,
};
#[cfg(feature = "postgres")]
use auction_application::core::{ApplicationError, RequestHash};
#[cfg(feature = "postgres")]
use auction_domain::CommandId;
#[cfg(feature = "postgres")]
use sqlx::{Row, postgres::PgRow};
#[cfg(feature = "postgres")]
use uuid::Uuid;

#[cfg(feature = "postgres")]
use super::errors::{command_corrupt, command_failed};

#[cfg(feature = "postgres")]
pub(super) fn map_command_row(row: PgRow) -> Result<AdminRuntimeStoredCommand, ApplicationError> {
    let command_id: Uuid =
        row.try_get("command_id").map_err(|_| command_failed("runtime 命令标识读取失败"))?;
    let digest_version: i16 = row
        .try_get("digest_version")
        .map_err(|_| command_failed("runtime 命令摘要版本读取失败"))?;
    let request_digest: Vec<u8> =
        row.try_get("request_digest").map_err(|_| command_failed("runtime 命令摘要读取失败"))?;
    let request_digest: [u8; 32] =
        request_digest.try_into().map_err(|_| command_failed("runtime 命令摘要长度无效"))?;
    let candidate_bytes: Vec<u8> = row
        .try_get("candidate_document")
        .map_err(|_| command_failed("runtime 候选文档读取失败"))?;
    let candidate_document: AdminRuntimeDocument = serde_json::from_slice(&candidate_bytes)
        .map_err(|_| command_corrupt("runtime 候选文档不是有效 JSON".to_owned()))?;
    let state = match row
        .try_get::<String, _>("state")
        .map_err(|_| command_failed("runtime 命令状态读取失败"))?
        .as_str()
    {
        "pending" => AdminRuntimeCommandState::Pending,
        "applied" => AdminRuntimeCommandState::Applied,
        "unknown" => AdminRuntimeCommandState::Unknown,
        _ => return Err(command_corrupt("runtime 命令状态无效".to_owned())),
    };
    let unknown_category: String = row
        .try_get("unknown_category")
        .map_err(|_| command_failed("runtime 命令未知分类读取失败"))?;
    let result_document =
        (state == AdminRuntimeCommandState::Applied).then(|| candidate_document.clone());
    let command = AdminRuntimeStoredCommand {
        command_id: CommandId::from_uuid(command_id),
        audit_id: row
            .try_get("audit_id")
            .map_err(|_| command_failed("runtime 审计标识读取失败"))?,
        digest_version: u16::try_from(digest_version)
            .map_err(|_| command_corrupt("runtime 命令摘要版本无效".to_owned()))?,
        request_hash: RequestHash::new(request_digest),
        expected_fingerprint: row
            .try_get("expected_fingerprint")
            .map_err(|_| command_failed("runtime expected fingerprint 读取失败"))?,
        old_fingerprint: row
            .try_get("old_fingerprint")
            .map_err(|_| command_failed("runtime old fingerprint 读取失败"))?,
        new_fingerprint: row
            .try_get("new_fingerprint")
            .map_err(|_| command_failed("runtime new fingerprint 读取失败"))?,
        candidate_document,
        result_document,
        restart_required: row
            .try_get("restart_required")
            .map_err(|_| command_failed("runtime restart 标记读取失败"))?,
        actor_id: row.try_get("actor_id").map_err(|_| command_failed("runtime actor 读取失败"))?,
        request_id: row
            .try_get("request_id")
            .map_err(|_| command_failed("runtime request_id 读取失败"))?,
        occurred_at: row
            .try_get("occurred_at")
            .map_err(|_| command_failed("runtime occurred_at 读取失败"))?,
        state,
        unknown_category: (!unknown_category.is_empty()).then_some(unknown_category),
        created_at: row
            .try_get("created_at")
            .map_err(|_| command_failed("runtime created_at 读取失败"))?,
        updated_at: row
            .try_get("updated_at")
            .map_err(|_| command_failed("runtime updated_at 读取失败"))?,
        applied_at: row
            .try_get("applied_at")
            .map_err(|_| command_failed("runtime applied_at 读取失败"))?,
    };
    command.validate().map_err(|error| command_corrupt(error.to_string()))?;
    Ok(command)
}
