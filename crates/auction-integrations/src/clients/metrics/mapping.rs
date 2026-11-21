//! 外部客户端与指标枚举之间的闭集映射。
use auction_application::external_metrics::{
    ForumOperation, ForumRequestCategory, ForumWorld, MailOperation, MailRequestCategory, MailWorld,
};

use super::super::ehentai::{EhentaiError, EhentaiErrorCategory, EhentaiOperation};
use super::super::mooglemail::{
    MoogleMailError, MoogleMailErrorCategory, MoogleMailOperation, MoogleMailWorld,
};

pub(crate) fn mail_world(world: MoogleMailWorld) -> MailWorld {
    match world {
        MoogleMailWorld::Main => MailWorld::Main,
        MoogleMailWorld::Isekai => MailWorld::Isekai,
    }
}

pub(crate) fn mail_operation(operation: MoogleMailOperation) -> MailOperation {
    match operation {
        MoogleMailOperation::Inbox => MailOperation::Inbox,
        MoogleMailOperation::ReadMail => MailOperation::ReadMail,
        MoogleMailOperation::Detail => MailOperation::Detail,
        MoogleMailOperation::Sent => MailOperation::Sent,
        MoogleMailOperation::SentDetail => MailOperation::SentDetail,
        MoogleMailOperation::Balance => MailOperation::Balance,
        MoogleMailOperation::Equipment => MailOperation::Equipment,
        MoogleMailOperation::Identity => MailOperation::Identity,
        MoogleMailOperation::Inventory => MailOperation::Inventory,
        MoogleMailOperation::InventoryOrganize => MailOperation::InventoryOrganize,
        MoogleMailOperation::EquipmentUnstore => MailOperation::EquipmentUnstore,
        MoogleMailOperation::EquipmentUnlock => MailOperation::EquipmentUnlock,
        MoogleMailOperation::Compose => MailOperation::Compose,
        MoogleMailOperation::AttachAdd => MailOperation::AttachAdd,
        MoogleMailOperation::AttachCod => MailOperation::AttachCod,
        MoogleMailOperation::Send => MailOperation::Send,
        MoogleMailOperation::ReturnMessage => MailOperation::ReturnMessage,
        MoogleMailOperation::Take => MailOperation::AttachRemove,
        MoogleMailOperation::Discard => MailOperation::Discard,
        MoogleMailOperation::CredentialCheck => MailOperation::CredentialCheck,
    }
}

pub(crate) fn mail_category(error: MoogleMailErrorCategory) -> MailRequestCategory {
    match error {
        MoogleMailErrorCategory::Configuration => MailRequestCategory::Configuration,
        MoogleMailErrorCategory::CredentialUnavailable => {
            MailRequestCategory::CredentialUnavailable
        }
        MoogleMailErrorCategory::SessionExpired => MailRequestCategory::SessionExpired,
        MoogleMailErrorCategory::AccountBusy => MailRequestCategory::AccountBusy,
        MoogleMailErrorCategory::RateLimited => MailRequestCategory::RateLimited,
        MoogleMailErrorCategory::RemoteTemporary => MailRequestCategory::RemoteTemporary,
        MoogleMailErrorCategory::SentScanPageLimit => MailRequestCategory::SentScanPageLimit,
        MoogleMailErrorCategory::SentScanTimeout => MailRequestCategory::SentScanTimeout,
        MoogleMailErrorCategory::PersistenceUnavailable => {
            MailRequestCategory::PersistenceUnavailable
        }
        MoogleMailErrorCategory::ResponseTooLarge => MailRequestCategory::ResponseTooLarge,
        MoogleMailErrorCategory::ProtocolDrift => MailRequestCategory::ProtocolDrift,
        MoogleMailErrorCategory::IdentityMismatch => MailRequestCategory::IdentityMismatch,
        MoogleMailErrorCategory::IdentityUnproven => MailRequestCategory::IdentityUnproven,
        MoogleMailErrorCategory::NotObserved => MailRequestCategory::NotObserved,
        MoogleMailErrorCategory::WriteUnknown => MailRequestCategory::WriteUnknown,
    }
}

