//! 受限权利的窄端口：存储、挑战码密码与通知类型。

mod crypto;
mod notice;
mod notice_work;
mod store;

pub use crypto::{ChallengeAad, RightChallengeCrypto, RightCryptoError, SealedChallengeSecret};
pub use notice::{RightNoticeDisposition, RightNoticeKind, RightNoticeRequest};
pub use notice_work::{
    ClaimedRightNotice, RightNoticeFuture, RightNoticeLease, RightNoticeSecret,
    RightNoticeStoreError, RightNoticeWorkStore,
};
pub use store::{
    BrowserBinding, ChallengeIssued, ChallengeRightCommand, ClaimDisposition, ClaimOutcome,
    ClaimRightCommand, GameIdentity, LocateRightCommand, RightReceipt, RightVerified, RightsFuture,
    RightsStore, RightsStoreError, VerifyRightCommand,
};

#[cfg(test)]
pub(crate) mod test_support;
