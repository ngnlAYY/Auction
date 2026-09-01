//! SQLite lot 查询结果到公开应用投影的映射。

use super::super::*;
use sqlx::Row;

#[cfg(feature = "sqlite")]
pub(super) fn map_sqlite_projection(
    row: sqlx::sqlite::SqliteRow,
) -> Result<PublicAuctionProjection, ApplicationError> {
    let status: String =
        row.try_get("status").map_err(|_| storage_error("读取 Lite lot 拍卖状态失败"))?;
    map_projection(
        uuid_from_blob(
            row.try_get("id").map_err(|_| storage_error("读取 Lite lot 拍卖标识失败"))?,
            "Lite lot 拍卖标识",
        )?,
        row.try_get("world_id").map_err(|_| storage_error("读取 Lite lot 世界标识失败"))?,
        row.try_get("sequence").map_err(|_| storage_error("读取 Lite lot 拍卖序号失败"))?,
        status,
        parse_timestamp(
            row.try_get("scheduled_open_at")
                .map_err(|_| storage_error("读取 Lite lot 开始时间失败"))?,
            "Lite lot 开始时间",
        )?,
        parse_optional_timestamp(
            row.try_get("scheduled_ends_at")
                .map_err(|_| storage_error("读取 Lite lot 结束时间失败"))?,
            "Lite lot 结束时间",
        )?,
        row.try_get("preview_revision").map_err(|_| storage_error("读取 Lite lot 预览版本失败"))?,
        row.try_get("presentation_mode")
            .map_err(|_| storage_error("读取 Lite lot 展示模式失败"))?,
        row.try_get::<i64, _>("bidding_open")
            .map_err(|_| storage_error("读取 Lite lot 竞价状态失败"))?
            != 0,
    )
}

#[cfg(feature = "sqlite")]
pub(super) fn map_sqlite_preview(
    row: sqlx::sqlite::SqliteRow,
    world_id: &str,
) -> Result<PublicPreviewLotSummary, ApplicationError> {
    let attributes = parse_attributes(
        row.try_get("equipment_attributes")
            .map_err(|_| storage_error("读取 Lite 预览装备属性失败"))?,
    )?;
    map_preview_fields(
        world_id,
        uuid_from_blob(
            row.try_get("id").map_err(|_| storage_error("读取 Lite 预览 lot 标识失败"))?,
            "Lite 预览 lot 标识",
        )?,
        EquipmentId::from_uuid(uuid_from_blob(
            row.try_get("equipment_id")
                .map_err(|_| storage_error("读取 Lite 预览装备 owner 失败"))?,
            "Lite 预览装备 owner",
        )?),
        uuid_from_blob(
            row.try_get("auction_id").map_err(|_| storage_error("读取 Lite 预览拍卖标识失败"))?,
            "Lite 预览拍卖标识",
        )?,
        row.try_get("lot_number").map_err(|_| storage_error("读取 Lite 预览 lot 序号失败"))?,
        row.try_get("status").map_err(|_| storage_error("读取 Lite 预览 lot 状态失败"))?,
        row.try_get("start_price").map_err(|_| storage_error("读取 Lite 预览起始价失败"))?,
        row.try_get("seller_name").map_err(|_| storage_error("读取 Lite 预览卖家失败"))?,
        row.try_get("external_equipment_id")
            .map_err(|_| storage_error("读取 Lite 预览装备编号失败"))?,
        row.try_get("equipment_name").map_err(|_| storage_error("读取 Lite 预览装备名称失败"))?,
        row.try_get("equipment_category")
            .map_err(|_| storage_error("读取 Lite 预览装备分类失败"))?,
        attributes,
    )
}

