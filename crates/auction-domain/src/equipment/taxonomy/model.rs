//! 词典的纯值类型、来源元数据与不携带原始输入的安全错误。

use thiserror::Error;

/// 稳定的七个装备大类；规范字符串保持历史持久化值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EquipmentCategory {
    /// 单手武器。
    OneHanded,
    /// 双手武器。
    TwoHanded,
    /// 法杖。
    Staff,
    /// 盾牌。
    Shield,
    /// 布甲。
    Cloth,
    /// 轻甲。
    Light,
    /// 重甲。
    Heavy,
}

impl EquipmentCategory {
    /// 七个大类的稳定顺序；公开分类统计与前端分类表沿用该顺序。
    pub const ALL: [Self; 7] = [
        Self::OneHanded,
        Self::TwoHanded,
        Self::Staff,
        Self::Shield,
        Self::Cloth,
        Self::Light,
        Self::Heavy,
    ];

    /// 返回既有 canonical JSON、快照与规则使用的规范名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OneHanded => "One-Handed",
            Self::TwoHanded => "Two-Handed",
            Self::Staff => "Staff",
            Self::Shield => "Shield",
            Self::Cloth => "Cloth",
            Self::Light => "Light",
            Self::Heavy => "Heavy",
        }
    }

    /// 只按持久化的规范名称精确解析；未知名称由调用方决定回退或失败关闭。
    pub fn from_name(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|category| category.as_str() == value)
    }

    /// 玩家可见 lot 代码的三字母前缀。论坛回帖按该代码定位竞价目标，网站、邮件与
    /// 竞价日志必须显示同一代码，因此前缀属于持久的玩家身份，不能随展示调整。
    pub const fn lot_code_prefix(self) -> &'static str {
        match self {
            Self::OneHanded => "One",
            Self::TwoHanded => "Two",
            Self::Staff => "Sta",
            Self::Shield => "Shd",
            Self::Cloth => "Clo",
            Self::Light => "Lig",
            Self::Heavy => "Hea",
        }
    }

    /// 公开 lot 列表筛选与分类统计使用的稳定 JSON 键。
    pub const fn public_key(self) -> &'static str {
        match self {
            Self::OneHanded => "one",
            Self::TwoHanded => "two",
            Self::Staff => "sta",
            Self::Shield => "shd",
            Self::Cloth => "clo",
            Self::Light => "lig",
            Self::Heavy => "hea",
        }
    }

    /// 按公开分类键精确解析；`all` 与未知键都不是大类。
    pub fn from_public_key(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|category| category.public_key() == value)
    }

    /// 前缀加至少两位补零的类别内序号，例如 `Clo01`、`Hea120`。
    pub fn lot_code(self, lot_number: i64) -> String {
        format!("{}{lot_number:02}", self.lot_code_prefix())
    }

    /// 解析真实页面 header 的大类标签，保留既有分隔符和大小写兼容规则。
    pub fn from_header_label(value: &str) -> Result<Self, TaxonomyError> {
        super::category_from_header(value)
    }
}

/// 布甲、轻甲和重甲的语义部位；名称中的部位词另行保留。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EquipmentSlot {
    /// 头部。
    Head,
    /// 躯干。
    Body,
    /// 双手。
    Hands,
    /// 双腿。
    Legs,
    /// 双脚。
    Feet,
}

impl EquipmentSlot {
    /// 返回与具体材料名称无关的稳定部位标识。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Head => "head",
            Self::Body => "body",
            Self::Hands => "hands",
            Self::Legs => "legs",
            Self::Feet => "feet",
        }
    }
}

/// 词条是现行确证、历史确证，还是仅受限语法可读。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaxonomyStatus {
    /// 当前 Wiki 的确证词条或关系。
    Current,
    /// Obsolete 页面明确记载的历史词条或关系。
    Legacy,
    /// 材质与公共部位的有限兼容语法，并非逐种类确证清单。
    BoundedGrammar,
}

/// 每个词条携带可复核的 Wiki 来源和该页固定修订号。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaxonomyMetadata {
    /// 词条的现行、历史或有限语法状态。
    pub status: TaxonomyStatus,
    /// 来源页面的公开 URL。
    pub source: &'static str,
    /// MediaWiki 的 oldid，使用十进制字符串。
    pub revision: &'static str,
}

/// 大类展示项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CategoryEntry {
    /// 历史规范名称。
    pub name: &'static str,
    /// 类型化的大类。
    pub category: EquipmentCategory,
    /// 确证来源。
    pub metadata: TaxonomyMetadata,
}

/// 品质词条；解析认识历史品质不改变收件准入。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QualityEntry {
    /// 规范品质名称。
    pub name: &'static str,
    /// 现行或历史来源。
    pub metadata: TaxonomyMetadata,
}

/// 护甲材质词条。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaterialEntry {
    /// 规范材质名称。
    pub name: &'static str,
    /// 材质所属大类。
    pub category: EquipmentCategory,
    /// 仅原十二个现行材质允许用作新多选规则的整族匹配。
    pub selectable_family: bool,
    /// 现行或历史来源。
    pub metadata: TaxonomyMetadata,
}

/// 已确证的完整基础名；护甲由种类和部位组成，受限语法不会被展开到此清单。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KindEntry {
    /// 完整规范基础名；例如 Phase Robe，武器和盾牌不带独立部位。
    pub name: &'static str,
    /// 所属大类。
    pub category: EquipmentCategory,
    /// 护甲材质；武器和盾牌为空。
    pub material: Option<&'static str>,
    /// 护甲部位；武器和盾牌为空。
    pub slot: Option<EquipmentSlot>,
    /// 完整种类的确证来源。
    pub metadata: TaxonomyMetadata,
}

/// 一次解析识别的完整基础名，可以来自确证清单或有限兼容语法。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedKind {
    /// 完整规范基础名；供名称匹配及历史 canonical JSON 边界使用。
    pub name: String,
    /// 所属大类。
    pub category: EquipmentCategory,
    /// 护甲材质。
    pub material: Option<&'static str>,
    /// 护甲部位。
    pub slot: Option<EquipmentSlot>,
    /// 保留确证或有限语法来源。
    pub metadata: TaxonomyMetadata,
}

impl From<&KindEntry> for ResolvedKind {
    fn from(value: &KindEntry) -> Self {
        Self {
            name: value.name.to_owned(),
            category: value.category,
            material: value.material,
            slot: value.slot,
            metadata: value.metadata,
        }
    }
}

/// 从完整名称拆出的稳定身份字段；原始名称由调用方保留。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EquipmentIdentity {
    /// 规范品质名称。
    pub quality: String,
    /// 规范空白后的原前缀文本。
    pub prefix: String,
    /// 规范空白后的原后缀文本，保留可选的 the。
    pub suffix: String,
    /// 词典种类所属类型，仅供详情 header 与 JSON 交叉校验；名称中没有类型词条。
    pub category: EquipmentCategory,
    /// 种类、材质、部位与来源。
    pub kind: ResolvedKind,
}

/// 纯名称解析的安全闭集错误，不保存或回显外部原文。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum TaxonomyError {
    /// 名称结构、长度或字符非法。
    #[error("装备名称结构非法")]
    InvalidName,
    /// 品质词未被当前或历史词典识别。
    #[error("装备品质未知")]
    UnknownQuality,
    /// 完整种类既不在词典，也不满足受限语法。
    #[error("装备种类未知")]
    UnknownKind,
    /// 页面大类标签未知。
    #[error("装备大类未知")]
    UnknownCategory,
}
