//! 组合 HTML 提取、领域名称识别和 header 状态，保持来源大类交叉校验。

use super::super::model::EquipmentFacts;
use super::{
    EquipmentParseError, header::parse_header, html::extract_equipment_page,
    identity::parse_identity,
};

pub(crate) fn parse_equipment_page(
    body: &[u8],
    external_equipment_id: &str,
) -> Result<EquipmentFacts, EquipmentParseError> {
    let fields = extract_equipment_page(body)?;
    let header = parse_header(&fields.header)?;
    let identity = parse_identity(&fields.name)?;
    if identity.category != header.category {
        return Err(EquipmentParseError::CategoryMismatch);
    }
    Ok(EquipmentFacts {
        external_equipment_id: external_equipment_id.to_owned(),
        name: fields.name,
        category: header.category.as_str().to_owned(),
        equipment_type: identity.equipment_type,
        quality: identity.quality,
        prefix: identity.prefix,
        suffix: identity.suffix,
        level: header.level,
        tradeable: header.trade_status.is_tradeable(),
        stats: fields.stats,
    })
}
