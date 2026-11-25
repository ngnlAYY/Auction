//! 寄售合同：Sale 与 Withdrawal 两种不可变合同及其类型化义务图。
//!
//! 合同是成交或撤回时冻结的事实，义务图由合同唯一决定；本模块不读时钟、不访问存储。

mod errors;
mod graph;
mod sale;
mod withdrawal;

use auction_time::Timestamp;
use uuid::Uuid;

pub use errors::ConsignmentContractError;
pub use graph::{
    ConsignmentObligation, ConsignmentObligationGraph, build_consignment_obligations,
    consignment_obligation_id, validate_consignment_graph,
};
pub use sale::{
    CONSIGNMENT_SETTLEMENT_POLICY_VERSION, ConsignmentSalePolicy, FundingAccountingMode,
    ISEKAI_REFUSAL_WINDOW_DAYS, MAX_FROZEN_TEXT_BYTES, REMINDER_LIMIT, SaleFundingFreeze,
    SaleTerms, SaleTermsInput,
};
pub use withdrawal::WithdrawalTerms;

pub use crate::settlement::ConsignmentContractKind;
use crate::{CommandId, ConsignmentContractId, SettlementWorld, UserId, valid_text};

/// 世代编号的安全上限，与数据库、DTO 的共同整数范围一致。
const MAX_GENERATION: u64 = 9_007_199_254_740_991;
/// 规范装备编号的字节上限。
const MAX_CANONICAL_EID_BYTES: usize = 64;

/// 两种合同共有的核心：谁、哪件实物、哪一代处分。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsignmentContractCore {
    pub contract_id: ConsignmentContractId,
    /// 寄售命令账本中产生本合同的命令。
    pub origin_command_id: CommandId,
    pub listing_id: Uuid,
    pub position_id: Uuid,
    /// 处分代；`(position, generation)` 全局唯一。
    pub generation: u64,
    /// 寄售实物身份（`consignment_custody_assets.equipment_id`）。
    pub custody_equipment_id: Uuid,
    pub world: SettlementWorld,
    pub canonical_eid: String,
    pub season_window_id: Option<Uuid>,
    pub seller_user_id: UserId,
    pub frozen_seller_identity: String,
    pub created_at: Timestamp,
}

impl ConsignmentContractCore {
    fn validate(&self) -> Result<(), ConsignmentContractError> {
        let ids_nil = self.contract_id.is_nil()
            || self.origin_command_id.is_nil()
            || self.listing_id.is_nil()
            || self.position_id.is_nil()
            || self.custody_equipment_id.is_nil()
            || self.seller_user_id.is_nil()
            || self.season_window_id.is_some_and(|id| id.is_nil());
        if ids_nil {
            return Err(ConsignmentContractError::Core("标识为零"));
        }
        if !(1..=MAX_GENERATION).contains(&self.generation) {
            return Err(ConsignmentContractError::Core("处分代越界"));
        }
        // 季窗只属于异世界合同。
        if self.world == SettlementWorld::Main && self.season_window_id.is_some() {
            return Err(ConsignmentContractError::Core("main 合同不得带季窗"));
        }
        if !valid_text(&self.canonical_eid, MAX_CANONICAL_EID_BYTES)
            || !valid_text(&self.frozen_seller_identity, MAX_FROZEN_TEXT_BYTES)
        {
            return Err(ConsignmentContractError::Core("装备编号或卖家冻结身份无效"));
        }
        Ok(())
    }
}

/// 寄售合同；种类由条款类型决定，不存在“种类与条款不符”的取值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsignmentContract {
    Sale { core: ConsignmentContractCore, terms: SaleTerms },
    Withdrawal { core: ConsignmentContractCore, terms: WithdrawalTerms },
}

impl ConsignmentContract {
    pub const fn kind(&self) -> ConsignmentContractKind {
        match self {
            Self::Sale { .. } => ConsignmentContractKind::Sale,
            Self::Withdrawal { .. } => ConsignmentContractKind::Withdrawal,
        }
    }

    pub fn core(&self) -> &ConsignmentContractCore {
        match self {
            Self::Sale { core, .. } | Self::Withdrawal { core, .. } => core,
        }
    }

    /// 校验核心与条款；付款期限必须晚于合同创建时刻。
    pub fn validate(&self) -> Result<(), ConsignmentContractError> {
        self.core().validate()?;
        match self {
            Self::Sale { core, terms } => {
                terms.validate()?;
                if terms.payment_deadline_at <= core.created_at {
                    return Err(ConsignmentContractError::Sale("付款期限不晚于成交时刻"));
                }
                Ok(())
            }
            Self::Withdrawal { terms, .. } => terms.validate(),
        }
    }
}
