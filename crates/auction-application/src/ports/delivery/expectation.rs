//! sending fence 前冻结实际渲染身份；持久层只接收正文摘要与非敏感资产、凭据引用。

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 原发送会话的持久凭据及 owner 版本，不含 Cookie、页面令牌或凭据摘要。
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
            && !self.subject.is_empty()
            && self.subject.len() <= 512
            && self.subject == self.subject.trim()
            && !self.subject.chars().any(char::is_control)
            && self.body_digest != [0; 32]
            && (0..=crate::MAX_DELIVERY_COD).contains(&self.credits)
            && (0..=crate::MAX_DELIVERY_COD).contains(&self.cod)
            && self.equipment_ids.len() <= 1000
            && self.equipment_ids.iter().all(|id| crate::valid_external_positive_id(id))
            && self.equipment_ids.windows(2).all(|ids| ids[0] < ids[1])
            && self.credential.as_ref().is_none_or(DeliverySendCredentialBinding::validate)
    }
}
