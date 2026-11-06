//! Users/MoogleMail 命令结果与闭集模型。

use uuid::Uuid;

use crate::ports::mail::processing::MailProcessingError;

/// Users owner 接受的 MoogleMail 命令闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailUserCommandKind {
    Register,
    Rebind,
    Rekey,
}

impl MailUserCommandKind {
    /// 返回迁移表和通知使用的稳定命令类型。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Register => "register",
            Self::Rebind => "rebind",
            Self::Rekey => "rekey",
        }
    }
}

/// Users owner 返回给邮件处理器的结果闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailUserResultCode {
    Registered,
    Recovered,
    Blacklisted,
    IdentityConflict,
    Rebound,
    Rekeyed,
    Unchanged,
    InvalidKey,
    NotRegistered,
}

impl MailUserResultCode {
    /// 返回数据库约束和通知模板使用的稳定结果码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Registered => "registered",
            Self::Recovered => "recovered",
            Self::Blacklisted => "blacklisted",
            Self::IdentityConflict => "identity_conflict",
            Self::Rebound => "rebound",
            Self::Rekeyed => "rekeyed",
            Self::Unchanged => "unchanged",
            Self::InvalidKey => "invalid_key",
            Self::NotRegistered => "not_registered",
        }
    }
}

/// Users owner 命令结果；`user_id` 为空只允许 identity_conflict 或 invalid_key。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailUserResult {
    pub user_id: Option<Uuid>,
    pub code: MailUserResultCode,
}

impl MailUserResult {
    /// 校验结果码与用户标识的组合，避免损坏 owner 结果进入决定或通知。
    pub fn validate(self, kind: MailUserCommandKind) -> Result<Self, MailProcessingError> {
        let allows_missing_user = matches!(
            (kind, self.code),
            (MailUserCommandKind::Register, MailUserResultCode::IdentityConflict)
                | (MailUserCommandKind::Rebind, MailUserResultCode::InvalidKey)
                | (MailUserCommandKind::Rekey, MailUserResultCode::IdentityConflict)
                | (MailUserCommandKind::Rekey, MailUserResultCode::NotRegistered)
        );
        if self.user_id.is_none() != allows_missing_user {
            return Err(MailProcessingError::InvalidUserResult);
        }
        let valid_code = match kind {
            MailUserCommandKind::Register => matches!(
                self.code,
                MailUserResultCode::Registered
                    | MailUserResultCode::Recovered
                    | MailUserResultCode::Blacklisted
                    | MailUserResultCode::IdentityConflict
            ),
            MailUserCommandKind::Rebind => matches!(
                self.code,
                MailUserResultCode::Rebound
                    | MailUserResultCode::Blacklisted
                    | MailUserResultCode::IdentityConflict
                    | MailUserResultCode::Unchanged
                    | MailUserResultCode::InvalidKey
            ),
            MailUserCommandKind::Rekey => matches!(
                self.code,
                MailUserResultCode::Rekeyed
                    | MailUserResultCode::Blacklisted
                    | MailUserResultCode::IdentityConflict
                    | MailUserResultCode::NotRegistered
            ),
        };
        if !valid_code {
            return Err(MailProcessingError::InvalidUserResult);
        }
        Ok(self)
    }
}
