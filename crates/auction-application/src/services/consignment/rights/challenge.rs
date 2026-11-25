//! 发码：按权利定位符请求挑战码。

use std::net::IpAddr;

use auction_domain::consignment::rights::{LocatorDigest, locator_digest, parse_locator};
use uuid::Uuid;

use super::{RightOperation, RightsService, parse_command_id, rights_encoding};
use crate::ports::consignment::{
    BrowserBinding, ChallengeIssued, ChallengeRightCommand, RightsStoreError,
};
use crate::services::consignment::deposit::encoding::CommandEncoding;

/// 发码输入。
#[derive(Clone)]
pub struct ChallengeRightInput {
    /// 浏览器 UUIDv4。
    pub command_id: Uuid,
    /// 浏览器绑定。
    pub browser: BrowserBinding,
    /// 可信客户端 IP。
    pub client_ip: IpAddr,
    /// 定位符文本；`Debug` 打码，也不入摘要原文。
    pub right_locator: String,
}

impl std::fmt::Debug for ChallengeRightInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ChallengeRightInput")
            .field("command_id", &self.command_id)
            .field("right_locator", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl RightsService {
    /// 发码；定位符形状无效为 `InvalidRequest`，不区分是否真实存在。
    pub async fn challenge(
        &self,
        input: ChallengeRightInput,
    ) -> Result<ChallengeIssued, RightsStoreError> {
        let command_id = parse_command_id(input.command_id)?;
        let Ok(locator) = parse_locator(&input.right_locator) else {
            return Err(RightsStoreError::InvalidRequest);
        };
        let locator_digest = locator_digest(&locator);
        self.store()
            .challenge(ChallengeRightCommand {
                command_id,
                browser: input.browser,
                client_ip: input.client_ip,
                locator_digest,
                request_hash: canonical_challenge(input.browser, &locator_digest).request_hash(),
            })
            .await
    }
}

/// 发码的 v2 规范编码：actor 与目标都是原浏览器，输入只有定位符摘要。
pub(super) fn canonical_challenge(
    browser: BrowserBinding,
    locator_digest: &LocatorDigest,
) -> CommandEncoding {
    let mut encoding = rights_encoding(RightOperation::Challenge, browser);
    encoding.bytes(&locator_digest.as_bytes());
    encoding
}
