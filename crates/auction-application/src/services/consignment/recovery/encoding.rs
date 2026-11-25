//! 第 7 期新增操作的 v2 规范编码与系统命令业务键。
//!
//! 沿 `deposit::encoding` 的流格式与前缀，只新增操作，不改既有字节。`request_id` 与处理时刻
//! 不入摘要。

use uuid::Uuid;

use crate::ports::consignment::{
    ConfirmPositionLossCommand, DepositActor, DepositOperation, RecordLossCaseOutcomeCommand,
    RecordLossProofCommand, RelistCommand,
};
use crate::services::consignment::deposit::encoding::{
    COMMAND_PREFIX_V2, CommandEncoding, CommandTarget,
};

/// 卖家重列：目标挂单，输入为预期版本。
pub fn canonical_relist(command: &RelistCommand) -> CommandEncoding {
    let mut encoding = CommandEncoding::new(
        DepositOperation::Relist,
        &DepositActor::PublicUser(command.seller_user_id),
        CommandTarget::Listing(command.listing_id),
    );
    encoding.u64(command.expected_version.value());
    encoding
}

/// Admin 确认灭失：目标 position，输入为预期版本、证据 ID 与私有说明。
pub fn canonical_confirm_position_loss(command: &ConfirmPositionLossCommand) -> CommandEncoding {
    let mut encoding = CommandEncoding::new(
        DepositOperation::ConfirmPositionLoss,
        &DepositActor::Admin(command.admin_actor.clone()),
        CommandTarget::Position(command.position_id),
    );
    encoding.u64(command.expected_version);
    encoding.uuid(command.proof_id);
    encoding.text(command.reason.as_str());
    encoding
}

/// Admin 追加案件结果：目标 position，输入为案件、案件版本、结果、依据与私有说明。
pub fn canonical_record_loss_case_outcome(
    command: &RecordLossCaseOutcomeCommand,
) -> CommandEncoding {
    let mut encoding = CommandEncoding::new(
        DepositOperation::RecordLossCaseOutcome,
        &DepositActor::Admin(command.admin_actor.clone()),
        CommandTarget::Position(command.position_id),
    );
    encoding.uuid(command.case_id);
    encoding.u64(command.expected_case_version);
    encoding.text(command.outcome.as_str());
    encoding.uuid(command.basis_id);
    encoding.text(command.private_note.as_str());
    encoding
}

/// Admin 录入灭失证据：目标 position，输入为证据来源、观察身份、最后确定位置与私有说明。
///
/// 三个摘要由存储在锁内推导，不是请求输入，所以不入摘要；`request_id` 只进审计。
pub fn canonical_record_position_loss_proof(command: &RecordLossProofCommand) -> CommandEncoding {
    let mut encoding = CommandEncoding::new(
        DepositOperation::RecordPositionLossProof,
        &DepositActor::Admin(command.admin_actor.clone()),
        CommandTarget::Position(command.position_id),
    );
    encoding.text(&command.source_kind);
    encoding.text(&command.observation_ref);
    encoding.text(command.last_known_ref.as_str());
    encoding.text(command.private_note.as_str());
    encoding
}

/// 系统履行撤回意愿：目标挂单，输入为意愿 ID。
pub fn canonical_fulfill_withdraw_intent(listing_id: Uuid, intent_id: Uuid) -> CommandEncoding {
    let mut encoding = CommandEncoding::new(
        DepositOperation::FulfillWithdrawIntent,
        &DepositActor::Worker,
        CommandTarget::Listing(listing_id),
    );
    encoding.uuid(intent_id);
    encoding
}

/// `record_position_loss_proof` 的系统命令业务键：`position || text(来源) || text(观察身份)`。
pub fn loss_proof_business_key(
    position_id: Uuid,
    evidence_source: &str,
    observation_identity: &str,
) -> Vec<u8> {
    let mut key = position_id.as_bytes().to_vec();
    for text in [evidence_source, observation_identity] {
        key.extend_from_slice(&(text.len() as u64).to_be_bytes());
        key.extend_from_slice(text.as_bytes());
    }
    key
}

/// 灭失证据 UUID：`UUIDv5(NAMESPACE_URL, prefix || text("loss_proof_id") || 业务键)`。
/// 证据身份随命令业务键确定，由系统派生而不由调用方给定；同业务键重放得到同一证据 ID。
pub fn loss_proof_id(position_id: Uuid, evidence_source: &str, observation_identity: &str) -> Uuid {
    let mut input = COMMAND_PREFIX_V2.to_vec();
    let domain = "loss_proof_id";
    input.extend_from_slice(&(domain.len() as u64).to_be_bytes());
    input.extend_from_slice(domain.as_bytes());
    input.extend_from_slice(&loss_proof_business_key(
        position_id,
        evidence_source,
        observation_identity,
    ));
    Uuid::new_v5(&Uuid::NAMESPACE_URL, &input)
}

