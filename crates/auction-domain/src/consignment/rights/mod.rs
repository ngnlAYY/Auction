//! 受限权利：定位符、挑战码、会话令牌、频率预算与可执行动作的领域规则。
//!
//! 权利人没有普通登录会话，凭随机定位符、一次性挑战码与短会话令牌办理既有权利。三类秘密
//! 都只在持有人与传输边界出现：`Debug` 打码、不实现序列化、错误只带安全分类。

mod actions;
mod budget;
mod challenge;
mod encoding;
mod errors;
mod locator;
mod right_id;
mod session;

pub use actions::{
    RightAction, RightActionFacts, RightKind, RightTarget, RightView, allowed_actions,
};
pub use budget::{
    BROWSER_BUDGET_LIMIT, BUDGET_WINDOW, IP_BUDGET_LIMIT, budget_allows, request_within_budget,
};
pub use challenge::{
    CHALLENGE_CODE_ENTROPY_BYTES, CHALLENGE_CODE_TEXT_LEN, CHALLENGE_TTL, ChallengeCode,
    ChallengeCodeText, ChallengeState, MAX_FAILED_ATTEMPTS, NOTICE_INTERVAL, format_challenge_code,
    notice_allowed, parse_challenge_code,
};
pub use errors::{RightsError, TextFault};
pub use locator::{
    LOCATOR_PREFIX, LOCATOR_RANDOM_BYTES, LOCATOR_TEXT_LEN, LocatorDigest, LocatorText,
    RightLocator, format_locator, locator_digest, parse_locator,
};
pub use right_id::{RightRole, right_id};
pub use session::{
    RightSessionToken, SESSION_TOKEN_PREFIX, SESSION_TTL, SessionDigest, SessionTokenText,
    format_session_token, parse_session_token, rotated_session_expiry, session_digest,
};
