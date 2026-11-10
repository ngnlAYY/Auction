//! Admin 装备录入事实和 pending lot 的原子写入。

use auction_application::admin::{AdminIntakeResult, AdminIntakeWrite};
use auction_application::core::{CommandLedgerError, CompetitionTransactionError};
use chrono::{DateTime, Utc};
use serde_json::json;
use uuid::Uuid;

use crate::database::auction::competition::{
    PostgresCompetitionTransaction, is_unique_error, map_sql_error,
};
use crate::database::auction::intake_target;

use super::failed;

pub(super) async fn add_admin_intake_inner(
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
    .bind(write.request_hash.as_bytes().as_slice())
    .bind(json!({}))
    .execute(&mut **transaction)
    .await
    .map_err(map_command_insert_error)?;

    let world_enabled =
        sqlx::query_scalar::<_, bool>("SELECT enabled FROM public.worlds WHERE id = $1 FOR UPDATE")
            .bind(&write.world_id)
            .fetch_optional(&mut **transaction)
            .await
            .map_err(map_sql_error)?
            .ok_or(CompetitionTransactionError::AdminIntakeUnavailable)?;
    if !world_enabled {
        return Err(CompetitionTransactionError::AdminIntakeUnavailable);
    }

    let target = intake_target::lock(transaction, &write.world_id)
        .await
        .map_err(map_sql_error)?
        .ok_or(CompetitionTransactionError::AdminIntakeUnavailable)?;
    if !target.is_consistent(&write.world_id) {
        return Err(CompetitionTransactionError::InvalidState);
    }
    let auction_id = target.auction_id;
    // 目标可能在外部采集或等待 world 锁期间切换；只使用取得当前目标锁后的数据库实时时间。
    let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
    // 与邮件收件同一规则：新赛季早鸟草稿在赛季开始前不接收任何装备。
    if target.intake_opens_at.is_some_and(|opens_at| now < opens_at) {
        return Err(CompetitionTransactionError::IntakeClosed);
    }

    // 用户名按 Users 唯一身份口径匹配并锁定真实行；展示原文与命令摘要不做大小写改写。
    let player_status = sqlx::query_scalar::<_, String>(
        "SELECT status FROM public.users WHERE lower(username) = lower($1) FOR SHARE",
    )
    .bind(&write.player_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    if player_status.as_deref() == Some("blacklisted") {
        return Err(CompetitionTransactionError::AdminIntakeSellerBlacklisted);
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
        return Err(CompetitionTransactionError::AdminIntakeEquipmentConflict);
    }

    // `lot_number` 保留场次内不可变全局顺序；用户可见编码在分类内独立递增，两者一次扫描取得。
    let (maximum_lot_number, maximum_category_number) = sqlx::query_as::<_, (i32, i32)>(
        "SELECT COALESCE(MAX(lot_number), 0),
                COALESCE(MAX(category_number) FILTER (WHERE category_key = $2), 0)
         FROM public.lots
         WHERE auction_id = $1",
    )
    .bind(auction_id)
    .bind(&write.equipment_category)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let lot_number =
        maximum_lot_number.checked_add(1).ok_or_else(|| failed("Admin 装备录入 lot 序号溢出"))?;
    if lot_number <= 0 {
        return Err(failed("Admin 装备录入 lot 序号无效"));
    }
    let category_number = maximum_category_number
        .checked_add(1)
        .ok_or_else(|| failed("Admin 装备录入分类编号溢出"))?;
    if category_number <= 0 {
        return Err(failed("Admin 装备录入分类编号无效"));
    }
    let next_versions =
        target.next_versions().ok_or_else(|| failed("Admin 装备录入拍卖版本溢出"))?;
    let (next_preview_revision, next_state_version) = next_versions;

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
    .map_err(map_equipment_insert_error)?;

    intake_target::insert_equipment_secret(
        transaction,
        write.candidate_equipment_id.as_uuid(),
        &write.key_envelope,
        now,
    )
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
    .bind(now)
    // `source_kind` 属于投稿 owner 闭集；URL/JSON 是 Admin 请求适配器种类，只进入来源引用与摘要。
    .bind("admin")
    .bind(&write.source_reference)
    .bind(write.source_fingerprint.as_slice())
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;

    sqlx::query(
        r#"INSERT INTO public.lots
            (id, auction_id, equipment_id, lot_number, category_key, category_number,
             status, start_price, submission_id, state_version, created_at, updated_at)
           VALUES ($1, $2, $3, $4, $5, $6, 'pending', $7, $8, 1, $9, $9)"#,
    )
    .bind(write.candidate_lot_id)
    .bind(auction_id)
    .bind(write.candidate_equipment_id.as_uuid())
    .bind(lot_number)
    .bind(&write.equipment_category)
    .bind(category_number)
    .bind(write.start_price)
    .bind(write.candidate_submission_id)
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;

    if !intake_target::advance(transaction, &target, next_versions, now)
        .await
        .map_err(map_sql_error)?
    {
        return Err(CompetitionTransactionError::InvalidState);
    }
    if target.is_preview() {
        let materialized = sqlx::query(
            "INSERT INTO public.auction_preview_items
                (auction_id, equipment_id, display_order, start_price,
                 source_revision, collected_at)
             SELECT lot.auction_id, lot.equipment_id, lot.lot_number, lot.start_price,
                    $1, equipment.collected_at
             FROM public.lots AS lot
             INNER JOIN public.equipment AS equipment ON equipment.id = lot.equipment_id
             WHERE lot.id = $2 AND lot.auction_id = $3 AND lot.status = 'pending'",
        )
        .bind(next_preview_revision)
        .bind(write.candidate_lot_id)
        .bind(auction_id)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
        if materialized.rows_affected() != 1 {
            return Err(failed("Admin Preview 装备物化失败"));
        }
    }

    let result = AdminIntakeResult {
        auction_id,
        auction_sequence: target.auction_sequence,
        auction_state_version: next_state_version,
        preview_revision: next_preview_revision,
        submission_id: write.candidate_submission_id,
        lot_id: write.candidate_lot_id,
        lot_number: category_number,
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

fn map_command_insert_error(error: sqlx::Error) -> CompetitionTransactionError {
    if is_unique_error(&error) {
        CompetitionTransactionError::CommandLedger(CommandLedgerError::Conflict)
    } else {
        map_sql_error(error)
    }
}

fn map_equipment_insert_error(error: sqlx::Error) -> CompetitionTransactionError {
    if is_unique_error(&error) {
        CompetitionTransactionError::AdminIntakeEquipmentConflict
    } else {
        map_sql_error(error)
    }
}
