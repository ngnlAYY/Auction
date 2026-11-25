//! 续办与会话读取：以会话令牌办理当前权利。

use auction_domain::consignment::rights::{
    RightAction, RightView, parse_session_token, session_digest,
};
use uuid::Uuid;

use super::{RightOperation, RightsService, parse_command_id};
use crate::ports::consignment::{
    BrowserBinding, ClaimOutcome, ClaimRightCommand, DepositActor, RightReceipt, RightsStoreError,
};
use crate::services::consignment::deposit::encoding::{CommandEncoding, CommandTarget};

/// 续办输入。
#[derive(Clone)]
pub struct ClaimRightInput {
    /// 受限人 UUIDv4。
    pub command_id: Uuid,
    /// 浏览器绑定。
    pub browser: BrowserBinding,
    /// `hv_right` cookie 文本；`Debug` 打码。
    pub session_token: String,
    /// 已读到的权利版本。
    pub expected_version: u64,
    /// 动作 wire 码。
    pub action: String,
}

impl std::fmt::Debug for ClaimRightInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ClaimRightInput")
            .field("command_id", &self.command_id)
            .field("session_token", &"[REDACTED]")
            .field("expected_version", &self.expected_version)
            .field("action", &self.action)
            .finish_non_exhaustive()
    }
}

impl RightsService {
    /// 续办；缺少或格式错误的令牌、无有效会话一律 `Unauthenticated`，未知动作码为 `ActionNotAllowed`。
    ///
    /// 保护的不变量：会话只用于认证，不是请求内容；摘要绑定会话对应的 `right_id`、版本与动作，
    /// 换令牌或轮换令牌后用同一命令 ID 重试得到原结果而不是冲突。
    pub async fn claim(&self, input: ClaimRightInput) -> Result<ClaimOutcome, RightsStoreError> {
        let command_id = parse_command_id(input.command_id)?;
        let Ok(token) = parse_session_token(&input.session_token) else {
            return Err(RightsStoreError::Unauthenticated);
        };
        let session = session_digest(&token);
        let action = RightAction::parse(&input.action).ok_or(RightsStoreError::ActionNotAllowed)?;
        let view = self
            .store()
            .current(session, input.browser)
            .await?
            .ok_or(RightsStoreError::Unauthenticated)?;
        self.store()
            .claim(ClaimRightCommand {
                command_id,
                right_id: view.right_id,
                session,
                browser: input.browser,
                expected_version: input.expected_version,
                action,
                request_hash: canonical_claim(view.right_id, input.expected_version, action)
                    .request_hash(),
            })
            .await
    }

    /// 读取当前权利；无会话为 `None`（HTTP 401）。
    pub async fn current(
        &self,
        session_token: &str,
        browser: BrowserBinding,
    ) -> Result<Option<RightView>, RightsStoreError> {
        let Ok(token) = parse_session_token(session_token) else {
            return Ok(None);
        };
        self.store().current(session_digest(&token), browser).await
    }

    /// 查本权利下的命令回执。
    pub async fn command_receipt(
        &self,
        session_token: &str,
        browser: BrowserBinding,
        command_id: Uuid,
    ) -> Result<Option<RightReceipt>, RightsStoreError> {
        let command_id = parse_command_id(command_id)?;
        let Ok(token) = parse_session_token(session_token) else {
            return Ok(None);
        };
        self.store().command_receipt(session_digest(&token), browser, command_id).await
    }
}

/// 续办的 v2 规范编码：actor 与目标都是权利，不含会话令牌与浏览器。
pub(super) fn canonical_claim(
    right_id: Uuid,
    expected_version: u64,
    action: RightAction,
) -> CommandEncoding {
    let mut encoding = CommandEncoding::with_operation(
        RightOperation::Claim.as_str(),
        &DepositActor::RightHolder { right_id },
        CommandTarget::Right(right_id),
    );
    encoding.u64(expected_version);
    encoding.text(action.as_str());
    encoding
}
