//! 寄售处罚的账号侧命令：把已固定的处罚映射为 Users owner 的幂等拉黑命令。
//!
//! 复用 [`RecoveryBuyerBlacklister`](crate::ports::admin::RecoveryBuyerBlacklister)：命令 ID 与摘要只由
//! 处罚业务键派生，Worker 重启、重复领取或并发执行都落到同一条命令账本记录（不变量 1）。

use auction_domain::consignment::refusal::SanctionRule;
use auction_domain::{CommandId, SettlementId, deterministic_uuid};
use auction_time::Timestamp;
use serde::Serialize;
use uuid::Uuid;

use crate::ports::admin::{
    RECOVERY_BUYER_BLACKLIST_CONSIGNMENT_ACTOR, RecoveryBuyerBlacklistCommand,
};
use crate::ports::core::RequestHash;

const COMMAND_LABEL: &str = "consignment-sanction-blacklist-command";
const HASH_KIND: &str = "users_consignment_sanction_blacklist";

/// 一项待执行的账号处罚工作；只含固定的业务键，不含阈值、窗口或计数。
#[derive(Debug, Clone, Copy)]
pub struct ConsignmentSanctionAccountWork {
    /// 处罚业务键，`UNIQUE(buyer_user_id, cycle_id)` 对应的处罚 ID。
    pub sanction_id: Uuid,
    /// 待执行账号工作 ID；只回显，不进命令 ID 与摘要，工作重建不会产生第二条命令。
    pub work_id: Uuid,
    /// 被处罚买家。
    pub buyer_user_id: Uuid,
    /// 处罚规则，决定账号理由码。
    pub rule: SanctionRule,
    /// 处罚固定的判定时刻；作为命令发生时刻，重放时保持不变。
    pub occurred_at: Timestamp,
}

/// 构造账号拉黑命令；同一处罚永远得到同一 `command_id` 与 `request_hash`。
pub fn consignment_sanction_blacklist_command(
    work: &ConsignmentSanctionAccountWork,
) -> Result<RecoveryBuyerBlacklistCommand, serde_json::Error> {
    #[derive(Serialize)]
    struct Request<'a> {
        sanction_id: Uuid,
        buyer_user_id: Uuid,
        public_reason: &'a str,
        actor_id: &'a str,
        occurred_at: String,
    }

    let public_reason = work.rule.reason_code();
    let request = Request {
        sanction_id: work.sanction_id,
        buyer_user_id: work.buyer_user_id,
        public_reason,
        actor_id: RECOVERY_BUYER_BLACKLIST_CONSIGNMENT_ACTOR,
        occurred_at: work.occurred_at.to_rfc3339_opts(auction_time::SecondsFormat::Nanos, true),
    };
    Ok(RecoveryBuyerBlacklistCommand {
        command_id: deterministic_uuid(CommandId::from_uuid(work.sanction_id), COMMAND_LABEL, 1),
        request_hash: RequestHash::from_request(HASH_KIND, work.sanction_id, &request)?,
        recovery_id: SettlementId::from_uuid(work.sanction_id),
        action_id: SettlementId::from_uuid(work.work_id),
        buyer_user_id: work.buyer_user_id,
        public_reason: public_reason.to_owned(),
        actor_id: RECOVERY_BUYER_BLACKLIST_CONSIGNMENT_ACTOR.to_owned(),
        occurred_at: work.occurred_at,
    })
}

#[cfg(test)]
mod tests {
    use auction_domain::consignment::refusal::SanctionRule;
    use auction_time::Timestamp;
    use uuid::Uuid;

    use super::{ConsignmentSanctionAccountWork, consignment_sanction_blacklist_command};
    use crate::ports::admin::{
        RECOVERY_BUYER_BLACKLIST_ACTOR, RECOVERY_BUYER_BLACKLIST_CONSIGNMENT_ACTOR,
        RECOVERY_BUYER_BLACKLIST_PUBLIC_REASON,
    };

    fn work(rule: SanctionRule) -> ConsignmentSanctionAccountWork {
        ConsignmentSanctionAccountWork {
            sanction_id: Uuid::from_u128(1),
            work_id: Uuid::from_u128(2),
            buyer_user_id: Uuid::from_u128(3),
            rule,
            occurred_at: Timestamp::from_timestamp(1_800_000_000, 0).unwrap(),
        }
    }

    #[test]
    fn same_sanction_yields_same_command_regardless_of_work_id() {
        let first = consignment_sanction_blacklist_command(&work(SanctionRule::MainCodReturns));
        let mut rebuilt = work(SanctionRule::MainCodReturns);
        rebuilt.work_id = Uuid::from_u128(99);
        let second = consignment_sanction_blacklist_command(&rebuilt);
        let (first, second) = (first.unwrap(), second.unwrap());
        assert!(first.validate());
        assert_eq!(first.command_id, second.command_id);
        assert_eq!(first.request_hash, second.request_hash);
    }

    #[test]
    fn different_sanction_or_rule_yields_different_command() {
        let base =
            consignment_sanction_blacklist_command(&work(SanctionRule::MainCodReturns)).unwrap();
        let other_rule =
            consignment_sanction_blacklist_command(&work(SanctionRule::IsekaiUnpaidCarrierReturns))
                .unwrap();
        assert_eq!(base.command_id, other_rule.command_id);
        assert_ne!(base.request_hash, other_rule.request_hash);
        let mut other = work(SanctionRule::MainCodReturns);
        other.sanction_id = Uuid::from_u128(11);
        let other = consignment_sanction_blacklist_command(&other).unwrap();
        assert_ne!(base.command_id, other.command_id);
    }

    #[test]
    fn consignment_reason_needs_consignment_actor_and_auction_reason_stays_auction_only() {
        let mut command =
            consignment_sanction_blacklist_command(&work(SanctionRule::IsekaiUnpaidCarrierReturns))
                .unwrap();
        assert!(command.validate());
        // 寄售理由码不得借用拍卖 actor。
        command.actor_id = RECOVERY_BUYER_BLACKLIST_ACTOR.to_owned();
        assert!(!command.validate());
        // 拍卖理由仍只配拍卖 actor，行为不变。
        command.public_reason = RECOVERY_BUYER_BLACKLIST_PUBLIC_REASON.to_owned();
        assert!(command.validate());
        command.actor_id = RECOVERY_BUYER_BLACKLIST_CONSIGNMENT_ACTOR.to_owned();
        assert!(!command.validate());
        // 闭集之外的理由一律拒绝。
        command.public_reason = "consignment_other".to_owned();
        assert!(!command.validate());
    }
}
