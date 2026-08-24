//! Public 补偿池与结算账本读取端口。
//!
//! 补偿池只公开已经提交的结算事实。游标、余额守恒和 movement 的 lot 投影在应用边界
//! 校验，存储适配器不得把内部结算计划或未提交的中间状态直接暴露给 HTTP。

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::ApplicationFuture;

/// Public 补偿池编号页的默认大小。
pub const DEFAULT_PUBLIC_COMPENSATION_PAGE_SIZE: i64 = 25;

/// Public 补偿账本页的默认大小。
pub const DEFAULT_PUBLIC_COMPENSATION_MOVEMENT_PAGE_SIZE: i64 = 50;

/// Public 补偿池允许的最大页大小。
pub const MAX_PUBLIC_COMPENSATION_PAGE_SIZE: i64 = 200;

/// Public 补偿池允许的最大 legacy offset。
pub const MAX_PUBLIC_COMPENSATION_OFFSET: i64 = 10_000;

/// Public 补偿池批次的稳定 keyset 游标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicCompensationCursor {
    /// 显式请求第一页 cursor 视图。
    Start,
    /// 按拍卖序号、拍卖 ID 倒序排列的边界。
    Boundary { sequence: i64, id: Uuid },
}

/// Public 补偿池批次列表请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicCompensationListRequest {
    pub limit: i64,
    pub offset: i64,
    pub cursor: Option<PublicCompensationCursor>,
}

impl PublicCompensationListRequest {
    /// 在应用端口边界拒绝无界分页和不完整游标。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.limit < 1
            || self.limit > MAX_PUBLIC_COMPENSATION_PAGE_SIZE
            || self.offset < 0
            || self.offset > MAX_PUBLIC_COMPENSATION_OFFSET
            || self.cursor.is_some() && self.offset != 0
        {
            return Err("公开补偿池分页参数无效");
        }
        if let Some(PublicCompensationCursor::Boundary { sequence, id }) = self.cursor.as_ref()
            && (*sequence <= 0 || id.is_nil())
        {
            return Err("公开补偿池游标无效");
        }
        Ok(())
    }
}

/// 已结算拍卖的补偿池守恒摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicCompensationRound {
    pub auction_id: Uuid,
    pub world_id: String,
    pub auction_sequence: i64,
    pub opening_balance: i64,
    pub tax_income: i64,
    pub discount_demand: i64,
    pub compensation_out: i64,
    pub closing_balance: i64,
    pub movement_count: i64,
    pub settled_at: DateTime<Utc>,
}

impl PublicCompensationRound {
    /// 校验补偿池守恒方程和公开字段。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.auction_id.is_nil()
            || self.world_id.is_empty()
            || self.world_id != self.world_id.trim()
            || self.world_id.len() > 64
            || self.world_id.chars().any(char::is_control)
            || self.auction_sequence <= 0
            || self.opening_balance < 0
            || self.tax_income < 0
            || self.discount_demand < 0
            || self.compensation_out < 0
            || self.closing_balance < 0
            || self.movement_count < 2
            || self.movement_count > i64::from(i32::MAX)
            || self.settled_at.timestamp_micros() == 0
        {
            return Err("公开补偿池批次无效");
        }
        let available =
            self.opening_balance.checked_add(self.tax_income).ok_or("公开补偿池余额溢出")?;
        if self.compensation_out > available
            || available - self.compensation_out != self.closing_balance
        {
            return Err("公开补偿池守恒方程无效");
        }
        Ok(())
    }
}

/// Public 补偿池批次列表页。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicCompensationPage {
    pub entries: Vec<PublicCompensationRound>,
    pub balance: i64,
    pub updated_at: Option<DateTime<Utc>>,
    pub total: Option<i64>,
    pub limit: i64,
    pub offset: i64,
    pub next_cursor: Option<PublicCompensationCursor>,
    pub has_more: bool,
}

impl PublicCompensationPage {
    /// 校验存储适配器返回的页结构，但保留编号页/cursor 页的 total 差异。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.limit < 1
            || self.limit > MAX_PUBLIC_COMPENSATION_PAGE_SIZE
            || self.offset < 0
            || self.offset > MAX_PUBLIC_COMPENSATION_OFFSET
            || self.balance < 0
            || self.entries.len() as i64 > self.limit
            || self.total.is_some_and(|total| total < 0 || total < self.entries.len() as i64)
            || self.next_cursor.is_some() != self.has_more
            || self.updated_at.is_some_and(|value| value.timestamp_micros() == 0)
        {
            return Err("公开补偿池页无效");
        }
        for entry in &self.entries {
            entry.validate()?;
        }
        if let Some(PublicCompensationCursor::Boundary { sequence, id }) = self.next_cursor.as_ref()
            && (*sequence <= 0 || id.is_nil())
        {
            return Err("公开补偿池下一页游标无效");
        }
        Ok(())
    }
}

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

        if self.offset + self.entries.len() as i64 == self.total && !self.entries.is_empty() {
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

/// Public 补偿池读取端口。
pub trait PublicCompensationReader: Send + Sync {
    fn list_public_compensation(
        &self,
        request: PublicCompensationListRequest,
    ) -> ApplicationFuture<'_, PublicCompensationPage>;

    fn list_public_compensation_movements(
        &self,
        auction_id: Uuid,
        request: PublicCompensationMovementRequest,
    ) -> ApplicationFuture<'_, Option<PublicCompensationMovementPage>>;
}