/// `fulfill_withdraw_intent` 的系统命令业务键：意愿 UUID 原 16 字节。
pub fn withdraw_intent_business_key(intent_id: Uuid) -> Vec<u8> {
    intent_id.as_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use auction_domain::consignment::ListingVersion;
    use auction_domain::{CommandId, UserId};
    use uuid::Uuid;

    use super::{
        canonical_confirm_position_loss, canonical_fulfill_withdraw_intent,
        canonical_record_loss_case_outcome, canonical_record_position_loss_proof, canonical_relist,
        loss_proof_business_key, loss_proof_id, withdraw_intent_business_key,
    };
    use crate::ports::consignment::{
        ConfirmPositionLossCommand, DepositActor, DepositOperation, RecordLossCaseOutcomeCommand,
        RecordLossProofCommand, RelistCommand,
    };
    use crate::ports::consignment::{ListingDelistReason, LossCaseOutcome, LossLastKnownRef};
    use crate::ports::core::RequestHash;
    use crate::services::consignment::deposit::encoding::{COMMAND_PREFIX_V2, system_command_id};
    use crate::services::consignment::deposit::encoding::{CommandEncoding, CommandTarget};

    const RELIST_SHA: &str = "41b71094e3d77892bf59536056d28466e55cbf0487c4f07ea1576fa1f30ec2e3";
    const CONFIRM_SHA: &str = "755b4b83398ba2b0e3957d2501dcfc4c3d68434766dd64fee808911544602372";
    const OUTCOME_SHA: &str = "01ef2a7d26d59fc3c3f3a77f2bd6651819277055d3cb5108c59468005ed3d770";
    const PROOF_SHA: &str = "f079fd22187bbbbb7b2da6f39c51608a056ea19f5ba3c3e452f8897610a147b6";
    const FULFILL_SHA: &str = "56439651d6b96a8e997ed15414c91644a5fd96a369c685a7e43dae78f8ee7821";
    const PROOF_ID: &str = "46ebddd2-64b4-55d5-aa57-debae91c7bf8";
    const LOSS_PROOF_ID: &str = "7d063ef3-791b-5b7e-a9c0-8865421258dc";
    const FULFILL_ID: &str = "2fd5cc8c-87f4-5782-8979-2a593705432d";

    fn uuid(value: u128) -> Uuid {
        Uuid::from_u128(value)
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn text(output: &mut Vec<u8>, value: &str) {
        output.extend_from_slice(&(value.len() as u64).to_be_bytes());
        output.extend_from_slice(value.as_bytes());
    }

    /// 独立于 `CommandEncoding` 手写头部，防止构建器与向量一起漂移。
    fn head(operation: &str, actor_kind: &str) -> Vec<u8> {
        let mut output = COMMAND_PREFIX_V2.to_vec();
        text(&mut output, operation);
        text(&mut output, actor_kind);
        output
    }

    fn reason(value: &str) -> ListingDelistReason {
        ListingDelistReason::new(value.to_owned()).unwrap()
    }

    fn relist(expected: u64) -> RelistCommand {
        RelistCommand {
            command_id: CommandId::from_uuid(uuid(90)),
            seller_user_id: UserId::from_uuid(uuid(7)),
            listing_id: uuid(1),
            expected_version: ListingVersion::new(expected).unwrap(),
            request_hash: RequestHash::new([0; 32]),
            request_id: None,
        }
    }

    fn confirm() -> ConfirmPositionLossCommand {
        ConfirmPositionLossCommand {
            admin_actor: "admin-1".to_owned(),
            command_id: CommandId::from_uuid(uuid(91)),
            position_id: uuid(2),
            expected_version: 5,
            proof_id: uuid(3),
            reason: reason("loss proven"),
            request_hash: RequestHash::new([0; 32]),
            request_id: None,
        }
    }

    fn outcome() -> RecordLossCaseOutcomeCommand {
        RecordLossCaseOutcomeCommand {
            admin_actor: "admin-1".to_owned(),
            command_id: CommandId::from_uuid(uuid(92)),
            position_id: uuid(2),
            case_id: uuid(4),
            expected_case_version: 1,
            outcome: LossCaseOutcome::Reviewing,
            basis_id: uuid(5),
            private_note: reason("manual review"),
            request_hash: RequestHash::new([0; 32]),
            request_id: None,
        }
    }

    fn proof() -> RecordLossProofCommand {
        RecordLossProofCommand {
            admin_actor: "admin-1".to_owned(),
            command_id: CommandId::from_uuid(uuid(93)),
            position_id: uuid(2),
            source_kind: "inventory-scan".to_owned(),
            observation_ref: "obs-1".to_owned(),
            last_known_ref: LossLastKnownRef::new("fake-last-known".to_owned()).unwrap(),
            private_note: reason("proof note"),
            request_hash: RequestHash::new([0; 32]),
            request_id: None,
        }
    }

    fn assert_vector(encoding: &CommandEncoding, expected: &[u8], sha: &str) {
        assert_eq!(hex(encoding.as_bytes()), hex(expected));
        assert_eq!(hex(&encoding.request_hash().as_bytes()), sha);
    }

    #[test]
    fn relist_vector_is_frozen() {
        let mut expected = head("relist", "public_user");
        expected.extend_from_slice(uuid(7).as_bytes());
        expected.extend_from_slice(uuid(1).as_bytes());
        expected.extend_from_slice(&4u64.to_be_bytes());
        assert_vector(&canonical_relist(&relist(4)), &expected, RELIST_SHA);
    }

    #[test]
    fn confirm_position_loss_vector_is_frozen() {
        let mut expected = head("confirm_position_loss", "admin");
        text(&mut expected, "admin-1");
        expected.extend_from_slice(uuid(2).as_bytes());
        expected.extend_from_slice(&5u64.to_be_bytes());
        expected.extend_from_slice(uuid(3).as_bytes());
        text(&mut expected, "loss proven");
        assert_vector(&canonical_confirm_position_loss(&confirm()), &expected, CONFIRM_SHA);
    }

    #[test]
    fn record_loss_case_outcome_vector_is_frozen() {
        let mut expected = head("record_loss_case_outcome", "admin");
        text(&mut expected, "admin-1");
        expected.extend_from_slice(uuid(2).as_bytes());
        expected.extend_from_slice(uuid(4).as_bytes());
        expected.extend_from_slice(&1u64.to_be_bytes());
        text(&mut expected, "reviewing");
        expected.extend_from_slice(uuid(5).as_bytes());
        text(&mut expected, "manual review");
        assert_vector(&canonical_record_loss_case_outcome(&outcome()), &expected, OUTCOME_SHA);
    }

    #[test]
    fn record_position_loss_proof_vector_is_frozen() {
        let mut expected = head("record_position_loss_proof", "admin");
        text(&mut expected, "admin-1");
        expected.extend_from_slice(uuid(2).as_bytes());
        text(&mut expected, "inventory-scan");
        text(&mut expected, "obs-1");
        text(&mut expected, "fake-last-known");
        text(&mut expected, "proof note");
        assert_vector(&canonical_record_position_loss_proof(&proof()), &expected, PROOF_SHA);
    }

    #[test]
    fn fulfill_withdraw_intent_vector_is_frozen() {
        let mut expected = head("fulfill_withdraw_intent", "worker");
        text(&mut expected, "consignment");
        expected.extend_from_slice(uuid(1).as_bytes());
        expected.extend_from_slice(uuid(6).as_bytes());
        assert_vector(&canonical_fulfill_withdraw_intent(uuid(1), uuid(6)), &expected, FULFILL_SHA);
    }

    #[test]
    fn system_ids_have_frozen_golden_vectors() {
        let proof_id = system_command_id(
            DepositOperation::RecordPositionLossProof,
            &loss_proof_business_key(uuid(2), "inventory-scan", "obs-1"),
        );
        assert_eq!(proof_id.as_uuid().to_string(), PROOF_ID);
        let derived = loss_proof_id(uuid(2), "inventory-scan", "obs-1");
        assert_eq!(derived.to_string(), LOSS_PROOF_ID);
        assert_ne!(derived, proof_id.as_uuid());
        assert_ne!(derived, loss_proof_id(uuid(2), "inventory-scan", "obs-2"));
        let fulfill_id = system_command_id(
            DepositOperation::FulfillWithdrawIntent,
            &withdraw_intent_business_key(uuid(6)),
        );
        assert_eq!(fulfill_id.as_uuid().to_string(), FULFILL_ID);
    }

    /// 同 ID 同请求等价：`request_id` 与摘要占位不入规范字节。
    #[test]
    fn request_id_does_not_change_the_digest() {
        let mut other = relist(4);
        other.request_id = Some("trace-1".to_owned());
        assert_eq!(
            canonical_relist(&relist(4)).request_hash().as_bytes(),
            canonical_relist(&other).request_hash().as_bytes()
        );
        let mut other = confirm();
        other.request_id = Some("trace-1".to_owned());
        assert_eq!(
            canonical_confirm_position_loss(&confirm()).request_hash().as_bytes(),
            canonical_confirm_position_loss(&other).request_hash().as_bytes()
        );
    }

    /// 同 ID 异请求冲突：每个输入字段、actor 与目标变体都必须改变摘要。
    #[test]
    fn every_input_field_changes_the_digest() {
        let digest = |encoding: CommandEncoding| encoding.request_hash().as_bytes();

        let base = digest(canonical_relist(&relist(4)));
        assert_ne!(base, digest(canonical_relist(&relist(5))));
        let mut other = relist(4);
        other.listing_id = uuid(11);
        assert_ne!(base, digest(canonical_relist(&other)));
        let mut other = relist(4);
        other.seller_user_id = UserId::from_uuid(uuid(8));
        assert_ne!(base, digest(canonical_relist(&other)));

        let base = digest(canonical_confirm_position_loss(&confirm()));
        let variants: [fn(&mut ConfirmPositionLossCommand); 5] = [
            |c| c.admin_actor = "admin-2".to_owned(),
            |c| c.position_id = uuid(12),
            |c| c.expected_version = 6,
            |c| c.proof_id = uuid(13),
            |c| c.reason = reason("other reason"),
        ];
        for mutate in variants {
            let mut other = confirm();
            mutate(&mut other);
            assert_ne!(base, digest(canonical_confirm_position_loss(&other)));
        }

        let base = digest(canonical_record_loss_case_outcome(&outcome()));
        let variants: [fn(&mut RecordLossCaseOutcomeCommand); 6] = [
            |c| c.admin_actor = "admin-2".to_owned(),
            |c| c.position_id = uuid(12),
            |c| c.case_id = uuid(14),
            |c| c.expected_case_version = 2,
            |c| c.outcome = LossCaseOutcome::Closed,
            |c| c.basis_id = uuid(15),
        ];
        for mutate in variants {
            let mut other = outcome();
            mutate(&mut other);
            assert_ne!(base, digest(canonical_record_loss_case_outcome(&other)));
        }
        let mut other = outcome();
        other.private_note = reason("other note");
        assert_ne!(base, digest(canonical_record_loss_case_outcome(&other)));

        let base = digest(canonical_record_position_loss_proof(&proof()));
        let variants: [fn(&mut RecordLossProofCommand); 6] = [
            |c| c.admin_actor = "admin-2".to_owned(),
            |c| c.position_id = uuid(12),
            |c| c.source_kind = "other".to_owned(),
            |c| c.observation_ref = "obs-2".to_owned(),
            |c| c.last_known_ref = LossLastKnownRef::new("other-ref".to_owned()).unwrap(),
            |c| c.private_note = reason("other note"),
        ];
        for mutate in variants {
            let mut other = proof();
            mutate(&mut other);
            assert_ne!(base, digest(canonical_record_position_loss_proof(&other)));
        }

        let base = digest(canonical_fulfill_withdraw_intent(uuid(1), uuid(6)));
        assert_ne!(base, digest(canonical_fulfill_withdraw_intent(uuid(2), uuid(6))));
        assert_ne!(base, digest(canonical_fulfill_withdraw_intent(uuid(1), uuid(7))));
    }

    /// 目标种类不单独入流（目标都是原 16 字节 UUID），由操作名确定；不同操作名必须得到不同字节流。
    #[test]
    fn operation_name_distinguishes_streams_over_the_same_target_bytes() {
        let by_listing = CommandEncoding::new(
            DepositOperation::ConfirmPositionLoss,
            &DepositActor::Admin("admin-1".to_owned()),
            CommandTarget::Listing(uuid(2)),
        );
        let by_position = CommandEncoding::new(
            DepositOperation::ConfirmPositionLoss,
            &DepositActor::Admin("admin-1".to_owned()),
            CommandTarget::Position(uuid(2)),
        );
        assert_eq!(by_listing.as_bytes(), by_position.as_bytes());
        let withdraw = CommandEncoding::new(
            DepositOperation::Withdraw,
            &DepositActor::PublicUser(UserId::from_uuid(uuid(7))),
            CommandTarget::Listing(uuid(1)),
        );
        let relist_head = CommandEncoding::new(
            DepositOperation::Relist,
            &DepositActor::PublicUser(UserId::from_uuid(uuid(7))),
            CommandTarget::Listing(uuid(1)),
        );
        assert_ne!(withdraw.as_bytes(), relist_head.as_bytes());
    }

    /// 既有操作的字节不受影响：撤回仍以第 4 期头部编码。
    #[test]
    fn existing_withdraw_head_is_unchanged() {
        let encoding = CommandEncoding::new(
            DepositOperation::Withdraw,
            &DepositActor::PublicUser(UserId::from_uuid(uuid(7))),
            CommandTarget::Listing(uuid(1)),
        );
        let mut expected = head("withdraw", "public_user");
        expected.extend_from_slice(uuid(7).as_bytes());
        expected.extend_from_slice(uuid(1).as_bytes());
        assert_eq!(encoding.as_bytes(), expected.as_slice());
    }
}
