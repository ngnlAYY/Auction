//! 核验：用挑战码换取单权利会话。

use auction_domain::consignment::rights::parse_challenge_code;
use uuid::Uuid;

use super::{RightOperation, RightsService, parse_command_id, rights_encoding};
use crate::ports::consignment::{
    BrowserBinding, ChallengeAad, RightVerified, RightsStoreError, VerifyRightCommand,
};
use crate::services::consignment::deposit::encoding::CommandEncoding;

/// 核验输入。
#[derive(Clone)]
pub struct VerifyRightInput {
    /// 浏览器 UUIDv4。
    pub command_id: Uuid,
    /// 浏览器绑定。
    pub browser: BrowserBinding,
    /// 挑战 UUID。
    pub challenge_id: Uuid,
    /// 用户输入的挑战码。
    pub code: String,
}

impl std::fmt::Debug for VerifyRightInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VerifyRightInput")
            .field("command_id", &self.command_id)
            .field("challenge_id", &self.challenge_id)
            .field("code", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl RightsService {
    /// 核验；码形状无效与码错误同样返回 `ChallengeInvalid`，不产生额外区分。
    ///
    /// 保护的不变量：摘要绑定码的 MAC（密码端口，不是裸哈希），同一命令 ID 换码重放得到冲突而不是原会话；
    /// 密码端口未注入时失败关闭。
    pub async fn verify(&self, input: VerifyRightInput) -> Result<RightVerified, RightsStoreError> {
        let command_id = parse_command_id(input.command_id)?;
        let Ok(code) = parse_challenge_code(&input.code) else {
            return Err(RightsStoreError::ChallengeInvalid);
        };
        let mac = self.crypto()?.mac(request_aad(input.challenge_id), &code)?;
        self.store()
            .verify(VerifyRightCommand {
                command_id,
                browser: input.browser,
                challenge_id: input.challenge_id,
                code,
                request_hash: canonical_verify(input.browser, input.challenge_id, &mac)
                    .request_hash(),
            })
            .await
    }
}

/// 请求摘要用的 MAC 绑定：右侧与版本取固定标记，与挑战入库的 MAC 域分离。
fn request_aad(challenge_id: Uuid) -> ChallengeAad {
    ChallengeAad { challenge_id, right_id: None, identity_digest: [0; 32], right_version: u64::MAX }
}

/// 核验的 v2 规范编码：actor 与目标都是原浏览器，输入为挑战 UUID 与码的 MAC。
pub(super) fn canonical_verify(
    browser: BrowserBinding,
    challenge_id: Uuid,
    mac: &[u8; 32],
) -> CommandEncoding {
    let mut encoding = rights_encoding(RightOperation::Verify, browser);
    encoding.uuid(challenge_id);
    encoding.bytes(mac);
    encoding
}
