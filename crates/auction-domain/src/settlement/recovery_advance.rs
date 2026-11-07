//! 恢复 case 的阶段推进规则：主世界只等付款到账，异世界先收付款再交付装备。
//! 只决定下一步动作、提醒计数与完成时间；时钟、租约、行锁、审计和外部副作用由调用方
//! 在同一事务内完成，本模块不读取数据库也不写日志。

use chrono::{DateTime, Utc};

use crate::SettlementWorld;

/// 恢复阶段闭集；与 `settlement_recoveries.stage` 的持久化取值一一对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryStage {
    AwaitingPayment,
    PaymentOverdue,
    DeliveringEquipment,
    Completed,
}

impl RecoveryStage {
    /// 持久化取值；完成态只由本模块的推进结果写入。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AwaitingPayment => "awaiting_payment",
            Self::PaymentOverdue => "payment_overdue",
            Self::DeliveringEquipment => "delivering_equipment",
            Self::Completed => "completed",
        }
    }

    /// 只接受闭集取值；未知阶段失败关闭，不做别名映射。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "awaiting_payment" => Some(Self::AwaitingPayment),
            "payment_overdue" => Some(Self::PaymentOverdue),
            "delivering_equipment" => Some(Self::DeliveringEquipment),
            "completed" => Some(Self::Completed),
            _ => None,
        }
    }
}

/// 一次推进所需的锁定事实；时间均已由调用方在同一事务内读取。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryAdvanceFacts {
    pub world: SettlementWorld,
    pub stage: RecoveryStage,
    pub payment_satisfied: bool,
    pub payment_completed_at: Option<DateTime<Utc>>,
    pub equipment_satisfied: bool,
    pub equipment_completed_at: Option<DateTime<Utc>>,
    pub reminder_count: i32,
    pub reminder_limit: i32,
    /// 付款投递已有自动或人工发送证明；缺失时逾期提醒只延后，不提前提醒。
    pub payment_sent_proven: bool,
    pub created_at: DateTime<Utc>,
    /// 本次事务的读取时刻，用作完成时间的上界。
    pub observed_at: DateTime<Utc>,
}

/// 推进结果；调用方据此选择唯一的 UPDATE/审计分支。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAdvance {
    Completed { completed_at: DateTime<Utc> },
    DeliverEquipment,
    RemindPayment { reminder_count: i32 },
    DeferPaymentReminder,
}

/// 非法推进原因；消息保持既有运维文案，调用方不再拼接第二套判断。
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RecoveryAdvanceError {
    #[error("recovery 完成事实缺少履约时间")]
    MissingCompletionTime,

    #[error("recovery 完成事实时间越界")]
    CompletionTimeOutOfRange,

    #[error("recovery 提醒次数已超过冻结上限")]
    ReminderLimitExceeded,

    #[error("recovery 提醒次数溢出")]
    ReminderCountOverflow,

    #[error("recovery due case 状态不支持推进")]
    UnsupportedStage,
}

/// 计算下一次推进；同一组事实的结论唯一，且不依赖本地时区或调用顺序。
pub fn next_recovery_advance(
    facts: &RecoveryAdvanceFacts,
) -> Result<RecoveryAdvance, RecoveryAdvanceError> {
    let completed = |at: Option<DateTime<Utc>>| {
        let completed_at = at.ok_or(RecoveryAdvanceError::MissingCompletionTime)?;
        if completed_at < facts.created_at || completed_at > facts.observed_at {
            return Err(RecoveryAdvanceError::CompletionTimeOutOfRange);
        }
        Ok(RecoveryAdvance::Completed { completed_at })
    };
    // 完成优先于阶段：付款履约已满足时，即便还停留在提醒阶段也直接结束。
    if facts.world == SettlementWorld::Main && facts.payment_satisfied {
        return completed(facts.payment_completed_at);
    }
    if facts.world == SettlementWorld::Isekai && facts.equipment_satisfied {
        return completed(facts.equipment_completed_at);
    }
    // 异世界付款已收但装备未交付：进入交付阶段，等待装备履约完成后结束。
    if facts.world == SettlementWorld::Isekai && facts.payment_satisfied {
        return Ok(RecoveryAdvance::DeliverEquipment);
    }
    if facts.stage != RecoveryStage::AwaitingPayment {
        return Err(RecoveryAdvanceError::UnsupportedStage);
    }
    if !facts.payment_sent_proven {
        return Ok(RecoveryAdvance::DeferPaymentReminder);
    }
    if facts.reminder_count >= facts.reminder_limit {
        return Err(RecoveryAdvanceError::ReminderLimitExceeded);
    }
    let reminder_count =
        facts.reminder_count.checked_add(1).ok_or(RecoveryAdvanceError::ReminderCountOverflow)?;
    Ok(RecoveryAdvance::RemindPayment { reminder_count })
}

#[cfg(test)]
mod tests {
    use self::parent::*;
    use crate::settlement::recovery_advance as parent;

    fn at(minute: u32) -> DateTime<Utc> {
        chrono::DateTime::parse_from_rfc3339(&format!("2026-11-02T00:{minute:02}:00Z"))
            .expect("固定测试时间")
            .with_timezone(&Utc)
    }

