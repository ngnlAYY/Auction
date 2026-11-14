//! 不含凭据材料的规范化证据事实与请求身份。

use std::time::Instant;

use auction_domain::{EquipmentId, SettlementWorld};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::ports::core::lower_hex;
use crate::ports::settlement::{
    INTERNAL_EVIDENCE_MAX_AGE_SECONDS, INTERNAL_EVIDENCE_PROTOCOL, valid_external_positive_id,
};

/// 持久全局 mail_cookie generation；仅支持 sticky 撤销的 owner 可以提供。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailRuntimeBinding {
    pub generation: i64,
}

/// 闭集操作身份；资金运维与普通投递不需要伪造结算义务。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InternalEvidencePurpose {
    SettlementClaim {
        obligation_id: Uuid,
        claim_token: Uuid,
        claim_fencing: i64,
        state_version: u64,
    },
    DeliveryClaim {
        delivery_id: Uuid,
        claim_token: Uuid,
        claim_fencing: i64,
        request_hash: crate::ports::core::RequestHash,
    },
    FundingReconciliation {
        command_id: Uuid,
        expected_account_version: u64,
    },
    /// 仅初始化未建账账号的记账模式，不声明或观察真实现金余额。
    FundingRecordOnlyInitialization {
        command_id: Uuid,
        expected_account_version: u64,
    },
}

impl InternalEvidencePurpose {
    /// 仅校验身份形状；数据库仍必须核验具体 claim、租约与稳定命令。
    pub fn validate(&self) -> bool {
        match self {
            Self::SettlementClaim { obligation_id, claim_token, claim_fencing, .. } => {
                !obligation_id.is_nil() && !claim_token.is_nil() && *claim_fencing > 0
            }
            Self::DeliveryClaim { delivery_id, claim_token, claim_fencing, request_hash } => {
                !delivery_id.is_nil()
                    && !claim_token.is_nil()
                    && *claim_fencing > 0
                    && request_hash.as_bytes() != [0; 32]
            }
            Self::FundingReconciliation { command_id, .. }
            | Self::FundingRecordOnlyInitialization { command_id, .. } => !command_id.is_nil(),
        }
    }

    fn digest(&self) -> [u8; 32] {
        match self {
            Self::SettlementClaim { obligation_id, claim_token, claim_fencing, state_version } => {
                facts_digest([
                    "settlement_claim",
                    &obligation_id.to_string(),
                    &claim_token.to_string(),
                    &claim_fencing.to_string(),
                    &state_version.to_string(),
                ])
            }
            Self::DeliveryClaim { delivery_id, claim_token, claim_fencing, request_hash } => {
                facts_digest([
                    "delivery_claim",
                    &delivery_id.to_string(),
                    &claim_token.to_string(),
                    &claim_fencing.to_string(),
                    &lower_hex(&request_hash.as_bytes()),
                ])
            }
            Self::FundingReconciliation { command_id, expected_account_version } => facts_digest([
                "funding_reconciliation",
                &command_id.to_string(),
                &expected_account_version.to_string(),
            ]),
            Self::FundingRecordOnlyInitialization { command_id, expected_account_version } => {
                facts_digest([
                    "funding_record_only_initialization",
                    &command_id.to_string(),
                    &expected_account_version.to_string(),
                ])
            }
        }
    }
}

/// 精确装备目标；内部 UUID 不等于游戏 EID。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InternalEquipmentTarget {
    pub equipment_id: EquipmentId,
    pub external_equipment_id: String,
    pub world: SettlementWorld,
}

/// 只接受可信业务投影，不接受 URL、Cookie、声明姓名或声明现金。
#[derive(Debug)]
pub struct InternalEvidenceRequest {
    pub operation_id: Uuid,
    pub purpose: InternalEvidencePurpose,
    pub world: SettlementWorld,
    pub equipment: Option<InternalEquipmentTarget>,
    pub observe_main_cash: bool,
    pub deadline: Instant,
}

