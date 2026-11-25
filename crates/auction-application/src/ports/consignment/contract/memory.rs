//! 义务图端口的内存假实现：只证明端口语义（重放、冲突、无效图不落地），不模拟 SQL。

use std::collections::HashMap;
use std::sync::Mutex;

use auction_domain::ConsignmentContractId;
use auction_domain::consignment::{ConsignmentObligationGraph, validate_consignment_graph};

use super::{ConsignmentGraphStore, StoredConsignmentGraph};
use crate::ports::consignment::errors::{ConsignmentError, ConsignmentFuture};

/// 以合同 ID 为键保存整图；写入前先校验，校验失败不落地。
#[derive(Default)]
pub(crate) struct MemoryGraphStore {
    graphs: Mutex<HashMap<ConsignmentContractId, ConsignmentObligationGraph>>,
}

impl MemoryGraphStore {
    pub(crate) fn len(&self) -> usize {
        self.graphs.lock().map_or(0, |graphs| graphs.len())
    }
}

impl ConsignmentGraphStore for MemoryGraphStore {
    fn write_graph(
        &self,
        graph: ConsignmentObligationGraph,
    ) -> ConsignmentFuture<'_, StoredConsignmentGraph> {
        Box::pin(async move {
            // 无效图是内部构造错误：失败关闭，不写任何行。
            if validate_consignment_graph(&graph).is_err() {
                return Err(ConsignmentError::Unavailable);
            }
            let contract_id = graph.contract.core().contract_id;
            let obligation_ids = graph.obligations.iter().map(|item| item.id).collect();
            let Ok(mut graphs) = self.graphs.lock() else {
                return Err(ConsignmentError::Unavailable);
            };
            match graphs.get(&contract_id) {
                Some(existing) if *existing == graph => {
                    Ok(StoredConsignmentGraph { contract_id, obligation_ids, replayed: true })
                }
                Some(_) => Err(ConsignmentError::CommandIdConflict),
                None => {
                    graphs.insert(contract_id, graph);
                    Ok(StoredConsignmentGraph { contract_id, obligation_ids, replayed: false })
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use auction_domain::consignment::{
        ConsignmentContract, ConsignmentContractCore, WithdrawalTerms,
        build_consignment_obligations,
    };
    use auction_domain::{
        CommandId, ConsignmentContractId, SettlementAmount, SettlementWorld, UserId,
    };
    use auction_time::Timestamp;
    use uuid::Uuid;

    use super::MemoryGraphStore;
    use crate::ports::consignment::contract::ConsignmentGraphStore;
    use crate::ports::consignment::errors::ConsignmentError;

    /// 只轮询立即就绪的假端口，不引入额外异步运行时依赖。
    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        let mut future = Box::pin(future);
        loop {
            if let std::task::Poll::Ready(value) = future.as_mut().poll(&mut context) {
                return value;
            }
        }
    }

    fn withdrawal(contract_id: ConsignmentContractId, eid: &str) -> ConsignmentContract {
        ConsignmentContract::Withdrawal {
            core: ConsignmentContractCore {
                contract_id,
                origin_command_id: CommandId::new(),
                listing_id: Uuid::new_v4(),
                position_id: Uuid::new_v4(),
                generation: 1,
                custody_equipment_id: Uuid::new_v4(),
                world: SettlementWorld::Main,
                canonical_eid: eid.to_owned(),
                season_window_id: None,
                seller_user_id: UserId::new(),
                frozen_seller_identity: "Seller".to_owned(),
                created_at: Timestamp::now(),
            },
            terms: WithdrawalTerms { intent_id: None, predecessor_contract_id: None },
        }
    }

    #[test]
    fn graph_write_replays_same_terms_and_conflicts_on_different_terms() {
        let store = MemoryGraphStore::default();
        let contract_id = ConsignmentContractId::new();
        let first = build_consignment_obligations(&withdrawal(contract_id, "1")).unwrap();
        let stored = block_on(store.write_graph(first.clone())).unwrap();
        assert!(!stored.replayed);
        let replay = block_on(store.write_graph(first)).unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.obligation_ids, stored.obligation_ids);

        // 同合同 ID、不同条款：冲突，且不覆盖既有图。
        let different = build_consignment_obligations(&withdrawal(contract_id, "2")).unwrap();
        assert_eq!(
            block_on(store.write_graph(different)),
            Err(ConsignmentError::CommandIdConflict)
        );
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn invalid_graph_is_rejected_without_writing() {
        let store = MemoryGraphStore::default();
        let mut graph =
            build_consignment_obligations(&withdrawal(ConsignmentContractId::new(), "1")).unwrap();
        graph.obligations[0].amount = SettlementAmount::new(1).unwrap();
        assert_eq!(block_on(store.write_graph(graph)), Err(ConsignmentError::Unavailable));
        assert_eq!(store.len(), 0);
    }
}
