//! 拍卖聚合与生命周期状态机。

use auction_time::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{AuctionId, DomainError};

/// 拍卖状态闭集。
///
/// 聚合只负责结算前的迁移；`Settling` 与 `Settled` 只由存储结算事务在 SQL 守卫下推进
/// （结算领取把 `final` 推进为 `settling`，结算应用把 `settling` 推进为 `settled`），
/// 聚合不提供这两步，避免出现绕过结算事务的第二条迁移路径。
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
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub scheduled_end_at: Option<Timestamp>,
    pub effective_end_at: Option<Timestamp>,
    #[serde(default)]
    pub cancelled_at: Option<Timestamp>,
    #[serde(default)]
    pub cancelled_reason: Option<String>,
    #[serde(default)]
    pub cancelled_by: Option<String>,
}

impl Auction {
    pub fn preview(&mut self, now: Timestamp) -> Result<(), DomainError> {
        self.transition(AuctionStatus::Preview, now)
    }

    /// 原子冻结结束时间并从预览进入开拍状态，只推进一次聚合版本。
    /// lot 激活、结束时间冻结和拍卖开拍是同一个事务事实，不能拆成两个版本。
    pub fn open(&mut self, scheduled_end_at: Timestamp, now: Timestamp) -> Result<(), DomainError> {
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

        self.transition(AuctionStatus::Live, now)?;
        self.scheduled_end_at = Some(scheduled_end_at);
        self.effective_end_at = Some(scheduled_end_at);
        Ok(())
    }

    pub fn enter_final(&mut self, now: Timestamp) -> Result<(), DomainError> {
        self.transition(AuctionStatus::Final, now)
    }

    /// 记录人工提前结算请求；Live 会先进入 Final，Final 则只推进同一聚合版本。
    /// 该方法只关闭新的竞价入口，不直接伪造 Settling/Settled；权威结算仍由持久化 Worker
    /// claim 推进。
    pub fn request_early_settlement(&mut self, now: Timestamp) -> Result<(), DomainError> {
        match self.status {
            AuctionStatus::Live => self.transition(AuctionStatus::Final, now),
            AuctionStatus::Final => {
                self.bump_version()?;
                self.updated_at = now;
                Ok(())
            }
            status => {
                Err(DomainError::InvalidTransition { from: status, to: AuctionStatus::Final })
            }
        }
    }

    /// 取消尚未开拍的 draft/preview，并记录不可缺失的管理事实。
    /// 取消发生在结束时间物化之前，因此不能依赖 live 阶段的取消截止窗口；已开拍状态会
    /// 失败关闭，防止用取消命令绕过竞价收口和结算。
    pub fn cancel_before_open_with_metadata(
        &mut self,
        now: Timestamp,
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

        self.transition(AuctionStatus::Cancelled, now)?;
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

    fn transition(&mut self, next: AuctionStatus, now: Timestamp) -> Result<(), DomainError> {
        if !self.status.can_transition_to(next) {
            return Err(DomainError::InvalidTransition { from: self.status, to: next });
        }

        self.bump_version()?;
        self.status = next;
        self.updated_at = now;
        Ok(())
    }

    /// 业务校验通过后、其他字段写入前推进版本；溢出时必须保持整个聚合不变。
    fn bump_version(&mut self) -> Result<(), DomainError> {
        self.state_version =
            self.state_version.checked_add(1).ok_or(DomainError::StateVersionOverflow)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::assert_matches;
    use std::time::SystemTime;

    use auction_time::{Duration, Timestamp};

    use super::*;

    fn epoch() -> Timestamp {
        Timestamp::from(SystemTime::UNIX_EPOCH)
    }

    /// 存储层装载的聚合快照；版本从 1 起，结束时间尚未物化。
    fn snapshot(status: AuctionStatus) -> Auction {
        let now = epoch();
        Auction {
            id: AuctionId::new(),
            title: "Example".to_owned(),
            status,
            state_version: 1,
            created_at: now,
            updated_at: now,
            scheduled_end_at: None,
            effective_end_at: None,
            cancelled_at: None,
            cancelled_reason: None,
            cancelled_by: None,
        }
    }

    #[test]
    fn draft_follows_the_production_lifecycle() -> Result<(), DomainError> {
        let now = epoch();
        let end = now + Duration::hours(1);
        let mut auction = snapshot(AuctionStatus::Draft);

        auction.preview(now)?;
        auction.open(end, now)?;
        assert_eq!(auction.status, AuctionStatus::Live);
        assert_eq!(auction.effective_end_at, Some(end));
        auction.enter_final(now)?;
        assert_eq!((auction.status, auction.state_version), (AuctionStatus::Final, 4));

        // Final 下的提前结算请求只推进版本，不伪造 Settling。
        auction.request_early_settlement(now)?;
        assert_eq!((auction.status, auction.state_version), (AuctionStatus::Final, 5));

        let mut live = snapshot(AuctionStatus::Live);
        live.request_early_settlement(now)?;
        assert_eq!((live.status, live.state_version), (AuctionStatus::Final, 2));
        Ok(())
    }

    #[test]
    fn aggregate_never_enters_settlement_states() {
        let now = epoch();

        // 结算领取（final → settling）与结算应用（settling → settled）只由存储结算事务推进；
        // 聚合必须拒绝这两步，且被拒绝的迁移不得改动聚合的任何字段。
        for (from, to) in [
            (AuctionStatus::Final, AuctionStatus::Settling),
            (AuctionStatus::Settling, AuctionStatus::Settled),
        ] {
            let mut auction = snapshot(from);
            let before = auction.clone();
            assert_eq!(
                auction.transition(to, now),
                Err(DomainError::InvalidTransition { from, to })
            );
            assert_eq!(auction, before);
        }
    }

    #[test]
    fn invalid_transitions_are_rejected() -> Result<(), DomainError> {
        let now = epoch();

        // 预览不能跳过开拍直接进入 Final。
        let mut preview = snapshot(AuctionStatus::Preview);
        assert_matches!(
            preview.enter_final(now),
            Err(DomainError::InvalidTransition {
                from: AuctionStatus::Preview,
                to: AuctionStatus::Final
            })
        );
        assert_eq!(preview.state_version, 1);

        // 已开拍的场次不能经取消命令绕过竞价收口与结算。
        let mut live = snapshot(AuctionStatus::Live);
        assert_matches!(
            live.cancel_before_open_with_metadata(now, "operator reason", "admin"),
            Err(DomainError::InvalidTransition {
                from: AuctionStatus::Live,
                to: AuctionStatus::Cancelled
            })
        );
        assert_eq!((live.state_version, live.cancelled_at.is_none()), (1, true));

        preview.cancel_before_open_with_metadata(now, "operator reason", "admin")?;
        assert_eq!(preview.status, AuctionStatus::Cancelled);
        assert_eq!(preview.cancelled_reason.as_deref(), Some("operator reason"));
        assert_eq!(preview.cancelled_by.as_deref(), Some("admin"));
        Ok(())
    }
}