impl InternalEvidenceRequest {
    /// 只检查请求形状，不替代业务 claim 或资金运维授权。
    pub fn validate(&self) -> bool {
        !self.operation_id.is_nil()
            && self.purpose.validate()
            && self.equipment.as_ref().is_none_or(|target| {
                !target.equipment_id.is_nil()
                    && valid_external_positive_id(&target.external_equipment_id)
            })
            && self.deadline > Instant::now()
    }
}

/// 同一认证账号、active 凭据版本、世界和持久 owner 的瞬时联合身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailAccountEvidenceBinding {
    pub credential_id: Uuid,
    pub credential_version: i64,
    pub runtime: MailRuntimeBinding,
    pub authenticated_uid: String,
    pub player_name: String,
    pub world: SettlementWorld,
    pub observed_at: DateTime<Utc>,
    pub protocol_version: &'static str,
    pub digest: [u8; 32],
}

impl MailAccountEvidenceBinding {
    /// 校验绑定形状与规范摘要；真实性由持有会话的集成边界负责。
    pub fn validate(&self) -> bool {
        !self.credential_id.is_nil()
            && self.credential_version > 0
            && self.runtime.generation > 0
            && valid_external_positive_id(&self.authenticated_uid)
            && crate::ports::admin::valid_mail_player_name(&self.player_name)
            && self.protocol_version == INTERNAL_EVIDENCE_PROTOCOL
            && self.digest == self.compute_digest()
    }

    /// 只散列规范化非敏感事实，不散列 Cookie、token 或整页。
    pub fn compute_digest(&self) -> [u8; 32] {
        facts_digest([
            self.protocol_version,
            &self.credential_id.to_string(),
            &self.credential_version.to_string(),
            crate::ports::settlement::MAIL_RUNTIME_SCOPE,
            &self.runtime.generation.to_string(),
            &self.authenticated_uid,
            &self.player_name,
            self.world.as_str(),
            &self.observed_at.to_rfc3339(),
        ])
    }

    /// 不同世界仍必须来自同一数据库凭据和同一认证账号。
    pub fn same_account(&self, other: &Self) -> bool {
        self.credential_id == other.credential_id
            && self.credential_version == other.credential_version
            && self.runtime == other.runtime
            && self.authenticated_uid == other.authenticated_uid
            && self.player_name == other.player_name
    }
}

/// 已观察的正常 organize 行；不宣称全库存或装备可交易/可寄。
#[derive(Debug)]
pub struct VerifiedMailEquipmentPresence {
    pub binding: MailAccountEvidenceBinding,
    pub equipment_id: EquipmentId,
    pub external_equipment_id: String,
    pub category: String,
    pub observed_at: DateTime<Utc>,
    pub digest: [u8; 32],
}

impl VerifiedMailEquipmentPresence {
    /// 散列精确装备与同 session 身份，不包含装备 key。
    pub fn compute_digest(&self) -> [u8; 32] {
        facts_digest([
            "organize_presence_clean_compose_v1",
            &lower_hex(&self.binding.digest),
            &self.equipment_id.to_string(),
            &self.external_equipment_id,
            &self.category,
            &self.observed_at.to_rfc3339(),
        ])
    }
}

/// Main Credits 观察值；自由资金及占用由资金 owner 决定。
#[derive(Debug)]
pub struct VerifiedMainCashObservation {
    pub binding: MailAccountEvidenceBinding,
    pub credits: i64,
    pub observed_at: DateTime<Utc>,
    pub digest: [u8; 32],
}

impl VerifiedMainCashObservation {
    /// 余额是观察事实，不将其命名或散列为自由资金。
    pub fn compute_digest(&self) -> [u8; 32] {
        facts_digest([
            "main_credits_observation_clean_compose_v1",
            &lower_hex(&self.binding.digest),
            &self.credits.to_string(),
            &self.observed_at.to_rfc3339(),
        ])
    }
}

