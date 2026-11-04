//! 只从类型、闭集变体和非敏感控制标志投影，绝不格式化原错误。

use auction_application::diagnostics::{DiagnosticContext, DiagnosticError, SafeErrorKind};
use auction_domain::SettlementWorld;

use super::ErrorProjection;
use crate::clients::{
    ehentai::{EhentaiError, EhentaiErrorCategory},
    mooglemail::{MoogleMailError, MoogleMailErrorCategory, MoogleMailWorld},
};

pub(crate) trait IntegrationDiagnostic {
    fn integration_diagnostic(&self) -> ErrorProjection;
}

impl<T: IntegrationDiagnostic + ?Sized> IntegrationDiagnostic for &T {
    fn integration_diagnostic(&self) -> ErrorProjection {
        T::integration_diagnostic(*self)
    }
}

macro_rules! fixed_projection {
    ($kind:ident; $($error:ty),+ $(,)?) => {
        $(impl IntegrationDiagnostic for $error {
            fn integration_diagnostic(&self) -> ErrorProjection {
                ErrorProjection::new(SafeErrorKind::$kind)
            }
        })+
    };
}

fixed_projection!(TIMEOUT; tokio::time::error::Elapsed);
fixed_projection!(TASK_JOIN; tokio::task::JoinError);
fixed_projection!(IO_ERROR; std::io::Error, std::fs::TryLockError);
fixed_projection!(CRYPTOGRAPHIC_ERROR; ring::error::Unspecified, ring::error::KeyRejected);
fixed_projection!(SERIALIZATION_FAILED; serde_json::Error);
fixed_projection!(RESPONSE_PARSE_ERROR;
    std::str::Utf8Error, std::string::FromUtf8Error, std::num::ParseIntError,
    std::num::ParseFloatError, std::num::TryFromIntError, base64::DecodeError, chrono::ParseError,
);
fixed_projection!(REQUEST_ENCODING_ERROR;
    reqwest::header::InvalidHeaderValue, reqwest::header::ToStrError,
    std::array::TryFromSliceError,
);
fixed_projection!(CONFIGURATION_INVALID; url::ParseError);
fixed_projection!(DOMAIN_REJECTED; uuid::Error, (), str);
fixed_projection!(CAPABILITY_UNAVAILABLE; tokio::sync::TryAcquireError);
fixed_projection!(DEPENDENCY_UNAVAILABLE; tokio::sync::watch::error::RecvError);
fixed_projection!(ADMIN_CREDENTIAL_ERROR; auction_application::AdminCredentialError);
fixed_projection!(ADMIN_FORUM_PREVIEW_ERROR; auction_application::AdminForumPreviewError);
fixed_projection!(DOMAIN_REJECTED; auction_domain::EquipmentKeyError);
fixed_projection!(REQUEST_ENCODING_ERROR; auction_domain::EquipmentKeyEnvelopeError);

macro_rules! application_projection {
    ($($error:ty),+ $(,)?) => {
        $(impl IntegrationDiagnostic for $error {
            fn integration_diagnostic(&self) -> ErrorProjection {
                ErrorProjection::new(self.diagnostic_kind())
            }
        })+
    };
}

application_projection!(
    auction_application::ApplicationError,
    auction_application::SentBaselineReason,
    auction_application::EquipmentSourceDetailCode,
    auction_application::InternalEvidenceError,
    auction_application::FundingError,
    auction_application::MailDiagnosticError,
    auction_application::RepositoryError,
    auction_application::AdminIntakeError,
    auction_application::CompetitionTransactionError,
    auction_domain::DomainError,
);

impl<T> IntegrationDiagnostic for std::sync::PoisonError<T> {
    fn integration_diagnostic(&self) -> ErrorProjection {
        ErrorProjection::new(SafeErrorKind::REPOSITORY_FAILED)
    }
}

impl<T> IntegrationDiagnostic for std::sync::Arc<T> {
    fn integration_diagnostic(&self) -> ErrorProjection {
        ErrorProjection::new(SafeErrorKind::DEPENDENCY_MISMATCH)
    }
}

impl IntegrationDiagnostic for scraper::error::SelectorErrorKind {
    fn integration_diagnostic(&self) -> ErrorProjection {
        ErrorProjection::new(SafeErrorKind::CONFIGURATION_INVALID)
    }
}

impl IntegrationDiagnostic for reqwest::Error {
    fn integration_diagnostic(&self) -> ErrorProjection {
        let kind = if self.is_timeout() {
            SafeErrorKind::TIMEOUT
        } else if self.is_builder() {
            SafeErrorKind::CONFIGURATION_INVALID
        } else if self.is_body() || self.is_decode() {
            SafeErrorKind::REMOTE_BODY_ERROR
        } else {
            SafeErrorKind::REMOTE_TRANSPORT_ERROR
        };
        ErrorProjection::new(kind)
    }
}

impl IntegrationDiagnostic for MoogleMailError {
    fn integration_diagnostic(&self) -> ErrorProjection {
        let kind = mooglemail_kind(self.category);
        ErrorProjection {
            kind,
            context: DiagnosticContext {
                world: Some(match self.world {
                    MoogleMailWorld::Main => SettlementWorld::Main,
                    MoogleMailWorld::Isekai => SettlementWorld::Isekai,
                }),
                write_started: Some(self.write_started()),
                evidence_id: self.evidence_id,
                ..Default::default()
            },
        }
    }
}

