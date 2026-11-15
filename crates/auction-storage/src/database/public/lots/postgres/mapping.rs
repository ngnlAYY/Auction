//! PostgreSQL lot 查询结果到公开应用投影的映射。

use std::collections::BTreeMap;

use auction_application::auction::{
    PUBLIC_LOT_CATEGORIES, PublicAuctionProjection, PublicLiveLotSummary, PublicPreviewLotDetail,
    PublicPreviewLotSummary,
};
use auction_application::core::ApplicationError;
use auction_application::forum::EquipmentColorRule;
use auction_application::public::lot_code;
use auction_domain::EquipmentId;
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

use crate::database::public::lots::mapping::map_counts;
use crate::database::public::lots::mapping::map_equipment;
use crate::database::public::lots::mapping::map_live_fields;
use crate::database::public::lots::mapping::map_preview_fields;
use crate::database::public::lots::mapping::map_projection;
use crate::database::storage_error_from;

pub(super) fn map_postgres_projection(
    row: sqlx::postgres::PgRow,
) -> Result<PublicAuctionProjection, ApplicationError> {
    let status: String = row
        .try_get("status")
        .map_err(|error| storage_error_from(error, "读取公开 lot 拍卖状态失败"))?;
    map_projection(
        row.try_get("id")
            .map_err(|error| storage_error_from(error, "读取公开 lot 拍卖标识失败"))?,
        row.try_get("world_id")
            .map_err(|error| storage_error_from(error, "读取公开 lot 世界标识失败"))?,
        row.try_get("sequence")
            .map_err(|error| storage_error_from(error, "读取公开 lot 拍卖序号失败"))?,
        status,
        row.try_get("scheduled_open_at")
            .map_err(|error| storage_error_from(error, "读取公开 lot 开始时间失败"))?,
        row.try_get("scheduled_ends_at")
            .map_err(|error| storage_error_from(error, "读取公开 lot 结束时间失败"))?,
        row.try_get("preview_revision")
            .map_err(|error| storage_error_from(error, "读取公开 lot 预览版本失败"))?,
        row.try_get("presentation_mode")
            .map_err(|error| storage_error_from(error, "读取公开 lot 展示模式失败"))?,
        row.try_get("bidding_open")
            .map_err(|error| storage_error_from(error, "读取公开 lot 竞价状态失败"))?,
    )
}

pub(super) fn map_postgres_preview(
    row: sqlx::postgres::PgRow,
    world_id: &str,
    colors: &[EquipmentColorRule],
) -> Result<PublicPreviewLotSummary, ApplicationError> {
    let attributes: sqlx::types::Json<Value> = row
        .try_get("equipment_attributes")
        .map_err(|error| storage_error_from(error, "读取 PostgreSQL 预览装备属性失败"))?;
    map_preview_fields(
        world_id,
        row.try_get("id")
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL 预览 lot 标识失败"))?,
        EquipmentId::from_uuid(
            row.try_get("equipment_id").map_err(|error| {
                storage_error_from(error, "读取 PostgreSQL 预览装备 owner 失败")
            })?,
        ),
        row.try_get("auction_id")
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL 预览拍卖标识失败"))?,
        i64::from(
            row.try_get::<i32, _>("lot_number")
                .map_err(|error| storage_error_from(error, "读取 PostgreSQL 预览 lot 序号失败"))?,
        ),
        row.try_get("status")
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL 预览 lot 状态失败"))?,
        row.try_get("start_price")
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL 预览起始价失败"))?,
        row.try_get("seller_name")
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL 预览卖家失败"))?,
        row.try_get("external_equipment_id")
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL 预览装备编号失败"))?,
        row.try_get("equipment_name")
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL 预览装备名称失败"))?,
        row.try_get("equipment_category")
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL 预览装备分类失败"))?,
        attributes.0,
        colors,
    )
}

