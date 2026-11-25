//! 处罚固定、执行与解禁周期重置的规范摘要与系统命令 UUIDv5。
//!
//! 保护不变量 1：命令 ID 只由业务键派生（处罚业务键、原账号解禁事实），重放同 ID 同摘要；
//! `request_id`、处理时刻与买家工作行 ID 不入摘要。

use auction_domain::CommandId;
use uuid::Uuid;

use super::resolve::{Stream, system_id};
use crate::ports::consignment::RecoveryOperation;
use crate::ports::core::RequestHash;

/// `open_sanction`：业务键为 `(buyer, cycle)`（每个周期至多一项处罚）；账本目标是周期，摘要含买家与处罚。
pub fn open_sanction_command(
    buyer_user_id: Uuid,
    cycle_id: Uuid,
    sanction_id: Uuid,
) -> (CommandId, RequestHash) {
    let mut stream = Stream::new(RecoveryOperation::OpenSanction, cycle_id);
    stream.uuid(buyer_user_id);
    stream.uuid(sanction_id);
    (system_id(RecoveryOperation::OpenSanction, cycle_id.as_bytes()), stream.request_hash())
}

/// `apply_sanction`：业务键为处罚 ID（`(buyer, cycle)` 唯一对应的处罚）；账本目标是处罚，摘要含被处罚买家。
pub fn apply_sanction_command(sanction_id: Uuid, buyer_user_id: Uuid) -> (CommandId, RequestHash) {
    let mut stream = Stream::new(RecoveryOperation::ApplySanction, sanction_id);
    stream.uuid(buyer_user_id);
    (system_id(RecoveryOperation::ApplySanction, sanction_id.as_bytes()), stream.request_hash())
}

/// `reset_cycle`：业务键为原账号解禁事实（解禁命令 ID）；账本目标是买家，摘要含解禁事实。
pub fn reset_cycle_command(buyer_user_id: Uuid, unban_fact_id: Uuid) -> (CommandId, RequestHash) {
    let mut stream = Stream::new(RecoveryOperation::ResetCycle, buyer_user_id);
    stream.uuid(unban_fact_id);
    (system_id(RecoveryOperation::ResetCycle, unban_fact_id.as_bytes()), stream.request_hash())
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{apply_sanction_command, open_sanction_command, reset_cycle_command};
    use crate::ports::consignment::RecoveryOperation;

    const SANCTION: Uuid = Uuid::from_u128(0x71);
    const BUYER: Uuid = Uuid::from_u128(0x72);
    const UNBAN: Uuid = Uuid::from_u128(0x73);

    #[test]
    fn ids_follow_business_keys_and_digests_follow_the_request() {
        let first = apply_sanction_command(SANCTION, BUYER);
        assert_eq!(first, apply_sanction_command(SANCTION, BUYER));
        let other_buyer = apply_sanction_command(SANCTION, Uuid::from_u128(0x74));
        assert_eq!(other_buyer.0, first.0, "同一处罚只有一个命令 ID");
        assert_ne!(other_buyer.1, first.1, "买家不同是异请求");
        let reset = reset_cycle_command(BUYER, UNBAN);
        assert_ne!(reset.0, reset_cycle_command(BUYER, Uuid::from_u128(0x75)).0);
        assert_eq!(reset.0, reset_cycle_command(Uuid::from_u128(0x76), UNBAN).0);
        assert_ne!(reset.1, reset_cycle_command(Uuid::from_u128(0x76), UNBAN).1);
    }

    #[test]
    fn system_command_ids_are_uuid_v5_as_the_ledger_requires() {
        assert_eq!(apply_sanction_command(SANCTION, BUYER).0.as_uuid().get_version_num(), 5);
        assert_eq!(reset_cycle_command(BUYER, UNBAN).0.as_uuid().get_version_num(), 5);
        assert_eq!(open_sanction_command(BUYER, UNBAN, SANCTION).0.as_uuid().get_version_num(), 5);
    }

    #[test]
    fn recovery_operation_codes_are_a_closed_set_with_the_expected_actors() {
        let codes: Vec<_> = RecoveryOperation::ALL.iter().map(|op| op.as_str()).collect();
        assert_eq!(
            codes,
            [
                "resolve_return",
                "auto_relist",
                "remind",
                "apply_sanction",
                "reset_cycle",
                "open_sanction",
                "deliver_right_notice"
            ]
        );
        let systems: Vec<_> = RecoveryOperation::ALL
            .iter()
            .filter(|op| op.actor_kind() == "system")
            .map(|op| op.as_str())
            .collect();
        assert_eq!(systems, ["reset_cycle"]);
    }
}
