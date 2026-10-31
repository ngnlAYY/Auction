use super::*;

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub(super) async fn add_mail_auction_intake_inner(
    transaction: &mut PostgresCompetitionTransaction,
    write: MailAuctionIntakeWrite,
) -> Result<MailAuctionIntakeResult, CompetitionTransactionError> {
    if write.command_id.is_nil()
        || write.aggregate_id.is_nil()
        || write.items.is_empty()
        || write.items.len() > 1000
        || write.request_hash.as_bytes() == [0; 32]
    {
        return Err(failed("MoogleMail 拍卖入站写入计划无效"));
    }
    let transaction = transaction.transaction_mut()?;
    sqlx::query(
        r#"INSERT INTO public.auction_commands
            (command_id, command_kind, aggregate_id, request_hash, result_code, result_payload)
           VALUES ($1, 'mail_auction_intake', $2, $3, 'pending', '{}'::jsonb)"#,
    )
    .bind(write.command_id.as_uuid())
    .bind(write.aggregate_id)
    .bind(write.request_hash.as_bytes().as_slice())
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
    // 退回与录入共享 world 锁；已安排退回的邮件不能在重试中重新成为可拍卖资产。
    if crate::database::mail_delivery::has_return_intent(
        transaction,
        &write.world_id,
        &write.external_mail_id,
    )
    .await
    .map_err(map_sql_error)?
    {
        return Err(CompetitionTransactionError::Conflict);
    }

    let target = sqlx::query(
        r#"SELECT target.auction_id,
                  target.target_status,
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
    let target_status: String = target.try_get("target_status").map_err(map_sql_error)?;
    let auction_world_id: String = target.try_get("world_id").map_err(map_sql_error)?;
    let auction_sequence: i64 = target.try_get("sequence").map_err(map_sql_error)?;
    let auction_status: String = target.try_get("status").map_err(map_sql_error)?;
    let current_state_version: i64 = target.try_get("state_version").map_err(map_sql_error)?;
    let current_preview_revision: i64 =
        target.try_get("preview_revision").map_err(map_sql_error)?;
    if auction_world_id != write.world_id
        || auction_sequence <= 0
        || !matches!(auction_status.as_str(), "draft" | "preview")
        || target_status != auction_status
        || current_state_version <= 0
        || current_preview_revision < 0
        || write.now < write.received_at
    {
        return Err(CompetitionTransactionError::Conflict);
    }
    let preview_intake = auction_status == "preview";
    // 外部读取后可能已切换到新 Draft；聚合时间必须在 world/target/auction 锁取得后生成。
    let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut **transaction)
        .await
        .map_err(map_sql_error)?;

    // 只规范化身份比较，不改邮件发件人原文、来源事实或已接受命令的摘要。
    let player_status = sqlx::query_scalar::<_, String>(
        "SELECT status FROM public.users WHERE lower(username) = lower($1) FOR SHARE",
    )
    .bind(&write.seller)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    if player_status.as_deref() == Some("blacklisted") {
        return Err(CompetitionTransactionError::Conflict);
    }

    let existing_submission = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM public.auction_submissions
         WHERE mailbox_key = $1 AND external_mail_id = $2
         FOR UPDATE",
    )
    .bind(&write.mailbox_key)
    .bind(&write.external_mail_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    if existing_submission.is_some() {
        return Err(CompetitionTransactionError::Conflict);
    }

    let category_rows = sqlx::query(
        "SELECT category_key, MAX(category_number)::integer AS maximum_category_number,
                MAX(lot_number)::integer AS maximum_lot_number
         FROM public.lots
         WHERE auction_id = $1
         GROUP BY category_key",
    )
    .bind(auction_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    // 分类聚合已经扫描当前场次；同时归并全局序号，避免再往返执行一次 MAX 查询。
    let mut maximum_lot_number: Option<i32> = None;
    let mut category_numbers = BTreeMap::<String, i32>::new();
    for row in category_rows {
        let category: String = row.try_get("category_key").map_err(map_sql_error)?;
        let maximum: i32 = row.try_get("maximum_category_number").map_err(map_sql_error)?;
        let category_lot_maximum: i32 = row.try_get("maximum_lot_number").map_err(map_sql_error)?;
        maximum_lot_number = Some(
            maximum_lot_number
                .map_or(category_lot_maximum, |maximum| maximum.max(category_lot_maximum)),
        );
        if category.is_empty()
            || maximum <= 0
            || category_numbers.insert(category, maximum).is_some()
        {
            return Err(failed("MoogleMail 分类编号快照无效"));
        }
    }
    let maximum_lot_number = maximum_lot_number.unwrap_or(0);
    let next_state_version = current_state_version
        .checked_add(1)
        .ok_or_else(|| failed("MoogleMail 拍卖状态版本溢出"))?;
    let next_preview_revision = current_preview_revision
        .checked_add(1)
        .ok_or_else(|| failed("MoogleMail 拍卖预览版本溢出"))?;

    sqlx::query(
        r#"INSERT INTO public.auction_submissions
            (id, source_command_id, auction_id, mailbox_key, external_mail_id, seller,
             received_at, source_kind)
           VALUES ($1, $2, $3, $4, $5, $6, $7, 'mail')"#,
    )
    .bind(write.submission_id)
    .bind(write.command_id.as_uuid())
    .bind(auction_id)
    .bind(&write.mailbox_key)
    .bind(&write.external_mail_id)
    .bind(&write.seller)
    .bind(write.received_at)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;

    for (index, item) in write.items.iter().enumerate() {
        // 全局 lot 顺序继续供锁序、游标和历史事实使用；分类编号只负责对外编码。
        let lot_number = maximum_lot_number
            .checked_add(i32::try_from(index).map_err(|_| failed("MoogleMail lot 序号溢出"))?)
            .and_then(|value| value.checked_add(1))
            .ok_or_else(|| failed("MoogleMail lot 序号溢出"))?;
        let category_number = {
            let current = category_numbers.entry(item.category.clone()).or_insert(0);
            *current = current.checked_add(1).ok_or_else(|| failed("MoogleMail 分类编号溢出"))?;
            *current
        };
        if lot_number <= 0
            || category_number <= 0
            || item.equipment_id.is_nil()
            || item.lot_id.is_nil()
            || item.equipment_id.as_uuid() == item.lot_id
            || item.start_price <= 0
            || item.attributes.is_null()
            || item.key_envelope.validate().is_err()
        {
            return Err(failed("MoogleMail 装备写入计划无效"));
        }
        let existing_equipment = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM public.equipment
             WHERE world_id = $1 AND external_equipment_id = $2
             FOR UPDATE",
        )
        .bind(&write.world_id)
        .bind(&item.external_equipment_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
        if existing_equipment.is_some() {
            return Err(CompetitionTransactionError::Conflict);
        }

        sqlx::query(
            r#"INSERT INTO public.equipment
                (id, world_id, external_equipment_id, name, category, attributes, collected_at)
               VALUES ($1, $2, $3, $4, $5, $6, $7)"#,
        )
        .bind(item.equipment_id.as_uuid())
        .bind(&write.world_id)
        .bind(&item.external_equipment_id)
        .bind(&item.name)
        .bind(&item.category)
        .bind(&item.attributes)
        .bind(item.collected_at)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;

        sqlx::query(
            r#"INSERT INTO public.equipment_secrets
                (equipment_id, ciphertext, nonce, encryption_key_id, fingerprint, created_at, updated_at)
               VALUES ($1, $2, $3, $4, $5, $6, $6)"#,
        )
        .bind(item.equipment_id.as_uuid())
        .bind(item.key_envelope.ciphertext())
        .bind(item.key_envelope.nonce())
        .bind(item.key_envelope.encryption_key_id())
        .bind(item.key_envelope.fingerprint())
        .bind(now)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;

        sqlx::query(
            r#"INSERT INTO public.lots
                (id, auction_id, equipment_id, lot_number, category_key, category_number,
                 status, start_price, submission_id, state_version, created_at, updated_at)
               VALUES ($1, $2, $3, $4, $5, $6, 'pending', $7, $8, 1, $9, $9)"#,
        )
        .bind(item.lot_id)
        .bind(auction_id)
        .bind(item.equipment_id.as_uuid())
        .bind(lot_number)
        .bind(&item.category)
        .bind(category_number)
        .bind(item.start_price)
        .bind(write.submission_id)
        .bind(now)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
    }

    let updated = sqlx::query(
        r#"UPDATE public.auctions
           SET preview_revision = $1, state_version = $2,
               updated_at = GREATEST(created_at, updated_at, $3)
           WHERE id = $4
             AND status = $7
             AND preview_revision = $5
             AND state_version = $6
           RETURNING state_version"#,
    )
    .bind(next_preview_revision)
    .bind(next_state_version)
    .bind(now)
    .bind(auction_id)
    .bind(current_preview_revision)
    .bind(current_state_version)
    .bind(&auction_status)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    if updated.is_none() {
        return Err(CompetitionTransactionError::Conflict);
    }
    if preview_intake {
        let materialized = sqlx::query(
            "INSERT INTO public.auction_preview_items
                (auction_id, equipment_id, display_order, start_price,
                 source_revision, collected_at)
             SELECT lot.auction_id, lot.equipment_id, lot.lot_number, lot.start_price,
                    $1, equipment.collected_at
             FROM public.lots AS lot
             INNER JOIN public.equipment AS equipment ON equipment.id = lot.equipment_id
             WHERE lot.auction_id = $2 AND lot.submission_id = $3 AND lot.status = 'pending'
             ORDER BY lot.lot_number",
        )
        .bind(next_preview_revision)
        .bind(auction_id)
        .bind(write.submission_id)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
        if usize::try_from(materialized.rows_affected()).ok() != Some(write.items.len()) {
            return Err(failed("MoogleMail Preview 装备物化数量不一致"));
        }
    }

    let lot_ids = write.items.iter().map(|item| item.lot_id).collect::<Vec<_>>();
    let result = MailAuctionIntakeResult {
        auction_id,
        auction_sequence,
        auction_state_version: next_state_version,
        preview_revision: next_preview_revision,
        submission_id: write.submission_id,
        added_count: i32::try_from(lot_ids.len()).map_err(|_| failed("MoogleMail lot 数量溢出"))?,
        existing_count: 0,
        lot_ids,
    };
    result.validate().map_err(|_| failed("MoogleMail 拍卖入站结果无效"))?;
    sqlx::query(
        "UPDATE public.auction_commands
         SET result_code = 'accepted', result_payload = $1
         WHERE command_id = $2 AND result_code = 'pending'",
    )
    .bind(serde_json::to_value(&result).map_err(|_| failed("MoogleMail 结果编码失败"))?)
    .bind(write.command_id.as_uuid())
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    Ok(result)
}
