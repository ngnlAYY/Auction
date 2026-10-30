//! 邮件凭据的预期名称与瞬时远端身份证明；配置声明不构成证明。

use auction_domain::SettlementWorld;
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// 身份证明的固定语义来源；联合来源由 HV 认证 UID 与论坛目标资料绑定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailPlayerIdentityProofSource {
    AuthenticatedPlayerPage,
    AuthenticatedHvUidForumTargetProfile,
}

impl MailPlayerIdentityProofSource {
    /// 返回审计使用的固定来源类别，不接受配置或远端任意字符串。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AuthenticatedPlayerPage => "authenticated_player_page",
            Self::AuthenticatedHvUidForumTargetProfile => {
                "authenticated_hv_uid_forum_target_profile_v1"
            }
        }
    }
}

/// 同一 active Cookie、world 与 ownership 下实时读取的身份事实。
/// 仅供可信集成边界生成，不支持反序列化或由管理员输入构造。结构校验不是远端证明；
/// 消费方必须在事务内重锁凭据版本，不能持久缓存后重新用于内部结清。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedMailPlayerIdentity {
    pub credential_id: Uuid,
    pub credential_version: i64,
    pub world: SettlementWorld,
    pub player_name: String,
    pub observed_at: DateTime<Utc>,
    pub proof_source: MailPlayerIdentityProofSource,
    /// 仅规范化非敏感身份证据的 SHA-256，不包含 Cookie 或完整页面。
    pub proof_digest: [u8; 32],
}

impl VerifiedMailPlayerIdentity {
    /// 拒绝非法身份字段；不把结构有效视为认证成功。
    pub fn validate(&self) -> bool {
        !self.credential_id.is_nil()
            && self.credential_version > 0
            && valid_mail_player_name(&self.player_name)
            && self.observed_at.timestamp_nanos_opt().is_some()
            && self.proof_digest != [0; 32]
    }
}

/// 完整游戏玩家名称按原文保留，禁止首尾空白、控制字符和超长值。
pub fn valid_mail_player_name(value: &str) -> bool {
    auction_domain::valid_text(value, 256)
}

/// 只有邮件 Cookie 可以声明预期名称；空值必须显式使用 None。
pub fn valid_credential_player_name(kind: super::AdminCredentialKind, value: Option<&str>) -> bool {
    value.is_none_or(|name| {
        kind == super::AdminCredentialKind::MailCookie && valid_mail_player_name(name)
    })
}
