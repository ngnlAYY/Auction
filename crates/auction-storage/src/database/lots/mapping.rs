//! Public lot 的跨后端字段映射、装备安全投影和输入校验。
//!
//! 数据库原始行在这里转换为应用 DTO；未知状态、越界文本和损坏的装备属性必须失败关闭。

use super::*;

// 这里把两个后端的列名映射集中收口；参数分别对应公开契约字段，拆成多个临时结构反而会
// 让后端行映射跨越适配器边界。
#[allow(clippy::too_many_arguments)]
pub(super) fn map_projection(
    auction_id: Uuid,
    world_id: String,
    sequence: i64,
    auction_status: String,
    opens_at: DateTime<Utc>,
    scheduled_ends_at: Option<DateTime<Utc>>,
    preview_revision: i64,
    presentation_mode: String,
    bidding_open: bool,
) -> Result<PublicAuctionProjection, ApplicationError> {
    if !matches!(auction_status.as_str(), "preview" | "live" | "final" | "settling" | "settled") {
        return Err(storage_error("公开 lot 拍卖状态无效"));
    }
    Ok(PublicAuctionProjection {
        auction_id,
        world_id,
        sequence,
        projection_kind: if auction_status == "preview" { "preview" } else { "live" }.to_owned(),
        final_restriction: (auction_status == "final").then_some(PublicFinalRestriction {
            new_slow_proxy_allowed: false,
            existing_slow_proxy_converted: true,
        }),
        auction_status,
        presentation_mode,
        bidding_open,
        opens_at,
        scheduled_ends_at,
        preview_revision,
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn map_preview_fields(
    world_id: &str,
    id: Uuid,
    auction_id: Uuid,
    lot_number: i64,
    status: String,
    start_price: i64,
    seller_name: Option<String>,
    external_id: String,
    equipment_name: String,
    equipment_category: String,
    attributes: Value,
) -> Result<PublicPreviewLotSummary, ApplicationError> {
    let equipment =
        map_equipment(world_id, &external_id, &equipment_name, &equipment_category, attributes)?;
    Ok(PublicPreviewLotSummary {
        id,
        auction_id,
        code: public_forum_lot_code(&equipment_category, lot_number),
        status,
        seller_name: seller_name.unwrap_or_else(|| "—".to_owned()),
        start_price,
        equipment_name,
        equipment_category,
        equipment_level: equipment.level,
        avg_roll: equipment.avg_roll,
        game_equipment_url: equipment.game_equipment_url,
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn map_live_fields(
    world_id: &str,
    id: Uuid,
    auction_id: Uuid,
    lot_number: i64,
    status: String,
    start_price: i64,
    current_price: Option<i64>,
    has_leader: bool,
    proxy_last: bool,
    material: bool,
    policy_bid_increment_ppm: i64,
    policy_minimum_increment: i64,
    leader_name: Option<String>,
    seller_name: Option<String>,
    effective_end_at: Option<DateTime<Utc>>,
    state_version: i64,
    external_id: String,
    equipment_name: String,
    equipment_category: String,
    attributes: Value,
) -> Result<PublicLiveLotSummary, ApplicationError> {
    let start =
        BidAmount::new(start_price).map_err(|_| storage_error("公开 lot 起始价不满足金额网格"))?;
    let current_value = current_price.unwrap_or(start_price);
    let current = BidAmount::new(current_value)
        .map_err(|_| storage_error("公开 lot 当前价不满足金额网格"))?;
    if current < start {
        return Err(storage_error("公开 lot 当前价低于起始价"));
    }
    let rules = AuctionRules {
        bid_increment_ppm: policy_bid_increment_ppm,
        minimum_increment: policy_minimum_increment,
        ..AuctionRules::default()
    };
    rules.validate().map_err(|_| storage_error("公开 lot 拍卖策略无效"))?;
    let next_bid = rules
        .next_minimum_bid(Some(start), has_leader.then_some(current), material, proxy_last)
        .map_err(|_| storage_error("公开 lot 下一口金额超出可表示范围"))?
        .value();
    let equipment =
        map_equipment(world_id, &external_id, &equipment_name, &equipment_category, attributes)?;
    Ok(PublicLiveLotSummary {
        id,
        auction_id,
        lot_number,
        status,
        seller_name: seller_name.unwrap_or_else(|| "—".to_owned()),
        start_price,
        current_price: current.value(),
        next_bid,
        has_leader,
        leader_name,
        effective_end_at,
        state_version,
        equipment,
    })
}

pub(super) fn map_counts(values: [i64; 8]) -> Result<BTreeMap<String, i64>, ApplicationError> {
    if values.iter().any(|value| *value < 0) {
        return Err(storage_error("公开 lot 分类统计为负数"));
    }
    Ok(CATEGORY_KEYS.into_iter().zip(values).map(|(key, value)| (key.to_owned(), value)).collect())
}

pub(super) fn map_equipment(
    world_id: &str,
    external_id: &str,
    name: &str,
    category: &str,
    attributes: Value,
) -> Result<PublicEquipment, ApplicationError> {
    if !matches!(world_id, "main" | "isekai")
        || external_id.is_empty()
        || external_id.len() > 32
        || !external_id.bytes().all(|byte| byte.is_ascii_digit())
        || !public_text(name, 512)
        || !public_text(category, 128)
        || !attributes.is_object()
    {
        return Err(storage_error("公开装备字段无效"));
    }
    let stored: StoredEquipmentAttributes = serde_json::from_value(attributes.clone())
        .map_err(|_| storage_error("公开装备属性无效"))?;
    let info_summary =
        stored.type_name.filter(|value| !value.is_empty()).unwrap_or_else(|| category.to_owned());
    let info_summary =
        stored.level.map_or(info_summary.clone(), |level| format!("{info_summary} Lv{level}"));
    if !public_text(&info_summary, 512) || stored.level.is_some_and(|level| level < 0) {
        return Err(storage_error("公开装备摘要无效"));
    }
    let quality = stored.quality.as_deref().and_then(quality_range);
    let mut stats = BTreeMap::new();
    let mut rolls = Vec::new();
    for (stat_name, stat) in stored.stats {
        if !public_text(&stat_name, 128)
            || !public_text(&stat.value, 256)
            || stat.base.is_some_and(|base| !base.is_finite())
        {
            return Err(storage_error("公开装备 stat 无效"));
        }
        let roll =
            quality.and_then(|range| stat.base.map(|base| public_equipment_roll(base, range)));
        if let Some(roll) = roll {
            rolls.push(roll);
        }
        stats.insert(stat_name, PublicEquipmentStat { value: stat.value, base: stat.base, roll });
    }
    let avg_roll = (!rolls.is_empty()).then(|| {
        let total: f64 = rolls.iter().sum();
        (total / rolls.len() as f64 * 10.0).round() / 10.0
    });
    let path = if world_id == "isekai" {
        format!("/isekai/equip/{external_id}")
    } else {
        format!("/equip/{external_id}")
    };
    Ok(PublicEquipment {
        name: name.to_owned(),
        category: category.to_owned(),
        level: stored.level,
        info_summary,
        stats,
        avg_roll,
        game_equipment_url: format!("https://hentaiverse.org{path}"),
        attributes,
    })
}

pub(super) fn public_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

fn public_equipment_roll(base: f64, range: EquipmentQualityRange) -> f64 {
    ((base - range.minimum) / (range.maximum - range.minimum) * 100.0 * 10.0).round() / 10.0
}

#[derive(Debug, Clone, Copy)]
struct EquipmentQualityRange {
    minimum: f64,
    maximum: f64,
}

fn quality_range(value: &str) -> Option<EquipmentQualityRange> {
    let name = value.split_once(' ').map_or(value, |(name, _)| name);
    match name {
        "Peerless" | "Legendary" => Some(EquipmentQualityRange { minimum: 170.0, maximum: 200.0 }),
        "Magnificent" => Some(EquipmentQualityRange { minimum: 150.0, maximum: 180.0 }),
        "Exquisite" => Some(EquipmentQualityRange { minimum: 120.0, maximum: 160.0 }),
        "Superior" => Some(EquipmentQualityRange { minimum: 90.0, maximum: 130.0 }),
        "Average" => Some(EquipmentQualityRange { minimum: 60.0, maximum: 100.0 }),
        "Fair" => Some(EquipmentQualityRange { minimum: 30.0, maximum: 70.0 }),
        "Crude" => Some(EquipmentQualityRange { minimum: 0.0, maximum: 40.0 }),
        _ => None,
    }
}

#[derive(Debug, Deserialize)]
struct StoredEquipmentAttributes {
    #[serde(rename = "type")]
    type_name: Option<String>,
    quality: Option<String>,
    level: Option<i64>,
    #[serde(default)]
    stats: BTreeMap<String, StoredEquipmentStat>,
}

#[derive(Debug, Deserialize)]
struct StoredEquipmentStat {
    value: String,
    base: Option<f64>,
}

pub(super) fn public_forum_lot_code(category: &str, number: i64) -> String {
    let prefix = match category {
        "One-Handed" => "One",
        "Two-Handed" => "Two",
        "Staff" => "Sta",
        "Shield" => "Shd",
        "Cloth" => "Clo",
        "Light" => "Lig",
        "Heavy" => "Hea",
        _ => "Lot",
    };
    format!("{prefix}{number:02}")
}

#[cfg(feature = "sqlite")]
pub(super) fn uuid_from_blob(bytes: Vec<u8>, label: &str) -> Result<Uuid, ApplicationError> {
    Uuid::from_slice(&bytes).map_err(|_| storage_error(label))
}

#[cfg(feature = "sqlite")]
pub(super) fn parse_attributes(value: String) -> Result<Value, ApplicationError> {
    serde_json::from_str(&value).map_err(|_| storage_error("Lite 公开装备属性不是有效 JSON"))
}

#[cfg(feature = "sqlite")]
pub(super) fn parse_optional_timestamp(
    value: Option<String>,
    label: &str,
) -> Result<Option<DateTime<Utc>>, ApplicationError> {
    value.map(|value| parse_timestamp(value, label)).transpose()
}

#[cfg(feature = "sqlite")]
pub(super) fn parse_timestamp(
    value: String,
    label: &str,
) -> Result<DateTime<Utc>, ApplicationError> {
    if let Ok(value) = DateTime::parse_from_rfc3339(&value) {
        return Ok(value.with_timezone(&Utc));
    }
    for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(value) = NaiveDateTime::parse_from_str(&value, format) {
            return Ok(DateTime::<Utc>::from_naive_utc_and_offset(value, Utc));
        }
    }
    Err(storage_error(label))
}

pub(super) fn validate_lot_request(
    auction_id: Uuid,
    request: &PublicLotListRequest,
) -> Result<(), ApplicationError> {
    if auction_id.is_nil() {
        return Err(storage_error("公开 lot 拍卖标识无效"));
    }
    request.validate().map_err(storage_error)
}

pub(super) fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(RepositoryError::Failed(message.to_owned()))
}
