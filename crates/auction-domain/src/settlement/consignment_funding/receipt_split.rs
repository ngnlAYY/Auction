//! COD 毛实收在旧 opening 与本笔新发行之间的拆分。

use uuid::Uuid;

use super::errors::FundingRuleError;

/// 同一经济实收：毛额 G、已含于旧 opening 的 A、新发行 U；0 <= A <= G，U = G - A（D9 第 4.2 节）。
///
/// 字段私有且只存 G 与 A，U 由 `new_issue()` 推出：任何构造路径都满足 U = G - A，
/// 不存在 A + U 与 G 失配的值（不变量 6）。本类型不提供反序列化入口。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodReceiptSplit {
    gross: i64,
    opening_included: i64,
}

impl CodReceiptSplit {
    /// 构造拆分；毛额必须为正，A 不得为负或超过毛额。
    pub fn new(gross: i64, opening_included: i64) -> Result<Self, FundingRuleError> {
        // 不变量 6：越界直接拒绝。U 不由调用方给出，避免 A + U 与 G 失配。
        if gross <= 0 || opening_included < 0 {
            return Err(FundingRuleError::InvalidAmount);
        }
        if opening_included > gross {
            return Err(FundingRuleError::OpeningExceedsGross);
        }
        Ok(Self { gross, opening_included })
    }

    /// 毛实收 G。
    pub const fn gross(&self) -> i64 {
        self.gross
    }

    /// 已含于旧 opening 的份额 A。
    pub const fn opening_included(&self) -> i64 {
        self.opening_included
    }

    /// 本笔新发行 U = G - A。
    pub const fn new_issue(&self) -> i64 {
        self.gross - self.opening_included
    }
}

/// 某个 opening 发行 entry 内的一段不可变份额，区间为半开 `[start, end)`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpeningSlice {
    pub entry_id: Uuid,
    pub start: i64,
    pub end: i64,
}

impl OpeningSlice {
    /// 本段金额。
    pub const fn amount(&self) -> i64 {
        self.end - self.start
    }
}

/// 汇总各段金额得到 A；同一 entry 的区间必须互不相交且非空，每段只消费一次。
pub fn opening_included_total(slices: &[OpeningSlice]) -> Result<i64, FundingRuleError> {
    let mut total: i64 = 0;
    for (index, slice) in slices.iter().enumerate() {
        if slice.start < 0 || slice.end <= slice.start {
            return Err(FundingRuleError::InvalidAmount);
        }
        // 数据库同样以区间不相交约束；同 entry 的重叠意味着同一笔金额被两次消费。
        for other in &slices[..index] {
            if other.entry_id == slice.entry_id
                && slice.start < other.end
                && other.start < slice.end
            {
                return Err(FundingRuleError::SliceOverlap);
            }
        }
        total = total.checked_add(slice.amount()).ok_or(FundingRuleError::InvalidAmount)?;
    }
    Ok(total)
}
