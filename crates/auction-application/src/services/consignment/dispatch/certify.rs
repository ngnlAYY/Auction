//! 组装 Worker 的 COD 实收认证命令（`certify_cod`，D9 §4.2、§6.2）：系统命令 ID 与规范摘要只由本层派生。
//!
//! 保护不变量 1、6：命令 ID 只由原付款义务加原投递决定，同一笔实收的重放与提交未知后的重试落在同一命令上；
//! 所消费的付款证据进摘要，换证据即换请求。opening 份额不由本入口登记或引用（恒为“已证全部未含”）：已有封存
//! 份额时资金 owner 按冲突整笔拒绝，交人工核对，不会把同一金额计两次。

use super::encoding;
use crate::ports::consignment::{CertifyCodFacts, CollectedEvidence};
use crate::ports::settlement::{CertifyCod, FreshCashObservation, FundingMutation};

/// 由冻结事实、已记录的付款证据与（严格现金时）新鲜现金观察组装认证命令；存储在资金锁内逐项复验。
pub fn certify_cod_command(
    facts: CertifyCodFacts,
    evidence: &CollectedEvidence,
    cash: Option<FreshCashObservation>,
) -> CertifyCod {
    let (command_id, request_hash) = encoding::certify_cod(&facts, evidence);
    CertifyCod {
        mutation: FundingMutation {
            command_id: command_id.as_uuid(),
            request_digest: request_hash.as_bytes(),
        },
        contract_id: facts.contract_id,
        payment_obligation_id: facts.payment_obligation_id,
        original_delivery_id: facts.original_delivery_id,
        receipt: facts.receipt,
        world: facts.world,
        mode: facts.mode,
        price: facts.price,
        quote: facts.quote,
        gross: facts.gross,
        freeze: facts.freeze,
        cash,
        attribution_id: None,
    }
}
