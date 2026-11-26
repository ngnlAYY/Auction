//! 派发尝试：唯一键、确定性 ID 与冻结身份。

use auction_time::Timestamp;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::errors::DispatchRuleError;
use crate::{SettlementWorld, legacy_natural_key_uuid};

const DISPATCH_ATTEMPT_NAMESPACE: &str = "consignment.dispatch-attempt.v1";
const COLLECTION_ATTEMPT_NAMESPACE: &str = "consignment.collection-attempt.v1";
const FROZEN_IDENTITY_DOMAIN: &[u8] = b"consignment.dispatch-identity.v1\0";

/// 尝试的唯一键：`UNIQUE(delivery, fence)`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AttemptKey {
    /// 投递。
    pub delivery_id: Uuid,
    /// 领取围栏。
    pub fence: u64,
}

/// 尝试 ID：`(delivery, fence)` 同则同；字节冻结。
pub fn dispatch_attempt_id(key: &AttemptKey) -> Uuid {
    legacy_natural_key_uuid(
        DISPATCH_ATTEMPT_NAMESPACE,
        &[key.delivery_id.as_bytes(), &key.fence.to_be_bytes()],
    )
}

/// 收款认证的现金证据操作 ID：由尝试确定性派生，且与派发现金证据的 [`dispatch_attempt_id`] 不同，
/// 两类证据不会共用同一操作身份；字节冻结。
pub fn collection_attempt_id(key: &AttemptKey) -> Uuid {
    legacy_natural_key_uuid(
        COLLECTION_ATTEMPT_NAMESPACE,
        &[key.delivery_id.as_bytes(), &key.fence.to_be_bytes()],
    )
}

/// 首次 POST 前冻结的派发身份；起写后任何字段都不得再变。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrozenDispatchIdentity {
    /// 原权利人身份。
    pub rights_holder_identity: String,
    /// 认证 Bot UID。
    pub bot_uid: String,
    /// 世界。
    pub world: SettlementWorld,
    /// 邮件主题。
    pub subject: String,
    /// 模板名。
    pub template: String,
    /// 正文摘要。
    pub body_digest: [u8; 32],
    /// 轮引用，即 [`super::dispatch_ref`]。
    pub round_ref: String,
    /// 期限起点 D0。
    pub d0: Timestamp,
    /// 附带 Credits。
    pub credits: i64,
    /// COD 金额。
    pub cod: i64,
    /// 标的或载体装备 ID。
    pub equipment_ids: Vec<String>,
}

impl FrozenDispatchIdentity {
    /// 校验冻结身份；缺字段、负金额、空或重复装备一律失败关闭。
    pub fn validate(&self) -> Result<(), DispatchRuleError> {
        let text_ok = |value: &str| !value.trim().is_empty() && value == value.trim();
        let mut seen = std::collections::HashSet::new();
        let equipment_ok = self.equipment_ids.iter().all(|id| text_ok(id) && seen.insert(id));
        if !text_ok(&self.rights_holder_identity)
            || !text_ok(&self.bot_uid)
            || !text_ok(&self.subject)
            || !text_ok(&self.template)
            || !text_ok(&self.round_ref)
            || self.credits < 0
            || self.cod < 0
            || !equipment_ok
        {
            return Err(DispatchRuleError::InvalidIdentity);
        }
        Ok(())
    }

    /// 冻结身份摘要：起写后重启核对“同一封”用；装备 ID 先排序，不依赖入参顺序。
    pub fn identity_digest(&self) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(FROZEN_IDENTITY_DOMAIN);
        let part = |hash: &mut Sha256, bytes: &[u8]| {
            hash.update(u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_be_bytes());
            hash.update(bytes);
        };
        part(&mut hash, self.rights_holder_identity.as_bytes());
        part(&mut hash, self.bot_uid.as_bytes());
        part(&mut hash, crate::delivery::settlement_world_code(self.world).as_bytes());
        part(&mut hash, self.subject.as_bytes());
        part(&mut hash, self.template.as_bytes());
        part(&mut hash, &self.body_digest);
        part(&mut hash, self.round_ref.as_bytes());
        part(&mut hash, &self.d0.timestamp().to_be_bytes());
        part(&mut hash, &self.d0.timestamp_subsec_nanos().to_be_bytes());
        part(&mut hash, &self.credits.to_be_bytes());
        part(&mut hash, &self.cod.to_be_bytes());
        let mut equipment = self.equipment_ids.iter().map(String::as_bytes).collect::<Vec<_>>();
        equipment.sort_unstable();
        part(&mut hash, &(equipment.len() as u64).to_be_bytes());
        for id in equipment {
            part(&mut hash, id);
        }
        hash.finalize().into()
    }
}
