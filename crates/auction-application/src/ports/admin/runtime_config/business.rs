use serde::{Deserialize, Serialize};

/// 单个 MoogleMail world 的可编辑运行策略。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeWorldConfig {
    /// 是否由 Worker 处理该 world；旧版要求 `main`、`isekai` 至少启用一个。
    pub enabled: bool,
    /// 允许读取详情的发件人身份。
    pub sender_allowlist: Vec<String>,
    /// 明确关闭 allowlist 后才允许读取任意发件人的详情。
    pub sender_allowlist_disabled: bool,
}

impl Default for AdminRuntimeWorldConfig {
    fn default() -> Self {
        Self { enabled: true, sender_allowlist: Vec::new(), sender_allowlist_disabled: false }
    }
}

/// Admin 与 Worker 共享的 MoogleMail world 闭集。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeWorldsConfig {
    pub main: AdminRuntimeWorldConfig,
    pub isekai: AdminRuntimeWorldConfig,
}

/// MoogleMail 装备过滤的等级范围。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeEquipmentLevelConfig {
    pub min: i32,
    pub max: i32,
}

/// Admin 可编辑、Worker 启动时冻结的 MoogleMail 装备过滤策略。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeEquipmentFilterConfig {
    /// 是否启用前后缀、分类、类型和等级过滤；品质门槛始终启用。
    pub enabled: bool,
    /// 硬性允许品质；空数组回退默认 Legendary/Peerless。
    pub qualities: Vec<String>,
    pub prefixes: Vec<String>,
    pub suffixes: Vec<String>,
    pub categories: Vec<String>,
    pub equipment_types: Vec<String>,
    pub level: AdminRuntimeEquipmentLevelConfig,
}

impl Default for AdminRuntimeEquipmentFilterConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            qualities: crate::ports::mail::default_accepted_qualities(),
            prefixes: Vec::new(),
            suffixes: Vec::new(),
            categories: Vec::new(),
            equipment_types: Vec::new(),
            level: AdminRuntimeEquipmentLevelConfig::default(),
        }
    }
}

/// 单条论坛名称标色规则；类型、种类、品质各维度内多选为 OR，维度间为 AND。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeEquipmentColorRule {
    /// 兼容旧单值装备类型；不得与非空 `categories` 同时使用。
    pub category: String,
    /// 兼容旧单值装备种类；不得与非空 `equipment_types` 同时使用。
    pub equipment_type: String,
    /// 兼容旧单值品质；不得与非空 `qualities` 同时使用。
    pub quality: String,
    /// 装备类型多选；空数组沿用旧单值，二者均空表示所有类型。
    /// 空数组不编码，保持旧文档 fingerprint 与 durable 命令摘要。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
    /// 装备种类多选，支持十二种护甲材质族或精确完整种类；空数组沿用旧单值。
    /// 材质族须同时匹配实际装备大类和完整种类首词，二者均空表示所有种类。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub equipment_types: Vec<String>,
    /// 品质多选；空数组沿用旧单值，二者均空表示所有品质。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub qualities: Vec<String>,
    pub level: AdminRuntimeEquipmentLevelConfig,
    pub prefixes: Vec<String>,
    pub suffixes: Vec<String>,
    pub prefix_color: String,
    pub suffix_color: String,
    pub combined_color: String,
}

impl Default for AdminRuntimeEquipmentColorRule {
    fn default() -> Self {
        Self {
            category: String::new(),
            equipment_type: String::new(),
            quality: String::new(),
            categories: Vec::new(),
            equipment_types: Vec::new(),
            qualities: Vec::new(),
            level: AdminRuntimeEquipmentLevelConfig::default(),
            prefixes: Vec::new(),
            suffixes: Vec::new(),
            prefix_color: "#ff0000".to_owned(),
            suffix_color: "#00aa00".to_owned(),
            combined_color: "#aa00aa".to_owned(),
        }
    }
}

/// Admin 可编辑、Public 分组与 Worker 论坛渲染在启动时冻结的装备标色规则集合。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeEquipmentColorsConfig {
    pub rules: Vec<AdminRuntimeEquipmentColorRule>,
}

impl AdminRuntimeEquipmentColorsConfig {
    /// 转换为公开装备分组与论坛渲染共用的窄规则类型。
    pub fn to_rules(&self) -> Vec<crate::ports::forum::EquipmentColorRule> {
        self.rules
            .iter()
            .map(|rule| crate::ports::forum::EquipmentColorRule {
                category: rule.category.clone(),
                equipment_type: rule.equipment_type.clone(),
                quality: rule.quality.clone(),
                categories: rule.categories.clone(),
                equipment_types: rule.equipment_types.clone(),
                qualities: rule.qualities.clone(),
                level_min: rule.level.min,
                level_max: rule.level.max,
                prefixes: rule.prefixes.clone(),
                suffixes: rule.suffixes.clone(),
                prefix_color: rule.prefix_color.clone(),
                suffix_color: rule.suffix_color.clone(),
                combined_color: rule.combined_color.clone(),
            })
            .collect()
    }
}
