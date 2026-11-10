//! 请求门等待与邮件长期操作锁的闭集。

/// 请求门等待结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitOutcome {
    Acquired,
    Error,
    Canceled,
}

impl WaitOutcome {
    pub const ALL: [Self; 3] = [Self::Acquired, Self::Error, Self::Canceled];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Acquired => "acquired",
            Self::Error => "error",
            Self::Canceled => "canceled",
        }
    }
}

/// 邮件长期操作锁等待结果；锁只记录是否获准。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailLockOutcome {
    Acquired,
    Canceled,
}

impl MailLockOutcome {
    pub const ALL: [Self; 2] = [Self::Acquired, Self::Canceled];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Acquired => "acquired",
            Self::Canceled => "canceled",
        }
    }
}

/// 邮件锁种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailLock {
    Exclusive,
    IncomingRead,
    IncomingWrite,
}

impl MailLock {
    pub const ALL: [Self; 3] = [Self::Exclusive, Self::IncomingRead, Self::IncomingWrite];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Exclusive => "exclusive",
            Self::IncomingRead => "incoming_read",
            Self::IncomingWrite => "incoming_write",
        }
    }
}
