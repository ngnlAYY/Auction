//! 各错误类型的诊断分类投影；只读取类型化变体。

use super::SafeErrorKind;

/// 只从类型化变体投影，不格式化错误；记录 helper 只接收投影结果。
pub trait DiagnosticError {
    fn diagnostic_kind(&self) -> SafeErrorKind;
}

impl DiagnosticError for crate::errors::ApplicationError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::from_label(self.safe_kind())
    }
}

impl DiagnosticError for crate::ports::settlement::InternalEvidenceError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::from_label(self.safe_kind())
    }
}

impl DiagnosticError for crate::ports::settlement::FundingError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::from_label(self.safe_kind())
    }
}

impl DiagnosticError for serde_json::Error {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::SERIALIZATION_FAILED
    }
}

impl DiagnosticError for auction_domain::DomainError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::DOMAIN_REJECTED
    }
}

impl DiagnosticError for std::num::TryFromIntError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::DOMAIN_REJECTED
    }
}

impl DiagnosticError for std::io::Error {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::REPOSITORY_FAILED
    }
}

impl DiagnosticError for chrono::OutOfRangeError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::CONFIGURATION_INVALID
    }
}

impl DiagnosticError for chrono_tz::ParseError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::DOMAIN_REJECTED
    }
}

impl DiagnosticError for url::ParseError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::CONFIGURATION_INVALID
    }
}

impl DiagnosticError for crate::errors::SessionError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::SESSION_ERROR
    }
}

impl DiagnosticError for crate::ports::public::LotWatchError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        match self {
            Self::Transaction(error) => error.diagnostic_kind(),
            Self::Unavailable => SafeErrorKind::DEPENDENCY_UNAVAILABLE,
            Self::Conflict => SafeErrorKind::REPOSITORY_CONFLICT,
            Self::Invalid | Self::NotFound | Self::TooMany | Self::PmUnavailable => {
                SafeErrorKind::DOMAIN_REJECTED
            }
        }
    }
}

impl DiagnosticError for crate::ports::forum::ForumPmError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        match self {
            Self::Unavailable => SafeErrorKind::DEPENDENCY_UNAVAILABLE,
            Self::RateLimited => SafeErrorKind::REMOTE_RETRYABLE,
            Self::OutcomeUnknown => SafeErrorKind::REMOTE_WRITE_STARTED,
            Self::Authentication | Self::InvalidRecipient | Self::Protocol => {
                SafeErrorKind::REMOTE_REJECTED
            }
        }
    }
}

impl DiagnosticError for crate::ports::mail::MailDiagnosticError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::REPOSITORY_FAILED
    }
}

impl DiagnosticError for crate::ports::core::RepositoryError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::REPOSITORY_FAILED
    }
}

impl DiagnosticError for crate::ports::admin::AdminRuntimeConfigError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::ADMIN_RUNTIME_ERROR
    }
}

impl DiagnosticError for crate::ports::admin::AdminRuntimeFileError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::RUNTIME_FILE_ERROR
    }
}

impl DiagnosticError for crate::ports::admin::AdminRuntimeCommandError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::RUNTIME_COMMAND_ERROR
    }
}

impl<T> DiagnosticError for std::sync::PoisonError<T> {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::REPOSITORY_FAILED
    }
}

impl DiagnosticError for chrono::ParseError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::DOMAIN_REJECTED
    }
}

impl DiagnosticError for crate::ports::admin::AdminIntakeError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::ADMIN_INTAKE_ERROR
    }
}

impl DiagnosticError for crate::ports::core::CompetitionTransactionError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        use crate::ports::core::CompetitionTransactionError as Error;
        match self {
            Error::Failed(_) => SafeErrorKind::REPOSITORY_FAILED,
            Error::SerializationConflict => SafeErrorKind::REPOSITORY_SERIALIZATION_CONFLICT,
            Error::QueueWaitTimeout => SafeErrorKind::COMPETITION_QUEUE_TIMEOUT,
            Error::Conflict => SafeErrorKind::REPOSITORY_CONFLICT,
            _ => SafeErrorKind::COMPETITION_REJECTED,
        }
    }
}

