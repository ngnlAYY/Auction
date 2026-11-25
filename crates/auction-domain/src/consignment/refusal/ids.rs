//! 拒收事实、周期与处罚的确定性 ID；v5 字节一经冻结不得改动。
//!
//! 算法沿 [`legacy_natural_key_uuid`]（长度前缀 SHA-256，截取并置 v5 位），向量固定在
//! `crates/auction-domain/tests/consignment_refusal_vectors.rs`。

use uuid::Uuid;

use crate::delivery::{OriginalMailKey, settlement_world_code};
use crate::{UserId, legacy_natural_key_uuid};

const RETURN_FACT_NAMESPACE: &str = "consignment.return-fact.v1";
const CYCLE_NAMESPACE: &str = "consignment.refusal-cycle.v1";
const SANCTION_NAMESPACE: &str = "consignment.sanction.v1";

/// 退回事实 ID：`(K, 原轮, 资产腿)` 同则同，重复 RTS 与两个消费者重见都落到同一事实。
pub fn return_fact_id(k: &OriginalMailKey, original_round: u32, leg: &str) -> Uuid {
    legacy_natural_key_uuid(
        RETURN_FACT_NAMESPACE,
        &[
            k.authenticated_bot_uid().as_bytes(),
            settlement_world_code(k.world()).as_bytes(),
            k.external_mid().as_bytes(),
            &original_round.to_be_bytes(),
            leg.as_bytes(),
        ],
    )
}

/// 买家第 `ordinal` 个拒收周期的 ID；解禁开启 `ordinal + 1`。
pub fn cycle_id(buyer: UserId, ordinal: u64) -> Uuid {
    legacy_natural_key_uuid(CYCLE_NAMESPACE, &[buyer.as_uuid().as_bytes(), &ordinal.to_be_bytes()])
}

/// 买家在某周期内的唯一处罚 ID，对应 `UNIQUE(buyer_user_id, cycle_id)`。
pub fn sanction_id(buyer: UserId, cycle: Uuid) -> Uuid {
    legacy_natural_key_uuid(SANCTION_NAMESPACE, &[buyer.as_uuid().as_bytes(), cycle.as_bytes()])
}
