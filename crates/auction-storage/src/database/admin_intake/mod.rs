//! PostgreSQL Admin 装备录入 owner 事务。
//!
//! 该事务只接收应用层脱敏写入计划。原始 URL、canonical JSON 和明文装备 key 都不进入
//! storage crate；命令账本占位、装备事实、密钥信封、投稿、pending lot、版本推进和审计/实时
//! 事件在同一数据库事务中完成。

use std::future::Future;

use auction_application::{
    AdminIntakeRecord, AdminIntakeResult, AdminIntakeTransaction, AdminIntakeWrite,
    CompetitionTransactionError,
};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

use crate::database::audit_health::AuditWriteResult as _;

use super::competition::{PostgresCompetitionTransaction, map_sql_error};

#[allow(clippy::manual_async_fn)]
impl AdminIntakeTransaction for PostgresCompetitionTransaction {
    fn add_admin_intake(
        &mut self,
        write: AdminIntakeWrite,
    ) -> impl Future<Output = Result<AdminIntakeResult, CompetitionTransactionError>> + Send {
        async move { add_admin_intake_inner(self, write).await }
    }

    fn append_admin_intake_record(
        &mut self,
        record: AdminIntakeRecord,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send {
        async move { append_admin_intake_record_inner(self, record).await }
    }
}

async fn add_admin_intake_inner(
    transaction: &mut PostgresCompetitionTransaction,
    write: AdminIntakeWrite,
) -> Result<AdminIntakeResult, CompetitionTransactionError> {
    write.validate().map_err(|_| failed("Admin 装备录入写入计划无效"))?;
    let transaction = transaction.transaction_mut()?;

    // 投稿表以 source_command_id 引用账本，因此先写入事务内的占位记录，再在全部事实
    // 成功后回填 accepted 结果；事务回滚时占位记录也会消失。
    sqlx::query(
        r#"INSERT INTO public.auction_commands
            (command_id, command_kind, aggregate_id, request_hash, result_code, result_payload)
           VALUES ($1, 'admin_auction_intake_item', $2, $3, 'pending', $4)"#,
    )
    .bind(write.command_id.as_uuid())
    .bind(write.aggregate_id)
    .bind(write.request_hash.as_bytes().to_vec())
    .bind(json!({}))
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;

    let world_enabled =
        sqlx::query_scalar::<_, bool>("SELECT enabled FROM public.worlds WHERE id = $1 FOR UPDATE")
            .bind(&write.world_id)
            .fetch_optional(&mut **transaction)
            .await
            .map_err(map_sql_error)?
            .ok_or(CompetitionTransactionError::NotFound)?;
    if !world_enabled {
        return Err(CompetitionTransactionError::Conflict);
    }

    let target = sqlx::query(
        r#"SELECT target.auction_id,
                  auction.world_id,
                  auction.sequence,
                  auction.status,
                  auction.state_version,
                  auction.preview_revision
           FROM public.auction_intake_targets AS target
           INNER JOIN public.auctions AS auction ON auction.id = target.auction_id
           WHERE target.world_id = $1
           FOR UPDATE OF target, auction"#,
    )
    .bind(&write.world_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    .ok_or(CompetitionTransactionError::NotFound)?;
    let auction_id: Uuid = target.try_get("auction_id").map_err(map_sql_error)?;
    let auction_world_id: String = target.try_get("world_id").map_err(map_sql_error)?;
    let auction_sequence: i64 = target.try_get("sequence").map_err(map_sql_error)?;
    let auction_status: String = target.try_get("status").map_err(map_sql_error)?;
    let current_state_version: i64 = target.try_get("state_version").map_err(map_sql_error)?;
    let current_preview_revision: i64 =
        target.try_get("preview_revision").map_err(map_sql_error)?;
    if auction_world_id != write.world_id
        || auction_sequence <= 0
        || auction_status != "draft"
        || current_state_version <= 0
        || current_preview_revision < 0
    {
        return Err(CompetitionTransactionError::Conflict);
    }

    let player_status = sqlx::query_scalar::<_, String>(
        "SELECT status FROM public.users WHERE game_player_id = $1 FOR SHARE",
    )
    .bind(&write.player_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    if player_status.as_deref() == Some("blacklisted") {
        return Err(CompetitionTransactionError::Conflict);
    }

    let existing_equipment = sqlx::query_scalar::<_, Uuid>(
        r#"SELECT id
           FROM public.equipment
           WHERE world_id = $1 AND external_equipment_id = $2
           FOR UPDATE"#,
    )
    .bind(&write.world_id)
    .bind(&write.external_equipment_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    if existing_equipment.is_some() {
        return Err(CompetitionTransactionError::Conflict);
    }

    let maximum_lot_number = sqlx::query_scalar::<_, Option<i32>>(
        "SELECT MAX(lot_number) FROM public.lots WHERE auction_id = $1",
    )
    .bind(auction_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    .unwrap_or(0);
    let lot_number =
        maximum_lot_number.checked_add(1).ok_or_else(|| failed("Admin 装备录入 lot 序号溢出"))?;
    if lot_number <= 0 {
        return Err(failed("Admin 装备录入 lot 序号无效"));
    }
    let next_state_version = current_state_version
        .checked_add(1)
        .ok_or_else(|| failed("Admin 装备录入拍卖状态版本溢出"))?;
    let next_preview_revision = current_preview_revision
        .checked_add(1)
        .ok_or_else(|| failed("Admin 装备录入预览版本溢出"))?;

    sqlx::query(
        r#"INSERT INTO public.equipment
            (id, world_id, external_equipment_id, name, category, attributes, collected_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7)"#,
    )
    .bind(write.candidate_equipment_id.as_uuid())
    .bind(&write.world_id)
    .bind(&write.external_equipment_id)
    .bind(&write.equipment_name)
    .bind(&write.equipment_category)
    .bind(&write.equipment_attributes)
    .bind(write.collected_at)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;

    sqlx::query(
        r#"INSERT INTO public.equipment_secrets
            (equipment_id, ciphertext, nonce, encryption_key_id, fingerprint, created_at, updated_at)
           VALUES ($1, $2, $3, $4, $5, $6, $6)"#,
    )
    .bind(write.candidate_equipment_id.as_uuid())
    .bind(write.key_envelope.ciphertext())
    .bind(write.key_envelope.nonce())
    .bind(write.key_envelope.encryption_key_id())
    .bind(write.key_envelope.fingerprint())
    .bind(write.collected_at)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;

    sqlx::query(
        r#"INSERT INTO public.auction_submissions
            (id, source_command_id, auction_id, mailbox_key, external_mail_id, seller,
             received_at, source_kind, source_reference, source_fingerprint)
           VALUES ($1, $2, $3, NULL, NULL, $4, $5, $6, $7, $8)"#,
    )
    .bind(write.candidate_submission_id)
    .bind(write.command_id.as_uuid())
    .bind(auction_id)
    .bind(&write.player_id)
    .bind(write.collected_at)
    .bind(write.source_kind.as_str())
    .bind(&write.source_reference)
    .bind(write.source_fingerprint.to_vec())
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;

    sqlx::query(
        r#"INSERT INTO public.lots
            (id, auction_id, equipment_id, lot_number, status, start_price,
             submission_id, state_version, created_at, updated_at)
           VALUES ($1, $2, $3, $4, 'pending', $5, $6, 1, $7, $7)"#,
    )
    .bind(write.candidate_lot_id)
    .bind(auction_id)
    .bind(write.candidate_equipment_id.as_uuid())
    .bind(lot_number)
    .bind(write.start_price)
    .bind(write.candidate_submission_id)
    .bind(write.collected_at)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;

    let updated = sqlx::query(
        r#"UPDATE public.auctions
           SET preview_revision = $1, state_version = $2, updated_at = $3
           WHERE id = $4
             AND status = 'draft'
             AND preview_revision = $5
             AND state_version = $6
           RETURNING state_version"#,
    )
    .bind(next_preview_revision)
    .bind(next_state_version)
    .bind(write.collected_at)
    .bind(auction_id)
    .bind(current_preview_revision)
    .bind(current_state_version)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    if updated.is_none() {
        return Err(CompetitionTransactionError::Conflict);
    }

    let result = AdminIntakeResult {
        auction_id,
        auction_sequence,
        auction_state_version: next_state_version,
        preview_revision: next_preview_revision,
        submission_id: write.candidate_submission_id,
        lot_id: write.candidate_lot_id,
        lot_number,
        lot_state_version: 1,
        equipment_id: write.candidate_equipment_id.as_uuid(),
        external_equipment_id: write.external_equipment_id,
        equipment_name: write.equipment_name,
        equipment_category: write.equipment_category,
        equipment_level: write.equipment_level,
        start_price: write.start_price,
        seller_player_id: write.player_id,
    };
    result.validate().map_err(|_| failed("Admin 装备录入返回结果无效"))?;
    sqlx::query(
        r#"UPDATE public.auction_commands
           SET result_code = 'accepted', result_payload = $1
           WHERE command_id = $2 AND result_code = 'pending'"#,
    )
    .bind(serde_json::to_value(&result).map_err(|_| failed("Admin 装备录入结果编码失败"))?)
    .bind(write.command_id.as_uuid())
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    Ok(result)
}

async fn append_admin_intake_record_inner(
    transaction: &mut PostgresCompetitionTransaction,
    record: AdminIntakeRecord,
) -> Result<(), CompetitionTransactionError> {
    record.result.validate().map_err(|_| failed("Admin 装备录入审计结果无效"))?;
    let transaction = transaction.transaction_mut()?;
    let metadata = json!({
        "command_id": record.command_id.as_uuid(),
        "auction_id": record.result.auction_id,
        "world_id": record.world_id,
        "lot_id": record.result.lot_id,
        "lot_number": record.result.lot_number,
        "preview_revision": record.result.preview_revision,
        "state_version": record.result.auction_state_version,
    });
    sqlx::query(
        r#"INSERT INTO public.audit_records
            (id, actor_type, actor_id, action, entity_type, entity_id, result,
             request_id, source, metadata)
           VALUES ($1, 'admin', $2, 'auction.admin_intake_item', 'lot', $3,
                   'success', $4, 'admin', $5)"#,
    )
    .bind(Uuid::new_v4())
    .bind(&record.actor_id)
    .bind(record.result.lot_id.to_string())
    .bind(record.request_id.as_deref())
    .bind(metadata)
    .execute(&mut **transaction)
    .await
    .record_audit_failure()
    .map_err(map_sql_error)?;

    let stream_key = format!("auction:{}", record.result.auction_id);
    sqlx::query(
        "INSERT INTO public.realtime_streams (stream_key) VALUES ($1)
         ON CONFLICT (stream_key) DO NOTHING",
    )
    .bind(&stream_key)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let sequence = sqlx::query_scalar::<_, i64>(
        "SELECT next_sequence FROM public.realtime_streams WHERE stream_key = $1 FOR UPDATE",
    )
    .bind(&stream_key)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    sqlx::query(
        "UPDATE public.realtime_streams
         SET next_sequence = next_sequence + 1, updated_at = CURRENT_TIMESTAMP
         WHERE stream_key = $1",
    )
    .bind(&stream_key)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let payload = json!({
        "world": record.world_id,
        "auction_id": record.result.auction_id,
        "preview_revision": record.result.preview_revision,
    });
    sqlx::query(
        r#"INSERT INTO public.realtime_events
            (event_id, stream_key, stream_sequence, kind, schema_version,
             public_payload, state_version, origin_command_id)
           VALUES ($1, $2, $3, 'preview_catalog_changed', 1, $4, $5, $6)"#,
    )
    .bind(Uuid::new_v4())
    .bind(&stream_key)
    .bind(sequence)
    .bind(payload)
    .bind(record.result.auction_state_version)
    .bind(record.command_id.as_uuid())
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}

fn failed(message: &str) -> CompetitionTransactionError {
    CompetitionTransactionError::Failed(message.to_owned())
}
