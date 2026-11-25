//! 已证代管件灭失的分流：按当前代是否仍有有效买方交割权或资金责任决定去向。
//!
//! 纯函数；证据完整性与当前代权责由调用方在持实物锁内读好。

use thiserror::Error;

/// 分流所需的锁内事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LossRoutingFacts {
    /// 原世界 / 季 / 原物 / 最后位置 / 完整后续移动 / 排除在途 / 独立灭失证据齐全。
    pub proof_complete: bool,
    /// 当前代是否仍有有效买方交割权；`None` 表示无法查清。
    pub current_generation_buyer_delivery_right: Option<bool>,
    /// 当前代是否仍有资金责任；`None` 表示无法查清。
    pub current_generation_funding_liability: Option<bool>,
    /// 存在相反的未知外写。
    pub opposite_unknown_write: bool,
}

/// 灭失的去向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LossRoute {
    /// 当前代既无有效买方交割权也无资金责任：按 position 办结。
    ClosePosition,
    /// 当前代仍有权责：走原 Sale resolve。
    ResolveInSale,
    /// 无法查清：保留占用与核对案，不终局。
    KeepOccupancy,
}

/// 分流被拒绝的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum LossRoutingError {
    /// 灭失证据不完整（`evidence_unproven`）；空库存、缺 MID 或说明文字都不够。
    #[error("灭失证据不完整")]
    ProofIncomplete,
}

/// 不变量 6：证据不全拒绝；任何一项未知或有相反未知外写都保留占用，不走 position 终局，
/// 也不派出相反付款。只有两项都明确为否才办结 position。
pub fn route_position_loss(facts: &LossRoutingFacts) -> Result<LossRoute, LossRoutingError> {
    if !facts.proof_complete {
        return Err(LossRoutingError::ProofIncomplete);
    }
    if facts.opposite_unknown_write {
        return Ok(LossRoute::KeepOccupancy);
    }
    Ok(
        match (
            facts.current_generation_buyer_delivery_right,
            facts.current_generation_funding_liability,
        ) {
            (Some(false), Some(false)) => LossRoute::ClosePosition,
            (Some(_), Some(_)) => LossRoute::ResolveInSale,
            _ => LossRoute::KeepOccupancy,
        },
    )
}
