//! 合同义务图的持久化端口。

use auction_domain::consignment::ConsignmentObligationGraph;
use auction_domain::{ConsignmentContractId, SettlementId};

use crate::ports::consignment::errors::ConsignmentFuture;

/// 已持久化的义务图摘要；`replayed` 表示本次调用命中了同一合同的既有图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredConsignmentGraph {
    pub contract_id: ConsignmentContractId,
    /// 与图的写入顺序一致。
    pub obligation_ids: Vec<SettlementId>,
    pub replayed: bool,
}

/// 合同义务图写入端口。
pub trait ConsignmentGraphStore: Send + Sync {
    /// 单个 Serializable 事务内依次写合同、Sale/Withdrawal 子项、义务与履约行；
    /// 保护不变量 1、2、3、6：同合同同条款重放返回既有图，同合同异条款返回
    /// `CommandIdConflict`，任一行失败整笔回滚，输入图未通过 `validate_consignment_graph`
    /// 时不写任何行。
    fn write_graph(
        &self,
        graph: ConsignmentObligationGraph,
    ) -> ConsignmentFuture<'_, StoredConsignmentGraph>;
}
