//! Public 补偿账本逐笔流水模型与一致性校验。

use super::*;

/// 补偿账本允许公开的 movement 类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicCompensationMovementKind {
    Opening,
    Tax,
    Compensation,
    Closing,
}

impl PublicCompensationMovementKind {
    /// 返回冻结的 JSON 字符串。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Opening => "opening",
            Self::Tax => "tax",
            Self::Compensation => "compensation",
            Self::Closing => "closing",
        }
    }

    /// 将数据库中的闭集字符串映射为安全枚举。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "opening" => Some(Self::Opening),
            "tax" => Some(Self::Tax),
            "compensation" => Some(Self::Compensation),
            "closing" => Some(Self::Closing),
            _ => None,
        }
    }
}

/// 单条公开补偿账本事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicCompensationMovement {
    pub auction_id: Uuid,
    pub sequence: i32,
    pub lot_id: Option<Uuid>,
    pub lot_number: Option<i32>,
    pub equipment_name: Option<String>,
    pub equipment_category: Option<String>,
    pub kind: PublicCompensationMovementKind,
    pub amount: i64,
    pub balance_after: i64,
    pub created_at: DateTime<Utc>,
}

impl PublicCompensationMovement {
    /// 校验 movement kind、符号和 lot 公开投影的一致性。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.auction_id.is_nil()
            || self.sequence <= 0
            || self.balance_after < 0
            || self.created_at.timestamp_micros() == 0
        {
            return Err("公开补偿账本条目无效");
        }
        let has_lot = self.lot_id.is_some()
            || self.lot_number.is_some()
            || self.equipment_name.is_some()
            || self.equipment_category.is_some();
        let valid_lot = self.lot_id.is_some_and(|value| !value.is_nil())
            && self.lot_number.is_some_and(|value| value > 0)
            && self.equipment_name.as_deref().is_some_and(|value| {
                !value.is_empty()
                    && value.len() <= 256
                    && value == value.trim()
                    && !value.chars().any(char::is_control)
            })
            && self.equipment_category.as_deref().is_some_and(|value| {
                !value.is_empty()
                    && value.len() <= 64
                    && value == value.trim()
                    && !value.chars().any(char::is_control)
            });

        match self.kind {
            PublicCompensationMovementKind::Opening => {
                if has_lot || self.amount < 0 || self.amount != self.balance_after {
                    return Err("公开补偿账本 opening 无效");
                }
            }
            PublicCompensationMovementKind::Tax => {
                if !valid_lot || self.amount <= 0 {
                    return Err("公开补偿账本 tax 无效");
                }
            }
            PublicCompensationMovementKind::Compensation => {
                if !valid_lot || self.amount >= 0 {
                    return Err("公开补偿账本 compensation 无效");
                }
            }
            PublicCompensationMovementKind::Closing => {
                if has_lot || self.amount != 0 {
                    return Err("公开补偿账本 closing 无效");
                }
            }
        }
        Ok(())
    }
}

/// 单场补偿账本分页请求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicCompensationMovementRequest {
    pub limit: i64,
    pub offset: i64,
}

impl PublicCompensationMovementRequest {
    /// 校验单场账本的编号分页。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.limit < 1
            || self.limit > MAX_PUBLIC_COMPENSATION_PAGE_SIZE
            || self.offset < 0
            || self.offset > MAX_PUBLIC_COMPENSATION_OFFSET
        {
            return Err("公开补偿账本分页参数无效");
        }
        Ok(())
    }
}

/// 绑定同一快照读取的补偿摘要与账本页。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicCompensationMovementPage {
    pub round: PublicCompensationRound,
    pub entries: Vec<PublicCompensationMovement>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

impl PublicCompensationMovementPage {
    /// 校验序号、首尾余额和当前页可观察到的余额链。
    pub fn validate(&self) -> Result<(), &'static str> {
        self.round.validate()?;
        let request = PublicCompensationMovementRequest { limit: self.limit, offset: self.offset };
        request.validate()?;
        if self.total != self.round.movement_count
            || self.total < 2
            || self.entries.len() as i64 > self.limit
        {
            return Err("公开补偿账本页无效");
        }

        for (index, entry) in self.entries.iter().enumerate() {
            let expected_sequence = self
                .offset
                .checked_add(index as i64)
                .and_then(|value| value.checked_add(1))
                .ok_or("公开补偿账本序号溢出")?;
            if entry.validate().is_err()
                || entry.auction_id != self.round.auction_id
                || i64::from(entry.sequence) != expected_sequence
            {
                return Err("公开补偿账本页包含无效条目");
            }
        }

        if self.offset == 0 && !self.entries.is_empty() {
            let mut balance = 0_i64;
            for entry in &self.entries {
                balance = balance.checked_add(entry.amount).ok_or("公开补偿账本余额溢出")?;
                if balance != entry.balance_after {
                    return Err("公开补偿账本余额链无效");
                }
            }
            if self.entries[0].kind != PublicCompensationMovementKind::Opening
                || self.entries[0].balance_after != self.round.opening_balance
            {
                return Err("公开补偿账本 opening 与摘要不一致");
            }
        }

        let page_end = self
            .offset
            .checked_add(i64::try_from(self.entries.len()).map_err(|_| "公开补偿账本页大小溢出")?)
            .ok_or("公开补偿账本页偏移溢出")?;
        if page_end == self.total && !self.entries.is_empty() {
            let last = self.entries.last().ok_or("公开补偿账本末尾缺失")?;
            if last.kind != PublicCompensationMovementKind::Closing
                || last.balance_after != self.round.closing_balance
            {
                return Err("公开补偿账本 closing 与摘要不一致");
            }
        }
        Ok(())
    }
}
