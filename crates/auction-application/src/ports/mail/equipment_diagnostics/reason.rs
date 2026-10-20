//! 装备来源解析的闭集诊断码；不携带原名称、页面、URL 或装备 key。

/// 页面提取与装备语义识别的失败原因；独立于收件策略拒绝。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EquipmentSourceDetailCode {
    InvalidName,
    UnknownQuality,
    UnknownKind,
    UnknownCategory,
    CategoryMismatch,
    InvalidLevel,
    PageStructure,
    InvalidAttributes,
    CanonicalMismatch,
}

impl EquipmentSourceDetailCode {
    /// 返回允许进入日志及诊断投影的稳定码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidName => "equipment_name_invalid",
            Self::UnknownQuality => "equipment_quality_unknown",
            Self::UnknownKind => "equipment_kind_unknown",
            Self::UnknownCategory => "equipment_category_unknown",
            Self::CategoryMismatch => "equipment_category_mismatch",
            Self::InvalidLevel => "equipment_level_invalid",
            Self::PageStructure => "equipment_page_structure_invalid",
            Self::InvalidAttributes => "equipment_attributes_invalid",
            Self::CanonicalMismatch => "equipment_canonical_mismatch",
        }
    }

    /// 校验已持久化的诊断码；任意外部文本不能成为来源失败原因。
    pub fn from_code(value: &str) -> Option<Self> {
        [
            Self::InvalidName,
            Self::UnknownQuality,
            Self::UnknownKind,
            Self::UnknownCategory,
            Self::CategoryMismatch,
            Self::InvalidLevel,
            Self::PageStructure,
            Self::InvalidAttributes,
            Self::CanonicalMismatch,
        ]
        .into_iter()
        .find(|code| code.as_str() == value)
    }
}