pub(crate) fn mail_result(result: &Result<Vec<u8>, MoogleMailError>) -> MailRequestCategory {
    match result {
        Ok(_) => MailRequestCategory::Ok,
        Err(error) => mail_category(error.category),
    }
}

pub(crate) fn forum_category(error: EhentaiErrorCategory) -> ForumRequestCategory {
    match error {
        EhentaiErrorCategory::Configuration => ForumRequestCategory::Configuration,
        EhentaiErrorCategory::CredentialUnavailable => ForumRequestCategory::CredentialUnavailable,
        EhentaiErrorCategory::SessionExpired => ForumRequestCategory::SessionExpired,
        EhentaiErrorCategory::RateLimited => ForumRequestCategory::RateLimited,
        EhentaiErrorCategory::RemoteTemporary => ForumRequestCategory::RemoteTemporary,
        EhentaiErrorCategory::Cloudflare => ForumRequestCategory::Cloudflare,
        EhentaiErrorCategory::TopicLocked => ForumRequestCategory::TopicLocked,
        EhentaiErrorCategory::PermissionDenied => ForumRequestCategory::PermissionDenied,
        EhentaiErrorCategory::ResponseTooLarge => ForumRequestCategory::ResponseTooLarge,
        EhentaiErrorCategory::ProtocolDrift | EhentaiErrorCategory::TopicPageLimit => {
            ForumRequestCategory::ProtocolDrift
        }
        EhentaiErrorCategory::WriteUnknown => ForumRequestCategory::WriteUnknown,
    }
}

pub(crate) fn forum_result<T>(result: &Result<T, EhentaiError>) -> ForumRequestCategory {
    match result {
        Ok(_) => ForumRequestCategory::Ok,
        Err(error) => forum_category(error.category()),
    }
}

pub(crate) fn forum_world(world: auction_domain::SettlementWorld) -> ForumWorld {
    match world {
        auction_domain::SettlementWorld::Main => ForumWorld::Main,
        auction_domain::SettlementWorld::Isekai => ForumWorld::Isekai,
    }
}

pub(crate) fn forum_operation(operation: EhentaiOperation) -> ForumOperation {
    match operation {
        EhentaiOperation::CheckSession => ForumOperation::CheckSession,
        EhentaiOperation::CreateTopic => ForumOperation::CreateTopic,
        EhentaiOperation::ReconcileTopic => ForumOperation::ReconcileTopic,
        EhentaiOperation::FetchTopic => ForumOperation::FetchTopic,
        EhentaiOperation::ReplyTopic => ForumOperation::ReplyTopic,
        EhentaiOperation::EditPost => ForumOperation::EditPost,
        EhentaiOperation::ReadPost => ForumOperation::ReadPost,
    }
}

pub(crate) fn forum_pm_category(
    error: auction_application::forum::ForumPmError,
) -> auction_application::external_metrics::ForumPmCategory {
    use auction_application::external_metrics::ForumPmCategory;
    use auction_application::forum::ForumPmError;
    match error {
        ForumPmError::Unavailable => ForumPmCategory::Unavailable,
        ForumPmError::Authentication => ForumPmCategory::Authentication,
        ForumPmError::RateLimited => ForumPmCategory::RateLimited,
        ForumPmError::InvalidRecipient => ForumPmCategory::InvalidRecipient,
        ForumPmError::Protocol => ForumPmCategory::Protocol,
        ForumPmError::OutcomeUnknown => ForumPmCategory::OutcomeUnknown,
    }
}

pub(crate) fn forum_pm_request_result<T>(
    result: &Result<T, auction_application::forum::ForumPmError>,
) -> ForumRequestCategory {
    use auction_application::forum::ForumPmError;
    match result {
        Ok(_) => ForumRequestCategory::Ok,
        Err(ForumPmError::Unavailable) => ForumRequestCategory::CredentialUnavailable,
        Err(ForumPmError::Authentication) => ForumRequestCategory::SessionExpired,
        Err(ForumPmError::RateLimited) => ForumRequestCategory::RateLimited,
        Err(ForumPmError::InvalidRecipient) => ForumRequestCategory::Configuration,
        Err(ForumPmError::Protocol) => ForumRequestCategory::ProtocolDrift,
        Err(ForumPmError::OutcomeUnknown) => ForumRequestCategory::WriteUnknown,
    }
}
