//! Admin 装备词典的只读投影；不进入 runtime 文档、命令摘要或装备持久化快照。

use super::*;

/// 服务端统一词典的推荐候选；`revision` 标识词典内容版本。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EquipmentTaxonomyDto {
    pub revision: String,
    pub categories: Vec<String>,
    pub qualities: Vec<EquipmentQualityOptionDto>,
    pub kinds: Vec<EquipmentKindOptionDto>,
    pub materials: Vec<EquipmentMaterialOptionDto>,
    /// 仅来自本次配置、经领域有限语法识别的具体种类；与确证目录分开，旧投影缺省为空。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resolved_kinds: Vec<EquipmentBoundedKindOptionDto>,
}

/// 当前或历史品质；可识别范围独立于自动收件品质策略。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EquipmentQualityOptionDto {
    pub name: String,
    pub legacy: bool,
}

/// 已明确的完整种类；有限兼容语法不展开为确证种类候选。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EquipmentKindOptionDto {
    pub name: String,
    pub category: String,
    pub material: Option<String>,
    pub slot: Option<String>,
    pub legacy: bool,
    pub prefixes: Vec<String>,
    pub suffixes: Vec<String>,
}

/// 当前配置中已识别的有限兼容种类，不将语法可能组合枚举为确证选项。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EquipmentBoundedKindOptionDto {
    /// 领域解析后的完整规范名称。
    pub name: String,
    /// 领域确定的大类。
    pub category: String,
    /// 领域确定的材质，引用单独的材质族。
    pub material: String,
    /// 语义部位标识，与确证种类沿用同一闭集。
    pub slot: String,
    /// 该具体种类的推荐前缀。
    pub prefixes: Vec<String>,
    /// 该具体种类的推荐后缀。
    pub suffixes: Vec<String>,
}

/// 多选规则支持的护甲材质族；历史单值仍按完整文本精确匹配。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EquipmentMaterialOptionDto {
    pub name: String,
    pub category: String,
    pub prefixes: Vec<String>,
    pub suffixes: Vec<String>,
}