pub(super) fn map_postgres_preview_detail(
    row: sqlx::postgres::PgRow,
    world_id: &str,
    colors: &[EquipmentColorRule],
) -> Result<PublicPreviewLotDetail, ApplicationError> {
    let attributes: sqlx::types::Json<Value> = row
        .try_get("equipment_attributes")
        .map_err(|error| storage_error_from(error, "读取 PostgreSQL 预览详情装备属性失败"))?;
    let id: Uuid = row
        .try_get("id")
        .map_err(|error| storage_error_from(error, "读取 PostgreSQL 预览详情 lot 标识失败"))?;
    let auction_id: Uuid = row
        .try_get("auction_id")
        .map_err(|error| storage_error_from(error, "读取 PostgreSQL 预览详情拍卖标识失败"))?;
    let lot_number = i64::from(
        row.try_get::<i32, _>("lot_number")
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL 预览详情 lot 序号失败"))?,
    );
    let status: String = row
        .try_get("status")
        .map_err(|error| storage_error_from(error, "读取 PostgreSQL 预览详情 lot 状态失败"))?;
    let start_price: i64 = row
        .try_get("start_price")
        .map_err(|error| storage_error_from(error, "读取 PostgreSQL 预览详情起始价失败"))?;
    let seller: String = row
        .try_get("seller_name")
        .map_err(|error| storage_error_from(error, "读取 PostgreSQL 预览详情卖家失败"))?;
    let external_id: String = row
        .try_get("external_equipment_id")
        .map_err(|error| storage_error_from(error, "读取 PostgreSQL 预览详情装备编号失败"))?;
    let name: String = row
        .try_get("equipment_name")
        .map_err(|error| storage_error_from(error, "读取 PostgreSQL 预览详情装备名称失败"))?;
    let category: String = row
        .try_get("equipment_category")
        .map_err(|error| storage_error_from(error, "读取 PostgreSQL 预览详情装备分类失败"))?;
    let equipment =
        map_equipment(world_id, &external_id, &name, &category, attributes.0, false, colors)?;
    let primary_attribute_initials = equipment.primary_attribute_initials();
    let summary = PublicPreviewLotSummary {
        id,
        equipment_id: EquipmentId::from_uuid(row.try_get("equipment_id").map_err(|error| {
            storage_error_from(error, "读取 PostgreSQL 预览详情装备 owner 失败")
        })?),
        auction_id,
        code: lot_code(&category, lot_number),
        status,
        seller_name: seller,
        start_price,
        equipment_name: name,
        equipment_category: category,
        equipment_group: equipment.equipment_group,
        equipment_level: equipment.level,
        primary_attribute_initials,
        avg_roll: equipment.avg_roll,
        game_equipment_url: equipment.game_equipment_url.clone(),
    };
    Ok(PublicPreviewLotDetail { summary, equipment })
}

pub(super) fn map_postgres_live(
    row: sqlx::postgres::PgRow,
    world_id: &str,
    colors: &[EquipmentColorRule],
) -> Result<PublicLiveLotSummary, ApplicationError> {
    let attributes: sqlx::types::Json<Value> = row
        .try_get("equipment_attributes")
        .map_err(|error| storage_error_from(error, "读取 PostgreSQL 正式装备属性失败"))?;
    map_live_fields(
        world_id,
        row.try_get("id")
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL 正式 lot 标识失败"))?,
        EquipmentId::from_uuid(
            row.try_get("equipment_id").map_err(|error| {
                storage_error_from(error, "读取 PostgreSQL 正式装备 owner 失败")
            })?,
        ),
        row.try_get("auction_id")
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL 正式拍卖标识失败"))?,
        i64::from(
            row.try_get::<i32, _>("lot_number")
                .map_err(|error| storage_error_from(error, "读取 PostgreSQL 正式 lot 序号失败"))?,
        ),
        row.try_get("status")
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL 正式 lot 状态失败"))?,
        row.try_get("start_price")
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL 正式起始价失败"))?,
        row.try_get("current_price")
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL 正式当前价失败"))?,
        row.try_get("has_leader")
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL 正式领先状态失败"))?,
        row.try_get("policy_bid_increment_ppm")
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL 正式竞价增幅失败"))?,
        row.try_get("policy_minimum_increment")
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL 正式最低增量失败"))?,
        row.try_get("leader_name")
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL 正式领先者失败"))?,
        row.try_get("seller_name")
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL 正式卖家失败"))?,
        row.try_get("effective_end_at")
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL 正式结束时间失败"))?,
        row.try_get("state_version")
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL 正式状态版本失败"))?,
        row.try_get("external_equipment_id")
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL 正式装备编号失败"))?,
        row.try_get("equipment_name")
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL 正式装备名称失败"))?,
        row.try_get("equipment_category")
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL 正式装备分类失败"))?,
        attributes.0,
        colors,
    )
}

pub(super) fn map_postgres_counts(
    row: &sqlx::postgres::PgRow,
) -> Result<BTreeMap<String, i64>, ApplicationError> {
    // 列名由公开键派生（`<key>_count`），按名读取而不按位置对齐，大类顺序变化时不会静默错位。
    let mut values = [0_i64; PUBLIC_LOT_CATEGORIES.len()];
    for (value, key) in values.iter_mut().zip(PUBLIC_LOT_CATEGORIES) {
        *value = row
            .try_get(format!("{key}_count").as_str())
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL lot 分类统计失败"))?;
    }
    map_counts(values)
}
