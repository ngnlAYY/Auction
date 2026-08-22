//! 拍卖聚合与生命周期状态机。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{AuctionId, AuctionRules, DomainError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuctionStatus {
    Draft,
    Preview,
    Live,
    Final,
    Settling,
    Settled,
    Cancelled,
}

impl AuctionStatus {
    fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Draft, Self::Preview)
                | (Self::Preview, Self::Live)
                | (Self::Live, Self::Final)
                | (Self::Final, Self::Settling)
                | (Self::Settling, Self::Settled)
                | (Self::Draft, Self::Cancelled)
                | (Self::Preview, Self::Cancelled)
                | (Self::Live, Self::Cancelled)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Auction {
    pub id: AuctionId,
    pub title: String,
    pub status: AuctionStatus,
    pub state_version: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub scheduled_end_at: Option<DateTime<Utc>>,
    pub effective_end_at: Option<DateTime<Utc>>,
}

impl Auction {
    pub fn draft(
        id: AuctionId,
        title: impl Into<String>,
        now: DateTime<Utc>,
    ) -> Result<Self, DomainError> {
        let title = title.into();
        if title.trim().is_empty() {
            return Err(DomainError::EmptyTitle);
        }

        Ok(Self {
            id,
            title,
            status: AuctionStatus::Draft,
            state_version: 1,
            created_at: now,
            updated_at: now,
            scheduled_end_at: None,
            effective_end_at: None,
        })
    }

    /// 为拍卖建立不可变的名义结束时间和当前有效结束时间。
    pub fn schedule_end(
        &mut self,
        scheduled_end_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<(), DomainError> {
        if !matches!(self.status, AuctionStatus::Draft | AuctionStatus::Preview) {
            return Err(DomainError::InvalidSchedule("当前状态不允许设置结束时间"));
        }
        if scheduled_end_at <= now {
            return Err(DomainError::InvalidSchedule("结束时间必须晚于当前时间"));
        }
        if self.scheduled_end_at.is_some() {
            return Err(DomainError::InvalidSchedule("名义结束时间只能设置一次"));
        }

        self.scheduled_end_at = Some(scheduled_end_at);
        self.effective_end_at = Some(scheduled_end_at);
        self.updated_at = now;
        self.bump_version()
    }

    /// 记录领域层已经计算出的软截止延长结果。
    pub fn apply_effective_end(
        &mut self,
        effective_end_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<(), DomainError> {
        if !matches!(self.status, AuctionStatus::Live | AuctionStatus::Final) {
            return Err(DomainError::InvalidSchedule("当前状态不允许延长结束时间"));
        }
        let current_end =
            self.effective_end_at.ok_or(DomainError::InvalidSchedule("拍卖尚未设置结束时间"))?;
        if effective_end_at <= current_end {
            return Err(DomainError::InvalidSchedule("有效结束时间只能向后延长"));
        }

        self.effective_end_at = Some(effective_end_at);
        self.updated_at = now;
        self.bump_version()
    }

    /// 按规则检查取消截止点，不在聚合外复制边界判断。
    pub fn cancel_with_rules(
        &mut self,
        now: DateTime<Utc>,
        rules: AuctionRules,
    ) -> Result<(), DomainError> {
        let scheduled_end_at = self
            .scheduled_end_at
            .ok_or(DomainError::InvalidSchedule("拍卖尚未设置名义结束时间"))?;
        if !rules.can_cancel(scheduled_end_at, now)? {
            return Err(DomainError::CancellationCutoff);
        }

        self.cancel(now)
    }

    pub fn preview(&mut self, now: DateTime<Utc>) -> Result<(), DomainError> {
        self.transition(AuctionStatus::Preview, now)
    }

    pub fn go_live(&mut self, now: DateTime<Utc>) -> Result<(), DomainError> {
        self.transition(AuctionStatus::Live, now)
    }

    pub fn enter_final(&mut self, now: DateTime<Utc>) -> Result<(), DomainError> {
        self.transition(AuctionStatus::Final, now)
    }

    pub fn begin_settlement(&mut self, now: DateTime<Utc>) -> Result<(), DomainError> {
        self.transition(AuctionStatus::Settling, now)
    }

    pub fn settle(&mut self, now: DateTime<Utc>) -> Result<(), DomainError> {
        self.transition(AuctionStatus::Settled, now)
    }

    pub fn cancel(&mut self, now: DateTime<Utc>) -> Result<(), DomainError> {
        self.transition(AuctionStatus::Cancelled, now)
    }

    fn transition(&mut self, next: AuctionStatus, now: DateTime<Utc>) -> Result<(), DomainError> {
        if !self.status.can_transition_to(next) {
            return Err(DomainError::InvalidTransition { from: self.status, to: next });
        }

        self.status = next;
        self.updated_at = now;
        self.bump_version()
    }

    fn bump_version(&mut self) -> Result<(), DomainError> {
        self.state_version =
            self.state_version.checked_add(1).ok_or(DomainError::StateVersionOverflow)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use chrono::{DateTime, Utc};

    use super::*;

    #[test]
    fn draft_follows_the_initial_lifecycle() {
        let now = DateTime::<Utc>::from(SystemTime::UNIX_EPOCH);
        let mut auction = Auction::draft(AuctionId::new(), "Example", now).unwrap();

        auction.preview(now).unwrap();
        auction.go_live(now).unwrap();
        auction.enter_final(now).unwrap();
        auction.begin_settlement(now).unwrap();
        auction.settle(now).unwrap();

        assert_eq!(auction.status, AuctionStatus::Settled);
    }

    #[test]
    fn invalid_transitions_are_rejected() {
        let now = DateTime::<Utc>::from(SystemTime::UNIX_EPOCH);
        let mut auction = Auction::draft(AuctionId::new(), "Example", now).unwrap();

        let error = auction.settle(now).unwrap_err();

        assert!(matches!(error, DomainError::InvalidTransition { .. }));
    }
}