impl DiagnosticError for crate::ports::operations::OperationsAlertError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        SafeErrorKind::OPERATIONS_ALERT_ERROR
    }
}

impl DiagnosticError for crate::ports::runtime_switches::RuntimeSwitchError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        match self {
            // 只收紧策略的拒绝属于输入被拒，与无效命令同一类别。
            Self::Invalid | Self::UnsafeRelaxation => SafeErrorKind::DOMAIN_REJECTED,
            Self::Conflict => SafeErrorKind::REPOSITORY_CONFLICT,
            Self::Corrupt => SafeErrorKind::RUNTIME_SWITCH_CORRUPT,
            Self::Unavailable => SafeErrorKind::DEPENDENCY_UNAVAILABLE,
            Self::OutcomeUnknown => SafeErrorKind::RUNTIME_SWITCH_OUTCOME_UNKNOWN,
        }
    }
}

impl DiagnosticError for crate::ports::mail_templates::MailTemplateStoreError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        match self {
            Self::Invalid | Self::UnknownTemplate | Self::InvalidBody(_) => {
                SafeErrorKind::DOMAIN_REJECTED
            }
            Self::Conflict => SafeErrorKind::REPOSITORY_CONFLICT,
            Self::Corrupt => SafeErrorKind::DEPENDENCY_MISMATCH,
            Self::Unavailable => SafeErrorKind::DEPENDENCY_UNAVAILABLE,
            Self::OutcomeUnknown => SafeErrorKind::PERSISTENCE_UNAVAILABLE,
        }
    }
}

impl DiagnosticError for crate::ports::observability::sent_baseline::SentBaselineReason {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        match self {
            Self::InvalidMid => SafeErrorKind::SENT_BASELINE_INVALID_MID,
            Self::OrderViolation => SafeErrorKind::SENT_BASELINE_ORDER_VIOLATION,
            Self::PageDiscontinuity => SafeErrorKind::SENT_BASELINE_PAGE_DISCONTINUITY,
            Self::AnchorMissing => SafeErrorKind::SENT_BASELINE_ANCHOR_MISSING,
            Self::AnchorChanged => SafeErrorKind::SENT_BASELINE_ANCHOR_CHANGED,
            Self::PageLimit => SafeErrorKind::SENT_BASELINE_PAGE_LIMIT,
            Self::Timeout => SafeErrorKind::SENT_BASELINE_TIMEOUT,
            Self::ReadUnavailable => SafeErrorKind::SENT_BASELINE_READ_UNAVAILABLE,
            Self::CacheBoundaryDelta => SafeErrorKind::SENT_BASELINE_CACHE_BOUNDARY_DELTA,
            Self::BoundaryUnavailable => SafeErrorKind::SENT_BASELINE_BOUNDARY_UNAVAILABLE,
            Self::ScopeChanged => SafeErrorKind::SENT_BASELINE_SCOPE_CHANGED,
            Self::ResponseHintConflict => SafeErrorKind::SENT_BASELINE_RESPONSE_HINT_CONFLICT,
            Self::Canceled => SafeErrorKind::SENT_BASELINE_CANCELED,
        }
    }
}

impl DiagnosticError for crate::ports::mail::EquipmentSourceDetailCode {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        match self {
            Self::InvalidName => SafeErrorKind::EQUIPMENT_NAME_INVALID,
            Self::UnknownQuality => SafeErrorKind::EQUIPMENT_QUALITY_UNKNOWN,
            Self::UnknownKind => SafeErrorKind::EQUIPMENT_KIND_UNKNOWN,
            Self::UnknownCategory => SafeErrorKind::EQUIPMENT_CATEGORY_UNKNOWN,
            Self::CategoryMismatch => SafeErrorKind::EQUIPMENT_CATEGORY_MISMATCH,
            Self::InvalidLevel => SafeErrorKind::EQUIPMENT_LEVEL_INVALID,
            Self::PageStructure => SafeErrorKind::EQUIPMENT_PAGE_STRUCTURE_INVALID,
            Self::InvalidAttributes => SafeErrorKind::EQUIPMENT_ATTRIBUTES_INVALID,
            Self::CanonicalMismatch => SafeErrorKind::EQUIPMENT_CANONICAL_MISMATCH,
        }
    }
}