/// 只由可信适配器生成，消费者借用到数据库事务确定结束；不支持 Clone/Deserialize。
#[derive(Debug)]
pub struct SettlementInternalEvidenceSnapshot {
    pub operation_id: Uuid,
    pub purpose: InternalEvidencePurpose,
    pub identity: MailAccountEvidenceBinding,
    pub equipment: Option<VerifiedMailEquipmentPresence>,
    pub main_cash: Option<VerifiedMainCashObservation>,
    pub sent_collection: Option<crate::ports::settlement::VerifiedMailSentCollectionObservation>,
    pub observed_from: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub digest: [u8; 32],
}

impl SettlementInternalEvidenceSnapshot {
    /// 绑定完整 purpose、证据摘要与唯一时窗，不采用可变 Debug 文本。
    pub fn compute_digest(&self) -> [u8; 32] {
        facts_digest([
            INTERNAL_EVIDENCE_PROTOCOL,
            &self.operation_id.to_string(),
            &lower_hex(&self.purpose.digest()),
            &lower_hex(&self.identity.digest),
            &self.equipment.as_ref().map(|value| lower_hex(&value.digest)).unwrap_or_default(),
            &self.main_cash.as_ref().map(|value| lower_hex(&value.digest)).unwrap_or_default(),
            &self
                .sent_collection
                .as_ref()
                .map(|value| lower_hex(&value.digest))
                .unwrap_or_default(),
            &self.observed_from.to_rfc3339(),
            &self.expires_at.to_rfc3339(),
        ])
    }

    /// 结构及交叉绑定校验；时效必须另以数据库 clock_timestamp 复核。
    pub fn validate(&self) -> bool {
        !self.operation_id.is_nil()
            && self.purpose.validate()
            && self.identity.validate()
            && self.expires_at > self.observed_from
            && self.expires_at - self.observed_from
                <= chrono::Duration::seconds(INTERNAL_EVIDENCE_MAX_AGE_SECONDS)
            && self.equipment.as_ref().is_none_or(|value| {
                value.binding.validate()
                    && self.identity.same_account(&value.binding)
                    && !value.equipment_id.is_nil()
                    && valid_external_positive_id(&value.external_equipment_id)
                    && matches!(
                        value.category.as_str(),
                        "weapon_1handed"
                            | "weapon_2handed"
                            | "weapon_staff"
                            | "shield"
                            | "armor_cloth"
                            | "armor_light"
                            | "armor_heavy"
                    )
                    && value.digest == value.compute_digest()
            })
            && self.main_cash.as_ref().is_none_or(|value| {
                value.binding.validate()
                    && self.identity.same_account(&value.binding)
                    && value.binding.world == SettlementWorld::Main
                    && value.credits >= 0
                    && value.digest == value.compute_digest()
            })
            && self
                .sent_collection
                .as_ref()
                .is_none_or(|value| value.validate() && self.identity.same_account(&value.binding))
            && self.digest == self.compute_digest()
    }

    /// 最早观测到最终提交均受同一个证据预算窗口约束，禁止未来时间证明。
    pub fn valid_at(&self, now: DateTime<Utc>) -> bool {
        let valid_time = |time: DateTime<Utc>| time >= self.observed_from && time <= now;
        self.validate()
            && self.observed_from <= now
            && now < self.expires_at
            && now - self.observed_from
                <= chrono::Duration::seconds(INTERNAL_EVIDENCE_MAX_AGE_SECONDS)
            && valid_time(self.identity.observed_at)
            && self.equipment.as_ref().is_none_or(|value| {
                valid_time(value.observed_at) && valid_time(value.binding.observed_at)
            })
            && self.main_cash.as_ref().is_none_or(|value| {
                valid_time(value.observed_at) && valid_time(value.binding.observed_at)
            })
            && self.sent_collection.as_ref().is_none_or(|value| {
                valid_time(value.observed_at) && valid_time(value.binding.observed_at)
            })
    }
}

/// 长度前缀防止不同字段拼接产生同一规范化输入。
pub(super) fn facts_digest<'a>(fields: impl IntoIterator<Item = &'a str>) -> [u8; 32] {
    let mut hash = Sha256::new();
    for field in fields {
        hash.update((field.len() as u64).to_be_bytes());
        hash.update(field.as_bytes());
    }
    hash.finalize().into()
}
