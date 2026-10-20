//! 装备名称语义的领域适配；词典、品质及大类识别统一由 domain taxonomy 拥有。

use auction_domain::equipment::taxonomy::{EquipmentIdentity, TaxonomyError, parse_equipment_name};

use super::EquipmentParseError;

pub(crate) fn parse_identity(name: &str) -> Result<EquipmentIdentity, EquipmentParseError> {
    parse_equipment_name(name).map_err(taxonomy_error)
}

pub(super) fn taxonomy_error(error: TaxonomyError) -> EquipmentParseError {
    match error {
        TaxonomyError::InvalidName => EquipmentParseError::InvalidName,
        TaxonomyError::UnknownQuality => EquipmentParseError::UnknownQuality,
        TaxonomyError::UnknownKind => EquipmentParseError::UnknownKind,
        TaxonomyError::UnknownCategory => EquipmentParseError::UnknownCategory,
    }
}
