use thiserror::Error;

/// PostgreSQL 与队列消费路径允许使用的事务隔离级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionIsolation {
    Serializable,
    ReadCommitted,
}

/// 应用层传给存储适配器的事务策略。
/// 竞价和关键生命周期写入使用 Serializable，最多尝试三次；队列领取等已经由显式行锁
/// 保护的路径才使用 Read Committed。这个类型不允许适配器把两种语义悄悄合并。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransactionPolicy {
    pub isolation: TransactionIsolation,
    pub max_attempts: u8,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum TransactionPolicyError {
    #[error("事务至少需要一次尝试")]
    NoAttempts,
    #[error("Serializable 事务最多允许三次尝试")]
    SerializableAttemptsExceeded,
    #[error("Read Committed 路径不允许隐式重试")]
    ReadCommittedRetry,
}

impl TransactionPolicy {
    /// 竞价写入和关键生命周期写入的固定策略。
    pub const fn serializable() -> Self {
        Self { isolation: TransactionIsolation::Serializable, max_attempts: 3 }
    }

    /// 队列领取等通过显式锁保护的短事务策略。
    pub const fn read_committed() -> Self {
        Self { isolation: TransactionIsolation::ReadCommitted, max_attempts: 1 }
    }

    /// 从配置或适配器输入构造并校验事务策略。
    pub const fn new(
        isolation: TransactionIsolation,
        max_attempts: u8,
    ) -> Result<Self, TransactionPolicyError> {
        if max_attempts == 0 {
            return Err(TransactionPolicyError::NoAttempts);
        }
        match isolation {
            TransactionIsolation::Serializable if max_attempts > 3 => {
                Err(TransactionPolicyError::SerializableAttemptsExceeded)
            }
            TransactionIsolation::ReadCommitted if max_attempts != 1 => {
                Err(TransactionPolicyError::ReadCommittedRetry)
            }
            _ => Ok(Self { isolation, max_attempts }),
        }
    }
}
