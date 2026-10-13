//! 追加式代理授权转换事实的闭集校验。

use super::{ProxyMode, ProxyStatus, ProxyTerminalReason, ProxyTransition, ProxyTransitionKind};
use crate::DomainError;

impl ProxyTransition {
    /// 校验追加式代理转换事实的闭集字段和版本推进。
    pub fn validate(self) -> Result<(), DomainError> {
        if self.authorization_id.is_nil() || self.command_id.is_nil() || self.state_version == 0 {
            return Err(DomainError::InvalidBidState("代理转换缺少有效身份或版本"));
        }
        if !self.new_maximum.is_credit_grid_aligned()
            || self.old_maximum.is_some_and(|amount| !amount.is_credit_grid_aligned())
        {
            return Err(DomainError::InvalidProxyMaximum);
        }

        match self.kind {
            ProxyTransitionKind::Created => {
                if self.state_version != 1
                    || self.old_maximum.is_some()
                    || self.old_mode.is_some()
                    || self.old_status.is_some()
                    || self.new_status != ProxyStatus::Active
                    || self.reason.is_some()
                {
                    return Err(DomainError::InvalidProxyTransition {
                        from: ProxyStatus::Active,
                        to: self.new_status,
                    });
                }
            }
            ProxyTransitionKind::Lowered => {
                let Some(old_maximum) = self.old_maximum else {
                    return Err(DomainError::InvalidProxyMaximum);
                };
                if self.state_version <= 1
                    || self.old_mode != Some(self.new_mode)
                    || !matches!(
                        self.old_status,
                        Some(ProxyStatus::Active | ProxyStatus::FinalFast)
                    )
                    || self.old_status != Some(self.new_status)
                    || (self.new_status == ProxyStatus::FinalFast
                        && self.new_mode != ProxyMode::Fast)
                    || !matches!(self.new_status, ProxyStatus::Active | ProxyStatus::FinalFast)
                    || self.new_maximum >= old_maximum
                    || self.reason.is_some()
                {
                    return Err(DomainError::InvalidProxyMaximum);
                }
            }
            ProxyTransitionKind::FinalFast => {
                if self.state_version <= 1
                    || self.old_mode != Some(ProxyMode::Slow)
                    || self.new_mode != ProxyMode::Fast
                    || self.old_status != Some(ProxyStatus::Active)
                    || self.new_status != ProxyStatus::FinalFast
                    || self.old_maximum != Some(self.new_maximum)
                    || self.reason.is_some()
                {
                    return Err(DomainError::InvalidProxyTransition {
                        from: ProxyStatus::Active,
                        to: self.new_status,
                    });
                }
            }
            ProxyTransitionKind::Exhausted
            | ProxyTransitionKind::Frozen
            | ProxyTransitionKind::Closed => {
                let valid_terminal = match self.kind {
                    ProxyTransitionKind::Exhausted => {
                        self.new_status == ProxyStatus::Exhausted
                            && matches!(
                                self.reason,
                                Some(
                                    ProxyTerminalReason::StrictlyDefeated
                                        | ProxyTerminalReason::RankedBelow
                                )
                            )
                    }
                    ProxyTransitionKind::Frozen => {
                        self.new_status == ProxyStatus::Frozen
                            && self.reason == Some(ProxyTerminalReason::EligibilityLost)
                    }
                    ProxyTransitionKind::Closed => {
                        self.new_status == ProxyStatus::Closed
                            && self.reason == Some(ProxyTerminalReason::CompetitionClosed)
                    }
                    _ => false,
                };
                if self.state_version <= 1
                    || self.old_mode != Some(self.new_mode)
                    || !matches!(
                        self.old_status,
                        Some(ProxyStatus::Active | ProxyStatus::FinalFast)
                    )
                    || (self.old_status == Some(ProxyStatus::FinalFast)
                        && self.old_mode != Some(ProxyMode::Fast))
                    || self.old_maximum != Some(self.new_maximum)
                    || !valid_terminal
                {
                    return Err(DomainError::InvalidProxyTransition {
                        from: ProxyStatus::Active,
                        to: self.new_status,
                    });
                }
            }
        }

        Ok(())
    }
}
