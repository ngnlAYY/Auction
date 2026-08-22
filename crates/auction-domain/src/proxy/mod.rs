use serde::{Deserialize, Serialize};

use crate::{BidAmount, CommandId, DomainError, LotId, ProxyAuthorizationId, UserId};

/// 代理竞价的调度模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProxyMode {
    Fast,
    Slow,
}

/// 代理授权的生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProxyStatus {
    Active,
    FinalFast,
    Exhausted,
    Frozen,
    Closed,
}

impl ProxyStatus {
    fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Active, Self::FinalFast)
                | (Self::Active, Self::Exhausted)
                | (Self::Active, Self::Frozen)
                | (Self::Active, Self::Closed)
                | (Self::FinalFast, Self::Exhausted)
                | (Self::FinalFast, Self::Frozen)
                | (Self::FinalFast, Self::Closed)
        )
    }
}

/// 代理授权终态的原因闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProxyTerminalReason {
    StrictlyDefeated,
    RankedBelow,
    EligibilityLost,
    CompetitionClosed,
}

/// 追加式代理授权转换的分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProxyTransitionKind {
    Created,
    Lowered,
    FinalFast,
    Exhausted,
    Frozen,
    Closed,
}

/// 代理授权当前快照，身份和版本由领域层约束。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyAuthorization {
    pub id: ProxyAuthorizationId,
    pub lot_id: LotId,
    pub user_id: UserId,
    pub creation_command_id: CommandId,
    pub authorization_sequence: u64,
    pub usage_number: u64,
    pub maximum_amount: BidAmount,
    pub mode: ProxyMode,
    pub status: ProxyStatus,
    pub state_version: u64,
}

