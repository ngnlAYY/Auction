//! sending fence 前冻结实际 Send 内容或 Take 会话身份；不保存正文、Cookie 或页面令牌。

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 原外部动作会话的持久凭据及 owner 版本，不含 Cookie、页面令牌或凭据摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliverySendCredentialBinding {
    pub credential_id: Uuid,
    pub credential_version: i64,
    pub runtime_generation: i64,
}

impl DeliverySendCredentialBinding {
    pub fn validate(&self) -> bool {
        !self.credential_id.is_nil() && self.credential_version > 0 && self.runtime_generation > 0
    }
}

/// 一次实际 Send 的不可变安全投影；正文只保留协议规范化后的 SHA-256。
/// 配置 Cookie 没有持久凭据身份时允许立即证明成功，但不能自动恢复历史发送。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliverySendExpectation {
    pub world_id: String,
    pub recipient: String,
    pub subject: String,
    pub body_digest: [u8; 32],
    pub credits: i64,
    pub cod: i64,
    /// canonical 正十进制 EID，按文本升序排列且不重复；不能使用内部装备 UUID。
    pub equipment_ids: Vec<String>,
    pub credential: Option<DeliverySendCredentialBinding>,
}

impl DeliverySendExpectation {
    /// 验证规范形状；摘要必须来自消费同一渲染请求的可信传输边界。
    pub fn validate(&self) -> bool {
        matches!(self.world_id.as_str(), "main" | "isekai")
            && crate::valid_mail_player_name(&self.recipient)
            && auction_domain::valid_text(&self.subject, 512)
            && self.body_digest != [0; 32]
            && (0..=crate::MAX_DELIVERY_COD).contains(&self.credits)
            && (0..=crate::MAX_DELIVERY_COD).contains(&self.cod)
            && self.equipment_ids.len() <= 1000
            && self.equipment_ids.iter().all(|id| crate::valid_external_positive_id(id))
            && self.equipment_ids.windows(2).all(|ids| ids[0] < ids[1])
            && self.credential.as_ref().is_none_or(DeliverySendCredentialBinding::validate)
    }
}

/// 一次实际 Take 的原会话与邮件身份；资产仍由同 command/hash 的不可变 payload 拥有。
/// 配置 Cookie 可立即证明动作，但没有持久凭据身份时禁止未知结果自动恢复。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryTakeExpectation {
    pub world_id: String,
    pub external_mail_id: String,
    pub credential: Option<DeliverySendCredentialBinding>,
}

impl DeliveryTakeExpectation {
    /// MID 保持规范正十进制文本，不收窄为数据库整数或推断跨账号唯一。
    pub fn validate(&self) -> bool {
        matches!(self.world_id.as_str(), "main" | "isekai")
            && auction_domain::valid_positive_decimal(&self.external_mail_id, 64)
            && self.credential.as_ref().is_none_or(DeliverySendCredentialBinding::validate)
    }
}

/// 准备 future 与持久 fence 共用的单层共享身份；Send 与 Take 不得互换授权。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryPreparedExpectation {
    Send(Arc<DeliverySendExpectation>),
    Take(Arc<DeliveryTakeExpectation>),
}