    fn facts() -> RecoveryAdvanceFacts {
        RecoveryAdvanceFacts {
            world: SettlementWorld::Main,
            stage: RecoveryStage::AwaitingPayment,
            payment_satisfied: false,
            payment_completed_at: None,
            equipment_satisfied: false,
            equipment_completed_at: None,
            reminder_count: 0,
            reminder_limit: 3,
            payment_sent_proven: true,
            created_at: at(0),
            observed_at: at(10),
        }
    }

    #[test]
    fn stage_strings_round_trip_and_reject_unknown() {
        for stage in [
            RecoveryStage::AwaitingPayment,
            RecoveryStage::PaymentOverdue,
            RecoveryStage::DeliveringEquipment,
            RecoveryStage::Completed,
        ] {
            assert_eq!(RecoveryStage::parse(stage.as_str()), Some(stage));
        }
        assert_eq!(RecoveryStage::parse("refunded"), None);
    }

    #[test]
    fn main_world_completes_on_payment() {
        let facts = RecoveryAdvanceFacts {
            payment_satisfied: true,
            payment_completed_at: Some(at(5)),
            ..facts()
        };
        assert_eq!(
            next_recovery_advance(&facts),
            Ok(RecoveryAdvance::Completed { completed_at: at(5) })
        );
    }

    #[test]
    fn isekai_waits_for_equipment_and_completes_on_it() {
        let paid = RecoveryAdvanceFacts {
            world: SettlementWorld::Isekai,
            payment_satisfied: true,
            payment_completed_at: Some(at(4)),
            ..facts()
        };
        assert_eq!(next_recovery_advance(&paid), Ok(RecoveryAdvance::DeliverEquipment));
        // 交付阶段内付款仍满足但装备未完成：不是提醒，也不能重复结束。
        let delivering = RecoveryAdvanceFacts { stage: RecoveryStage::DeliveringEquipment, ..paid };
        assert_eq!(next_recovery_advance(&delivering), Ok(RecoveryAdvance::DeliverEquipment));
        let shipped = RecoveryAdvanceFacts {
            equipment_satisfied: true,
            equipment_completed_at: Some(at(9)),
            ..delivering
        };
        assert_eq!(
            next_recovery_advance(&shipped),
            Ok(RecoveryAdvance::Completed { completed_at: at(9) })
        );
    }

    #[test]
    fn missing_or_out_of_range_completion_time_fails_closed() {
        let missing = RecoveryAdvanceFacts { payment_satisfied: true, ..facts() };
        assert_eq!(
            next_recovery_advance(&missing),
            Err(RecoveryAdvanceError::MissingCompletionTime)
        );
        let early = RecoveryAdvanceFacts {
            payment_completed_at: Some(at(0) - chrono::Duration::minutes(1)),
            ..missing
        };
        assert_eq!(
            next_recovery_advance(&early),
            Err(RecoveryAdvanceError::CompletionTimeOutOfRange)
        );
        let future = RecoveryAdvanceFacts { payment_completed_at: Some(at(11)), ..missing };
        assert_eq!(
            next_recovery_advance(&future),
            Err(RecoveryAdvanceError::CompletionTimeOutOfRange)
        );
    }

    #[test]
    fn unpaid_reminder_respects_proof_limit_and_overflow() {
        let unproven = RecoveryAdvanceFacts { payment_sent_proven: false, ..facts() };
        assert_eq!(next_recovery_advance(&unproven), Ok(RecoveryAdvance::DeferPaymentReminder));

        let first = facts();
        assert_eq!(
            next_recovery_advance(&first),
            Ok(RecoveryAdvance::RemindPayment { reminder_count: 1 })
        );
        let exhausted = RecoveryAdvanceFacts { reminder_count: 3, ..facts() };
        assert_eq!(
            next_recovery_advance(&exhausted),
            Err(RecoveryAdvanceError::ReminderLimitExceeded)
        );
        // 计数必然小于上限才会自增，因此溢出分支只作为防御性失败关闭保留。
        let never_overflow = RecoveryAdvanceFacts {
            reminder_count: i32::MAX - 1,
            reminder_limit: i32::MAX,
            ..facts()
        };
        assert_eq!(
            next_recovery_advance(&never_overflow),
            Ok(RecoveryAdvance::RemindPayment { reminder_count: i32::MAX })
        );
    }

    #[test]
    fn unsupported_stage_fails_closed() {
        for stage in [
            RecoveryStage::PaymentOverdue,
            RecoveryStage::DeliveringEquipment,
            RecoveryStage::Completed,
        ] {
            let facts = RecoveryAdvanceFacts { stage, ..facts() };
            assert_eq!(next_recovery_advance(&facts), Err(RecoveryAdvanceError::UnsupportedStage));
        }
        // 异世界装备已交付但付款未到不能跳过付款。
        let isekai = RecoveryAdvanceFacts {
            world: SettlementWorld::Isekai,
            stage: RecoveryStage::DeliveringEquipment,
            ..facts()
        };
        assert_eq!(next_recovery_advance(&isekai), Err(RecoveryAdvanceError::UnsupportedStage));
    }
}