impl ProxyAuthorization {
    /// 创建一个从 command_id 派生的活动代理授权快照。
    // 八个参数分别对应身份、序号、重入次数、金额和模式，合并为结构体会削弱调用点的字段语义。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: ProxyAuthorizationId,
        lot_id: LotId,
        user_id: UserId,
        creation_command_id: CommandId,
        authorization_sequence: u64,
        usage_number: u64,
        maximum_amount: BidAmount,
        mode: ProxyMode,
    ) -> Result<Self, DomainError> {
        if id.is_nil() || lot_id.is_nil() || user_id.is_nil() || creation_command_id.is_nil() {
            return Err(DomainError::InvalidBidState("代理授权缺少有效标识"));
        }
        if authorization_sequence == 0 || usage_number == 0 {
            return Err(DomainError::InvalidBidState("代理授权序号和使用次数必须为正数"));
        }

        Ok(Self {
            id,
            lot_id,
            user_id,
            creation_command_id,
            authorization_sequence,
            usage_number,
            maximum_amount,
            mode,
            status: ProxyStatus::Active,
            state_version: 1,
        })
    }

    /// 创建代理授权对应的第一条追加式转换事实。
    pub fn creation_transition(&self) -> Result<ProxyTransition, DomainError> {
        self.validate()?;

        Ok(ProxyTransition {
            authorization_id: self.id,
            command_id: self.creation_command_id,
            kind: ProxyTransitionKind::Created,
            old_maximum: None,
            new_maximum: self.maximum_amount,
            old_mode: None,
            new_mode: self.mode,
            old_status: None,
            new_status: self.status,
            state_version: self.state_version,
            reason: None,
        })
    }

    /// 校验代理授权当前快照的身份、金额和版本不变量。
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.id.is_nil()
            || self.lot_id.is_nil()
            || self.user_id.is_nil()
            || self.creation_command_id.is_nil()
            || self.authorization_sequence == 0
            || self.usage_number == 0
            || self.state_version == 0
        {
            return Err(DomainError::InvalidBidState("代理授权快照缺少有效身份或版本"));
        }
        if self.status == ProxyStatus::FinalFast && self.mode != ProxyMode::Fast {
            return Err(DomainError::InvalidProxyTransition { from: self.status, to: self.status });
        }
        Ok(())
    }

    /// 将活动授权的上限降低，并返回待追加的转换事实。
    pub fn lower(
        &mut self,
        command_id: CommandId,
        new_maximum: BidAmount,
    ) -> Result<ProxyTransition, DomainError> {
        if command_id.is_nil()
            || self.status != ProxyStatus::Active
            || new_maximum >= self.maximum_amount
        {
            return Err(DomainError::InvalidProxyMaximum);
        }

        let old_maximum = self.maximum_amount;
        self.maximum_amount = new_maximum;
        let state_version = self.bump_version()?;
        Ok(ProxyTransition {
            authorization_id: self.id,
            command_id,
            kind: ProxyTransitionKind::Lowered,
            old_maximum: Some(old_maximum),
            new_maximum,
            old_mode: Some(self.mode),
            new_mode: self.mode,
            old_status: Some(ProxyStatus::Active),
            new_status: ProxyStatus::Active,
            state_version,
            reason: None,
        })
    }

    /// 将慢速代理提升到最后阶段的快速模式。
    pub fn promote_to_final_fast(
        &mut self,
        command_id: CommandId,
    ) -> Result<ProxyTransition, DomainError> {
        if command_id.is_nil()
            || self.mode != ProxyMode::Slow
            || !self.status.can_transition_to(ProxyStatus::FinalFast)
        {
            return Err(DomainError::InvalidProxyTransition {
                from: self.status,
                to: ProxyStatus::FinalFast,
            });
        }

        let state_version =
            self.state_version.checked_add(1).ok_or(DomainError::StateVersionOverflow)?;
        let old_mode = self.mode;
        let old_status = self.status;
        self.mode = ProxyMode::Fast;
        self.status = ProxyStatus::FinalFast;
        self.state_version = state_version;

        Ok(ProxyTransition {
            authorization_id: self.id,
            command_id,
            kind: ProxyTransitionKind::FinalFast,
            old_maximum: Some(self.maximum_amount),
            new_maximum: self.maximum_amount,
            old_mode: Some(old_mode),
            new_mode: self.mode,
            old_status: Some(old_status),
            new_status: self.status,
            state_version,
            reason: None,
        })
    }

    /// 将代理标记为已耗尽。
    pub fn exhaust(
        &mut self,
        command_id: CommandId,
        reason: ProxyTerminalReason,
    ) -> Result<ProxyTransition, DomainError> {
        self.transition(
            command_id,
            ProxyStatus::Exhausted,
            ProxyTransitionKind::Exhausted,
            Some(reason),
        )
    }

    /// 将代理冻结，保留冻结原因供审计记录。
    pub fn freeze(
        &mut self,
        command_id: CommandId,
        reason: ProxyTerminalReason,
    ) -> Result<ProxyTransition, DomainError> {
        self.transition(command_id, ProxyStatus::Frozen, ProxyTransitionKind::Frozen, Some(reason))
    }

    /// 关闭仍然有效的代理授权。
    pub fn close(&mut self, command_id: CommandId) -> Result<ProxyTransition, DomainError> {
        self.transition(
            command_id,
            ProxyStatus::Closed,
            ProxyTransitionKind::Closed,
            Some(ProxyTerminalReason::CompetitionClosed),
        )
    }

    fn transition(
        &mut self,
        command_id: CommandId,
        next_status: ProxyStatus,
        kind: ProxyTransitionKind,
        reason: Option<ProxyTerminalReason>,
    ) -> Result<ProxyTransition, DomainError> {
        if command_id.is_nil() || !self.status.can_transition_to(next_status) {
            return Err(DomainError::InvalidProxyTransition { from: self.status, to: next_status });
        }

        let old_status = self.status;
        self.status = next_status;
        let state_version = self.bump_version()?;
        Ok(ProxyTransition {
            authorization_id: self.id,
            command_id,
            kind,
            old_maximum: Some(self.maximum_amount),
            new_maximum: self.maximum_amount,
            old_mode: Some(self.mode),
            new_mode: self.mode,
            old_status: Some(old_status),
            new_status: next_status,
            state_version,
            reason,
        })
    }

    fn bump_version(&mut self) -> Result<u64, DomainError> {
        self.state_version =
            self.state_version.checked_add(1).ok_or(DomainError::StateVersionOverflow)?;
        Ok(self.state_version)
    }
}

/// 应用层在事务中追加的代理授权转换事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyTransition {
    pub authorization_id: ProxyAuthorizationId,
    pub command_id: CommandId,
    pub kind: ProxyTransitionKind,
    pub old_maximum: Option<BidAmount>,
    pub new_maximum: BidAmount,
    pub old_mode: Option<ProxyMode>,
    pub new_mode: ProxyMode,
    pub old_status: Option<ProxyStatus>,
    pub new_status: ProxyStatus,
    pub state_version: u64,
    pub reason: Option<ProxyTerminalReason>,
}

impl ProxyTransition {
    /// 校验追加式代理转换事实的闭集字段和版本推进。
    pub fn validate(self) -> Result<(), DomainError> {
        if self.authorization_id.is_nil() || self.command_id.is_nil() || self.state_version == 0 {
            return Err(DomainError::InvalidBidState("代理转换缺少有效身份或版本"));
        }

        match self.kind {
            ProxyTransitionKind::Created => {
                if self.state_version != 1
                    || self.old_maximum.is_some()
                    || self.old_mode.is_some()
                    || self.old_status.is_some()
                    || self.new_status != ProxyStatus::Active
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
                    || self.old_status != Some(ProxyStatus::Active)
                    || self.new_status != ProxyStatus::Active
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
