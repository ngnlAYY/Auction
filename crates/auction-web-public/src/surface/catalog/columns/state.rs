//! 目录列的固定身份、布局权重与浏览器偏好；不改变目录查询或竞价状态。

/// 位序属于 v1 本地偏好格式；改动已有位序时必须更换存储版本。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum CatalogColumn {
    Code = 0,
    Item = 1,
    Group = 2,
    AverageRoll = 3,
    Level = 4,
    Attributes = 5,
    Seller = 6,
    Price = 7,
    NextBid = 8,
    Leader = 9,
    Remaining = 10,
    Status = 11,
    Actions = 12,
}

impl CatalogColumn {
    const fn bit(self) -> u16 {
        1 << self as u8
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CatalogLayout {
    Live,
    Settled,
    Preview,
}

impl CatalogLayout {
    /// 表头顺序与原有列权重的唯一来源；TableColumns 只归一化当前可见项。
    pub(super) const fn columns(self) -> &'static [(CatalogColumn, u16)] {
        use CatalogColumn::*;
        match self {
            Self::Live => &[
                // 编号左侧留有“已标记”记号位，比纯编号多占一份。
                (Code, 6),
                // Item 拿到可容纳常见装备全名的宽度；开拍后新增的 Actions 列不能挤掉它。
                (Item, 26),
                (Group, 4),
                (AverageRoll, 9),
                (Level, 5),
                (Attributes, 4),
                (Seller, 7),
                (Price, 9),
                (NextBid, 8),
                (Leader, 8),
                (Remaining, 7),
                (Status, 7),
                (Actions, 9),
            ],
            Self::Settled => &[
                (Code, 7),
                (Item, 25),
                (Group, 5),
                (AverageRoll, 6),
                (Level, 5),
                (Attributes, 5),
                (Seller, 8),
                (Price, 10),
                (NextBid, 9),
                (Leader, 8),
                (Remaining, 6),
                (Status, 6),
                // 结算后操作列只剩查看入口，只需一个图标的宽度。
                (Actions, 6),
            ],
            Self::Preview => &[
                (Code, 6),
                (Item, 26),
                (Group, 5),
                (AverageRoll, 9),
                (Level, 5),
                (Attributes, 6),
                (Seller, 11),
                (Price, 12),
                (Leader, 6),
                (Remaining, 7),
                (Status, 7),
                (Actions, 6),
            ],
        }
    }

    const fn mask(self) -> u16 {
        match self {
            Self::Live => ALL_COLUMNS,
            Self::Settled => const { column_mask(CatalogLayout::Settled.columns()) },
            Self::Preview => const { column_mask(CatalogLayout::Preview.columns()) },
        }
    }
}

const fn column_mask(columns: &[(CatalogColumn, u16)]) -> u16 {
    let mut mask = 0;
    let mut index = 0;
    while index < columns.len() {
        mask |= columns[index].0.bit();
        index += 1;
    }
    mask
}

const ALL_COLUMNS: u16 = column_mask(CatalogLayout::Live.columns());
#[cfg(target_arch = "wasm32")]
const STORAGE_KEY: &str = "hv.public.catalog-columns.v1";

/// 小型 Copy 位集跨目录模式共享；隐藏不适用于当前模式的列时仍保留其偏好。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CatalogVisibility(u16);

impl Default for CatalogVisibility {
    fn default() -> Self {
        // 装备分组可从筛选器和列设置恢复；默认不占目录列宽，给价格/剩余时间/状态留出空间。
        Self(ALL_COLUMNS & !CatalogColumn::Group.bit())
    }
}

impl CatalogVisibility {
    pub(crate) fn load() -> Self {
        #[cfg(target_arch = "wasm32")]
        let stored = auction_web_shared::storage::get_item(STORAGE_KEY);
        #[cfg(not(target_arch = "wasm32"))]
        let stored = None::<String>;

        stored.as_deref().and_then(Self::from_stored).unwrap_or_default()
    }

    /// 严格接受本版本写出的非零十进制位集；空白、未知位和全隐藏均视为损坏。
    pub(super) fn from_stored(value: &str) -> Option<Self> {
        if value.is_empty()
            || value.len() > 4
            || value.starts_with('0')
            || !value.bytes().all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        let bits = value.parse::<u16>().ok()?;
        (bits & !ALL_COLUMNS == 0).then_some(Self(bits))
    }

    /// 模式切换不得出现空表；只在派生视图临时补上装备列，不覆盖已保存偏好。
    pub(crate) fn for_layout(self, layout: CatalogLayout) -> Self {
        if self.visible_count(layout) == 0 {
            Self(self.0 | CatalogColumn::Item.bit())
        } else {
            self
        }
    }

    pub(crate) fn contains(self, column: CatalogColumn) -> bool {
        self.0 & column.bit() != 0
    }

    pub(crate) fn widths(self, layout: CatalogLayout) -> Vec<u16> {
        let visible = self.for_layout(layout);
        let mut widths = Vec::with_capacity(visible.visible_count(layout) as usize);
        for &(column, width) in layout.columns() {
            if visible.contains(column) {
                widths.push(width);
            }
        }
        widths
    }

    pub(super) fn visible_count(self, layout: CatalogLayout) -> u32 {
        (self.0 & layout.mask()).count_ones()
    }

    /// 原生 disabled 之外再守住最后一列；用户明确修改时才把有效布局写回偏好。
    pub(super) fn with_column(
        self,
        layout: CatalogLayout,
        column: CatalogColumn,
        shown: bool,
    ) -> Self {
        let current = self.for_layout(layout);
        if layout.mask() & column.bit() == 0
            || current.contains(column) == shown
            || !shown && current.visible_count(layout) == 1
        {
            return self;
        }
        if shown { Self(current.0 | column.bit()) } else { Self(current.0 & !column.bit()) }
    }

    /// “显示全部列”只打开当前布局包含的字段；其他布局的偏好原样保留。
    pub(super) fn with_all_columns(self, layout: CatalogLayout) -> Self {
        Self(self.0 | layout.mask())
    }

    /// 存储受限或写入失败时仍保留本次 Signal 修改，不影响当前页面使用。
    pub(super) fn save(self) {
        #[cfg(target_arch = "wasm32")]
        let _ = auction_web_shared::storage::set_item(STORAGE_KEY, &self.0.to_string());
    }
}
