//! 受限权利用例：输入校验、规范摘要与对 `RightsStore` 的编排。
//!
//! 业务规则在 `auction-domain` 的 `rights`，持久化在存储端口；本层只做形状校验、
//! 命令的 v2 规范编码与秘密文本的解析，不保留任何秘密明文。

mod challenge;
mod claim;
mod locate;
mod verify;

use std::sync::Arc;

use auction_domain::CommandId;
use uuid::Uuid;

pub use challenge::ChallengeRightInput;
pub use claim::ClaimRightInput;
pub use locate::LocateRightInput;
pub use verify::VerifyRightInput;

use crate::ports::consignment::{
    BrowserBinding, DepositActor, RightChallengeCrypto, RightsStore, RightsStoreError,
};
use crate::services::consignment::deposit::encoding::{CommandEncoding, CommandTarget};

/// 受限权利用例服务。
pub struct RightsService {
    store: Arc<dyn RightsStore>,
    crypto: Option<Arc<dyn RightChallengeCrypto>>,
}

impl RightsService {
    /// 以存储端口与密码端口建立服务；密码端口为空时核验失败关闭。
    pub fn new(store: Arc<dyn RightsStore>, crypto: Option<Arc<dyn RightChallengeCrypto>>) -> Self {
        Self { store, crypto }
    }

    pub(super) fn crypto(&self) -> Result<&dyn RightChallengeCrypto, RightsStoreError> {
        self.crypto.as_deref().ok_or(RightsStoreError::Unavailable)
    }

    pub(super) fn store(&self) -> &dyn RightsStore {
        self.store.as_ref()
    }
}

/// 命令 ID 必须是 UUIDv4；其他版本失败关闭。
pub(super) fn parse_command_id(command_id: Uuid) -> Result<CommandId, RightsStoreError> {
    if command_id.get_version_num() != 4 {
        return Err(RightsStoreError::InvalidRequest);
    }
    Ok(CommandId::from_uuid(command_id))
}

/// 权利命令在 v2 账本里的操作名；不进入拍卖 `DepositOperation` 闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RightOperation {
    /// 找回。
    Locate,
    /// 发码。
    Challenge,
    /// 核验。
    Verify,
    /// 续办。
    Claim,
}

impl RightOperation {
    /// 稳定 wire 与账本码。
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Locate => "locate_right",
            Self::Challenge => "challenge_right",
            Self::Verify => "verify_right",
            Self::Claim => "claim_right",
        }
    }
}

/// 浏览器发起的三个命令（找回、发码、核验）共用的编码头：actor 与目标都是原浏览器。
pub(super) fn rights_encoding(
    operation: RightOperation,
    browser: BrowserBinding,
) -> CommandEncoding {
    CommandEncoding::with_operation(
        operation.as_str(),
        &DepositActor::Browser { binding_digest: browser.0 },
        CommandTarget::Browser(browser.0),
    )
}

#[cfg(test)]
mod tests;
