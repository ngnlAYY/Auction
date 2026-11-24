//! 收件证据会话与世界就绪的窄写端口。
//!
//! 会话只绑定当前 active 的 mail_cookie 凭据与世界；就绪投影只保存能力布尔值、allowlist、
//! 验证器版本与数据库时间，不保存 Cookie、正文或任何秘密。

use auction_domain::SettlementWorld;
use auction_time::Timestamp;
use uuid::Uuid;

use crate::ports::consignment::errors::ConsignmentFuture;

/// 一次已认证的邮件证据会话草稿；写入时刻由数据库时钟决定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailEvidenceSessionDraft {
    /// 会话 UUID；同一凭据绑定重放时必须保持同一值。
    pub session_id: Uuid,
    /// 收件世界。
    pub world: SettlementWorld,
    /// 认证 Bot 的游戏 UID。
    pub bot_uid: String,
    /// Bot 显示的玩家名。
    pub player_name: String,
    /// 当前 mail_cookie 凭据 UUID。
    pub credential_id: Uuid,
    /// 凭据版本；必须与 `credential_keys.state_version` 相等。
    pub credential_version: u64,
    /// 运行时代次。
    pub runtime_generation: u64,
    /// 证据协议版本。
    pub protocol_version: String,
    /// 会话绑定摘要。
    pub binding_digest: [u8; 32],
}

/// 世界收件能力投影；与具体邮件无关。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailReadinessPolicy {
    /// 策略摘要。
    pub policy_digest: [u8; 32],
    /// `enforced` 或 `disabled`。
    pub allowlist_mode: String,
    /// allowlist 中的完整发件人。
    pub allowed_senders: Vec<String>,
    /// 已安装的验证器版本。
    pub validator_versions: Vec<String>,
    /// 详情读取已就绪；只有为真时收件候选门控才可打开。
    pub detail_ready: bool,
    /// Take 验证器已签收。
    pub take_ready: bool,
    /// Return 验证器已签收。
    pub return_ready: bool,
}

/// 一次就绪续期；会话与就绪在同一顶层事务内按此顺序写入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailEvidenceReadinessRecord {
    /// 证据会话。
    pub session: MailEvidenceSessionDraft,
    /// 世界能力。
    pub policy: MailReadinessPolicy,
    /// 当前寄售服务代次。
    pub service_epoch: u64,
    /// 就绪有效秒数。
    pub valid_for_seconds: u32,
}

/// 写后就绪投影。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailReadinessReceipt {
    /// 单调递增的世界就绪版本。
    pub version: u64,
    /// 数据库检查时刻。
    pub checked_at: Timestamp,
    /// 数据库计算的有效期。
    pub valid_until: Timestamp,
}

/// 收件就绪写端。
pub trait MailEvidenceReadinessStore: Send + Sync {
    /// 先写会话再续期世界就绪；会话缺失或凭据不再是当前 active 版本时失败关闭。
    fn record_mail_readiness(
        &self,
        record: MailEvidenceReadinessRecord,
    ) -> ConsignmentFuture<'_, MailReadinessReceipt>;
}