#[cfg(feature = "sqlite")]
pub(super) fn map_sqlite_preview_detail(
    row: sqlx::sqlite::SqliteRow,
    world_id: &str,
) -> Result<PublicPreviewLotDetail, ApplicationError> {
    let attributes = parse_attributes(
        row.try_get("equipment_attributes")
            .map_err(|_| storage_error("读取 Lite 预览详情装备属性失败"))?,
    )?;
    let id = uuid_from_blob(
        row.try_get("id").map_err(|_| storage_error("读取 Lite 预览详情 lot 标识失败"))?,
        "Lite 预览详情 lot 标识",
    )?;
    let auction_id = uuid_from_blob(
        row.try_get("auction_id").map_err(|_| storage_error("读取 Lite 预览详情拍卖标识失败"))?,
        "Lite 预览详情拍卖标识",
    )?;
    let lot_number: i64 =
        row.try_get("lot_number").map_err(|_| storage_error("读取 Lite 预览详情 lot 序号失败"))?;
    let status: String =
        row.try_get("status").map_err(|_| storage_error("读取 Lite 预览详情 lot 状态失败"))?;
    let start_price: i64 =
        row.try_get("start_price").map_err(|_| storage_error("读取 Lite 预览详情起始价失败"))?;
    let seller: String =
        row.try_get("seller_name").map_err(|_| storage_error("读取 Lite 预览详情卖家失败"))?;
    let external_id: String = row
        .try_get("external_equipment_id")
        .map_err(|_| storage_error("读取 Lite 预览详情装备编号失败"))?;
    let name: String = row
        .try_get("equipment_name")
        .map_err(|_| storage_error("读取 Lite 预览详情装备名称失败"))?;
    let category: String = row
        .try_get("equipment_category")
        .map_err(|_| storage_error("读取 Lite 预览详情装备分类失败"))?;
    let equipment = map_equipment(world_id, &external_id, &name, &category, attributes, false)?;
    let summary = PublicPreviewLotSummary {
        id,
        equipment_id: EquipmentId::from_uuid(uuid_from_blob(
            row.try_get("equipment_id")
                .map_err(|_| storage_error("读取 Lite 预览详情装备 owner 失败"))?,
            "Lite 预览详情装备 owner",
        )?),
        auction_id,
        code: public_forum_lot_code(&category, lot_number),
        status,
        seller_name: seller,
        start_price,
        equipment_name: name,
        equipment_category: category,
        equipment_level: equipment.level,
        avg_roll: equipment.avg_roll,
        game_equipment_url: equipment.game_equipment_url.clone(),
    };
    Ok(PublicPreviewLotDetail { summary, equipment })
}

#[cfg(feature = "sqlite")]
pub(super) fn map_sqlite_live(
    row: sqlx::sqlite::SqliteRow,
    world_id: &str,
) -> Result<PublicLiveLotSummary, ApplicationError> {
    let attributes = parse_attributes(
        row.try_get("equipment_attributes")
            .map_err(|_| storage_error("读取 Lite 正式装备属性失败"))?,
    )?;
    map_live_fields(
        world_id,
        uuid_from_blob(
            row.try_get("id").map_err(|_| storage_error("读取 Lite 正式 lot 标识失败"))?,
            "Lite 正式 lot 标识",
        )?,
        EquipmentId::from_uuid(uuid_from_blob(
            row.try_get("equipment_id")
                .map_err(|_| storage_error("读取 Lite 正式装备 owner 失败"))?,
            "Lite 正式装备 owner",
        )?),
        uuid_from_blob(
            row.try_get("auction_id").map_err(|_| storage_error("读取 Lite 正式拍卖标识失败"))?,
            "Lite 正式拍卖标识",
        )?,
        row.try_get("lot_number").map_err(|_| storage_error("读取 Lite 正式 lot 序号失败"))?,
        row.try_get("status").map_err(|_| storage_error("读取 Lite 正式 lot 状态失败"))?,
        row.try_get("start_price").map_err(|_| storage_error("读取 Lite 正式起始价失败"))?,
        row.try_get("current_price").map_err(|_| storage_error("读取 Lite 正式当前价失败"))?,
        row.try_get::<i64, _>("has_leader")
            .map_err(|_| storage_error("读取 Lite 正式领先状态失败"))?
            != 0,
        row.try_get::<i64, _>("material")
            .map_err(|_| storage_error("读取 Lite 正式材质状态失败"))?
            != 0,
        row.try_get("policy_bid_increment_ppm")
            .map_err(|_| storage_error("读取 Lite 正式竞价增幅失败"))?,
        row.try_get("policy_minimum_increment")
            .map_err(|_| storage_error("读取 Lite 正式最低增量失败"))?,
        row.try_get("leader_name").map_err(|_| storage_error("读取 Lite 正式领先者失败"))?,
        row.try_get("seller_name").map_err(|_| storage_error("读取 Lite 正式卖家失败"))?,
        parse_optional_timestamp(
            row.try_get("effective_end_at")
                .map_err(|_| storage_error("读取 Lite 正式结束时间失败"))?,
            "Lite 正式结束时间",
        )?,
        row.try_get("state_version").map_err(|_| storage_error("读取 Lite 正式状态版本失败"))?,
        row.try_get("external_equipment_id")
            .map_err(|_| storage_error("读取 Lite 正式装备编号失败"))?,
        row.try_get("equipment_name").map_err(|_| storage_error("读取 Lite 正式装备名称失败"))?,
        row.try_get("equipment_category")
            .map_err(|_| storage_error("读取 Lite 正式装备分类失败"))?,
        attributes,
    )
}
