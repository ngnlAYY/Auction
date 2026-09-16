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
    #[serde(default)]
    pub cancelled_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub cancelled_reason: Option<String>,
    #[serde(default)]
    pub cancelled_by: Option<String>,
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
            cancelled_at: None,
            cancelled_reason: None,
            cancelled_by: None,
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

    /// 按规则取消并记录管理端要求的原因与操作者；原因属于审计事实，不能在存储层静默补造。
    pub fn cancel_with_rules_and_metadata(
        &mut self,
        now: DateTime<Utc>,
        rules: AuctionRules,
        reason: impl Into<String>,
        actor: impl Into<String>,
    ) -> Result<(), DomainError> {
        let (reason, actor) = Self::validate_cancellation_metadata(reason, actor)?;
        self.cancel_with_rules(now, rules)?;
        self.cancelled_at = Some(now);
        self.cancelled_reason = Some(reason);
        self.cancelled_by = Some(actor);
        Ok(())
    }

    pub fn preview(&mut self, now: DateTime<Utc>) -> Result<(), DomainError> {
        self.transition(AuctionStatus::Preview, now)
    }

    pub fn go_live(&mut self, now: DateTime<Utc>) -> Result<(), DomainError> {
        self.transition(AuctionStatus::Live, now)
    }

    /// 原子冻结结束时间并从预览进入开拍状态，只推进一次聚合版本。
    ///
    /// 旧系统把 lot 激活、结束时间冻结和拍卖开拍视为一个事务事实；该方法避免先调用
    /// `schedule_end` 再迁移状态而产生两个无法对应单条命令的版本。
    pub fn open(
        &mut self,
        scheduled_end_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<(), DomainError> {
        if self.status != AuctionStatus::Preview {
            return Err(DomainError::InvalidTransition {
                from: self.status,
                to: AuctionStatus::Live,
            });
        }
        if self.scheduled_end_at.is_some() || self.effective_end_at.is_some() {
            return Err(DomainError::InvalidSchedule("开拍前结束时间必须尚未物化"));
        }
        if scheduled_end_at <= now {
            return Err(DomainError::InvalidSchedule("结束时间必须晚于开拍时间"));
        }

        self.scheduled_end_at = Some(scheduled_end_at);
        self.effective_end_at = Some(scheduled_end_at);
        self.transition(AuctionStatus::Live, now)
    }

    pub fn enter_final(&mut self, now: DateTime<Utc>) -> Result<(), DomainError> {
        self.transition(AuctionStatus::Final, now)
    }

    /// 记录人工提前结算请求；Live 会先进入 Final，Final 则只推进同一聚合版本。
    ///
    /// 该方法只关闭新的竞价入口，不直接伪造 Settling/Settled；权威结算仍由持久化 Worker
    /// claim 推进。
    pub fn request_early_settlement(&mut self, now: DateTime<Utc>) -> Result<(), DomainError> {
        match self.status {
            AuctionStatus::Live => self.transition(AuctionStatus::Final, now),
            AuctionStatus::Final => {
                self.updated_at = now;
                self.bump_version()
            }
            status => {
                Err(DomainError::InvalidTransition { from: status, to: AuctionStatus::Final })
            }
        }
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

    /// 取消尚未开拍的 draft/preview，并记录不可缺失的管理事实。
    ///
    /// 取消发生在结束时间物化之前，因此不能依赖 live 阶段的取消截止窗口；已开拍状态会
    /// 失败关闭，防止用取消命令绕过竞价收口和结算。
    pub fn cancel_before_open_with_metadata(
        &mut self,
        now: DateTime<Utc>,
        reason: impl Into<String>,
        actor: impl Into<String>,
    ) -> Result<(), DomainError> {
        let (reason, actor) = Self::validate_cancellation_metadata(reason, actor)?;
        if !matches!(self.status, AuctionStatus::Draft | AuctionStatus::Preview) {
            return Err(DomainError::InvalidTransition {
                from: self.status,
                to: AuctionStatus::Cancelled,
            });
        }

        self.cancel(now)?;
        self.cancelled_at = Some(now);
        self.cancelled_reason = Some(reason);
        self.cancelled_by = Some(actor);
        Ok(())
    }

    /// 统一校验取消原因和操作者，避免不同取消入口产生不一致的审计边界。
    fn validate_cancellation_metadata(
        reason: impl Into<String>,
        actor: impl Into<String>,
    ) -> Result<(String, String), DomainError> {
        let reason = reason.into();
        let actor = actor.into();
        if reason.trim().is_empty()
            || reason.len() > 1024
            || reason.chars().any(char::is_control)
            || actor.trim().is_empty()
            || actor.len() > 128
            || actor.chars().any(char::is_control)
        {
            return Err(DomainError::InvalidCancellationMetadata);
        }
        Ok((reason, actor))
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
    fn draft_follows_the_initial_lifecycle() -> Result<(), DomainError> {
        let now = DateTime::<Utc>::from(SystemTime::UNIX_EPOCH);
        let mut auction = Auction::draft(AuctionId::new(), "Example", now)?;

        auction.preview(now)?;
        auction.go_live(now)?;
        auction.enter_final(now)?;
        auction.begin_settlement(now)?;
        auction.settle(now)?;

        assert_eq!(auction.status, AuctionStatus::Settled);
        Ok(())
    }

    #[test]
    fn invalid_transitions_are_rejected() -> Result<(), DomainError> {
        let now = DateTime::<Utc>::from(SystemTime::UNIX_EPOCH);
        let mut auction = Auction::draft(AuctionId::new(), "Example", now)?;

        let result = auction.settle(now);

        assert!(matches!(result, Err(DomainError::InvalidTransition { .. })));
        Ok(())
    }
}
