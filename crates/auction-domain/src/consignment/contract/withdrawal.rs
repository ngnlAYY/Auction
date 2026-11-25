//! Withdrawal 合同条款：只表达撤回意愿与代际前驱，不含任何金额。

use uuid::Uuid;

use super::errors::ConsignmentContractError;
use crate::ConsignmentContractId;

/// 撤回合同的条款；撤回不产生现金，所以没有价格、费用或资金字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithdrawalTerms {
    /// 触发撤回的意愿登记（卖家撤回意愿控制行）。
    pub intent_id: Option<Uuid>,
    /// 被本次撤回取代的前一份合同。
    pub predecessor_contract_id: Option<ConsignmentContractId>,
}

impl WithdrawalTerms {
    /// 引用的标识不得为零 UUID（不变量 6）。
    pub fn validate(&self) -> Result<(), ConsignmentContractError> {
        if self.intent_id.is_some_and(|id| id.is_nil())
            || self.predecessor_contract_id.is_some_and(ConsignmentContractId::is_nil)
        {
            return Err(ConsignmentContractError::Withdrawal("引用标识为零"));
        }
        Ok(())
    }
}