impl IntegrationDiagnostic for EhentaiError {
    fn integration_diagnostic(&self) -> ErrorProjection {
        use EhentaiErrorCategory as Category;
        let kind = match self.category() {
            Category::Configuration => SafeErrorKind::CONFIGURATION_INVALID,
            Category::CredentialUnavailable => SafeErrorKind::CREDENTIAL_UNAVAILABLE,
            Category::SessionExpired => SafeErrorKind::REMOTE_SESSION_EXPIRED,
            Category::RateLimited => SafeErrorKind::REMOTE_RATE_LIMITED,
            Category::RemoteTemporary => SafeErrorKind::REMOTE_RETRYABLE,
            Category::Cloudflare => SafeErrorKind::REMOTE_CHALLENGE,
            Category::TopicLocked => SafeErrorKind::REMOTE_TOPIC_LOCKED,
            Category::PermissionDenied => SafeErrorKind::REMOTE_PERMISSION_DENIED,
            Category::ResponseTooLarge => SafeErrorKind::REMOTE_RESPONSE_TOO_LARGE,
            Category::ProtocolDrift => SafeErrorKind::REMOTE_PROTOCOL_ERROR,
            Category::WriteUnknown => SafeErrorKind::REMOTE_WRITE_UNKNOWN,
        };
        ErrorProjection {
            kind,
            context: DiagnosticContext {
                write_started: Some(self.write_started()),
                ..Default::default()
            },
        }
    }
}

impl IntegrationDiagnostic for auction_application::ForumPmError {
    fn integration_diagnostic(&self) -> ErrorProjection {
        use auction_application::ForumPmError;
        ErrorProjection::new(match self {
            ForumPmError::Unavailable => SafeErrorKind::DEPENDENCY_UNAVAILABLE,
            ForumPmError::Authentication => SafeErrorKind::REMOTE_AUTHENTICATION_ERROR,
            ForumPmError::RateLimited => SafeErrorKind::REMOTE_RATE_LIMITED,
            ForumPmError::InvalidRecipient => SafeErrorKind::REMOTE_RECIPIENT_INVALID,
            ForumPmError::Protocol => SafeErrorKind::REMOTE_PROTOCOL_ERROR,
            ForumPmError::OutcomeUnknown => SafeErrorKind::REMOTE_WRITE_UNKNOWN,
        })
    }
}

impl IntegrationDiagnostic for crate::EquipmentKeySealerError {
    fn integration_diagnostic(&self) -> ErrorProjection {
        use crate::EquipmentKeySealerError as Error;
        ErrorProjection::new(match self {
            Error::InvalidKeyId | Error::InvalidDataKey | Error::InvalidAssociatedData => {
                SafeErrorKind::CONFIGURATION_INVALID
            }
            Error::Unconfigured => SafeErrorKind::CAPABILITY_UNAVAILABLE,
            Error::RandomSource | Error::Encryption => SafeErrorKind::CRYPTOGRAPHIC_ERROR,
        })
    }
}

impl IntegrationDiagnostic for crate::EquipmentKeyOpenerError {
    fn integration_diagnostic(&self) -> ErrorProjection {
        use crate::EquipmentKeyOpenerError as Error;
        ErrorProjection::new(match self {
            Error::InvalidEquipmentId | Error::InvalidPlaintext => SafeErrorKind::DOMAIN_REJECTED,
            Error::InvalidEnvelope => SafeErrorKind::REQUEST_ENCODING_ERROR,
            Error::KeyMismatch => SafeErrorKind::DEPENDENCY_MISMATCH,
            Error::Decryption => SafeErrorKind::CRYPTOGRAPHIC_ERROR,
            Error::FingerprintMismatch => SafeErrorKind::EVIDENCE_UNPROVEN,
            Error::InvalidUtf8 => SafeErrorKind::RESPONSE_PARSE_ERROR,
        })
    }
}

pub(crate) fn mooglemail_kind(category: MoogleMailErrorCategory) -> SafeErrorKind {
    use MoogleMailErrorCategory as Category;
    match category {
        Category::Configuration => SafeErrorKind::CONFIGURATION_INVALID,
        Category::CredentialUnavailable => SafeErrorKind::CREDENTIAL_UNAVAILABLE,
        Category::SessionExpired => SafeErrorKind::REMOTE_SESSION_EXPIRED,
        Category::AccountBusy => SafeErrorKind::REMOTE_ACCOUNT_BUSY,
        Category::RateLimited => SafeErrorKind::REMOTE_RATE_LIMITED,
        Category::RemoteTemporary => SafeErrorKind::REMOTE_RETRYABLE,
        Category::SentScanPageLimit => SafeErrorKind::SENT_SCAN_PAGE_LIMIT,
        Category::SentScanTimeout => SafeErrorKind::SENT_SCAN_TIMEOUT,
        Category::PersistenceUnavailable => SafeErrorKind::PERSISTENCE_UNAVAILABLE,
        Category::ResponseTooLarge => SafeErrorKind::REMOTE_RESPONSE_TOO_LARGE,
        Category::ProtocolDrift => SafeErrorKind::REMOTE_PROTOCOL_ERROR,
        Category::IdentityMismatch => SafeErrorKind::REMOTE_IDENTITY_MISMATCH,
        Category::IdentityUnproven => SafeErrorKind::REMOTE_IDENTITY_UNPROVEN,
        Category::NotObserved => SafeErrorKind::EVIDENCE_NOT_OBSERVED,
        Category::WriteUnknown => SafeErrorKind::REMOTE_WRITE_UNKNOWN,
    }
}
