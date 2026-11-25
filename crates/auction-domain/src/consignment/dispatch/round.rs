//! 派发轮：键、确定性 ID、轮引用与轮次推进。

use uuid::Uuid;

use super::errors::DispatchRuleError;
use crate::legacy_natural_key_uuid;

const DISPATCH_ROUND_NAMESPACE: &str = "consignment.dispatch-round.v1";

/// 派发轮的业务键：`PK(contract, obligation, round)`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DispatchRoundKey {
    /// 寄售合同。
    pub contract_id: Uuid,
    /// 被履行的义务。
    pub obligation_id: Uuid,
    /// 轮次号，从 1 开始。
    pub round: u32,
}

/// 轮 ID：`(合同, 义务, 轮)` 同则同。
///
/// 作为账本目标（`target_kind = dispatch_round`）与邮件模板的独立轮引用；字节冻结。
pub fn dispatch_round_id(key: &DispatchRoundKey) -> Uuid {
    legacy_natural_key_uuid(
        DISPATCH_ROUND_NAMESPACE,
        &[key.contract_id.as_bytes(), key.obligation_id.as_bytes(), &key.round.to_be_bytes()],
    )
}

/// 邮件模板的 `dispatch_ref`：轮 ID 的小写带连字符文本（裁定 R5-C7）。
pub fn dispatch_ref(key: &DispatchRoundKey) -> String {
    dispatch_round_id(key).hyphenated().to_string()
}

/// 新轮的来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundOrigin {
    /// 购买同事务建立的首轮。
    First,
    /// 上一轮因退件接管而封闭后的新轮；必须引用该接管事实。
    Redispatch {
        /// 封闭上一轮的退件接管（intake）。
        closed_by_return_intake: Uuid,
    },
}

/// 推进轮次号。
///
/// 保护不变量 6：首轮只在没有任何轮时建立；新轮只能紧接上一轮，且必须引用封闭上一轮的
/// 接管事实，不允许跳号、重复或无依据重发。
pub fn next_round_number(
    prior: Option<u32>,
    origin: &RoundOrigin,
) -> Result<u32, DispatchRuleError> {
    match (prior, origin) {
        (None, RoundOrigin::First) => Ok(1),
        (Some(_), RoundOrigin::First) => Err(DispatchRuleError::FirstRoundExists),
        (None, RoundOrigin::Redispatch { .. }) => Err(DispatchRuleError::MissingPriorRound),
        (Some(0), RoundOrigin::Redispatch { .. }) => Err(DispatchRuleError::InvalidRound),
        (Some(prior), RoundOrigin::Redispatch { closed_by_return_intake }) => {
            if closed_by_return_intake.is_nil() {
                return Err(DispatchRuleError::MissingClosure);
            }
            prior.checked_add(1).ok_or(DispatchRuleError::InvalidRound)
        }
    }
}
