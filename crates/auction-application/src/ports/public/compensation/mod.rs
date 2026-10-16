//! Public 补偿池与结算账本读取端口。
//!
//! 补偿池只公开已经提交的结算事实。游标、余额守恒和 movement 的 lot 投影在应用边界
//! 校验，存储适配器不得把内部结算计划或未提交的中间状态直接暴露给 HTTP。

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::ApplicationFuture;

mod movement;

pub use movement::{
    PublicCompensationMovement, PublicCompensationMovementKind, PublicCompensationMovementPage,
    PublicCompensationMovementRequest,
};

/// Public 补偿池编号页的默认大小。
pub const DEFAULT_PUBLIC_COMPENSATION_PAGE_SIZE: i64 = 25;

/// Public 补偿账本页的默认大小。
pub const DEFAULT_PUBLIC_COMPENSATION_MOVEMENT_PAGE_SIZE: i64 = 50;

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
    /// 校验正整数数量、既有 offset 上限和完整游标，不设数量业务上限。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.limit < 1
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
        let entry_count = i64::try_from(self.entries.len()).map_err(|_| "公开补偿池页大小溢出")?;
        if self.limit < 1
            || self.offset < 0
            || self.offset > MAX_PUBLIC_COMPENSATION_OFFSET
            || self.balance < 0
            || entry_count > self.limit
            || self.total.is_some_and(|total| total < 0 || total < entry_count)
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
