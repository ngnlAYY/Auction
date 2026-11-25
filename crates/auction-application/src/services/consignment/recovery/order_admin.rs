//! Admin 订单 recheck 与 resolve 用例：校验命令身份，产生 v2 规范编码与摘要后交给写端口。
//!
//! 保护不变量 1、6：命令 ID 必须是客户端 UUIDv4；摘要只在此处产生，含目标合同、预期版本、动作与证据 ID，
//! `request_id` 只进审计、不进摘要。同 ID 同请求返回原回执，异请求由存储层判 `command_id_conflict`。

use std::sync::Arc;

use auction_domain::CommandId;
use uuid::Uuid;

use crate::ports::consignment::{
    ConsignmentError, DepositActor, OrderAdminOperation, OrderAdminStore, OrderResolveAction,
    OrderResolveReceipt, RecheckOrderCommand, RecheckOrderReceipt, ResolveOrderCommand,
    validate_listing_client_command_id, validate_listing_request_id,
};
use crate::ports::core::RequestHash;
use crate::services::consignment::deposit::encoding::{CommandEncoding, CommandTarget};

/// Admin 排入复核的输入。
pub struct RecheckOrderInput {
    /// 已认证 Admin 稳定引用。
    pub admin_actor: String,
    /// Admin 会话 UUIDv4。
    pub command_id: CommandId,
    /// 目标合同。
    pub contract_id: Uuid,
    /// 已提交合同版本。
    pub expected_version: u64,
    /// 仅审计关联。
    pub request_id: Option<String>,
}

/// Admin 处置的输入。
pub struct ResolveOrderInput {
    /// 已认证 Admin 稳定引用。
    pub admin_actor: String,
    /// Admin 会话 UUIDv4。
    pub command_id: CommandId,
    /// 目标合同。
    pub contract_id: Uuid,
    /// 已提交合同版本。
    pub expected_version: u64,
    /// 处置动作。
    pub action: OrderResolveAction,
    /// 证据 UUID。
    pub evidence_id: Uuid,
    /// 仅审计关联。
    pub request_id: Option<String>,
}

/// recheck 的规范编码：目标合同，输入为预期版本。
pub fn canonical_recheck_order(command: &RecheckOrderCommand) -> CommandEncoding {
    let mut encoding = CommandEncoding::with_operation(
        OrderAdminOperation::Recheck.as_str(),
        &DepositActor::Admin(command.admin_actor.clone()),
        CommandTarget::Contract(command.contract_id),
    );
    encoding.u64(command.expected_version);
    encoding
}

/// resolve 的规范编码：目标合同，输入为预期版本、动作与证据 ID。
pub fn canonical_resolve_order(command: &ResolveOrderCommand) -> CommandEncoding {
    let mut encoding = CommandEncoding::with_operation(
        OrderAdminOperation::Resolve.as_str(),
        &DepositActor::Admin(command.admin_actor.clone()),
        CommandTarget::Contract(command.contract_id),
    );
    encoding.u64(command.expected_version);
    encoding.text(command.action.as_str());
    encoding.uuid(command.evidence_id);
    encoding
}

/// Admin 订单 recheck / resolve 用例。
pub struct OrderAdminService {
    store: Arc<dyn OrderAdminStore>,
}

impl OrderAdminService {
    /// 注入写端口。
    pub fn new(store: Arc<dyn OrderAdminStore>) -> Self {
        Self { store }
    }

    /// 排入订单复核；回执只有合同、版本与工作 ID。
    pub async fn recheck(
        &self,
        input: RecheckOrderInput,
    ) -> Result<RecheckOrderReceipt, ConsignmentError> {
        validate_listing_client_command_id(input.command_id)?;
        validate_listing_request_id(input.request_id.as_deref())?;
        let mut command = RecheckOrderCommand {
            admin_actor: input.admin_actor,
            command_id: input.command_id,
            contract_id: input.contract_id,
            expected_version: input.expected_version,
            request_hash: RequestHash::new([0; 32]),
            request_id: input.request_id,
        };
        command.request_hash = canonical_recheck_order(&command).request_hash();
        self.store.recheck_order(command).await
    }

    /// 记录处置；退款类动作的资金只经 6B 单写者，这里没有任何金额输入。
    pub async fn resolve(
        &self,
        input: ResolveOrderInput,
    ) -> Result<OrderResolveReceipt, ConsignmentError> {
        validate_listing_client_command_id(input.command_id)?;
        validate_listing_request_id(input.request_id.as_deref())?;
        let mut command = ResolveOrderCommand {
            admin_actor: input.admin_actor,
            command_id: input.command_id,
            contract_id: input.contract_id,
            expected_version: input.expected_version,
            action: input.action,
            evidence_id: input.evidence_id,
            request_hash: RequestHash::new([0; 32]),
            request_id: input.request_id,
        };
        command.request_hash = canonical_resolve_order(&command).request_hash();
        self.store.resolve_order(command).await
    }
}

#[cfg(test)]
mod tests {
    use auction_domain::CommandId;
    use uuid::Uuid;

    use super::{canonical_recheck_order, canonical_resolve_order};
    use crate::ports::consignment::{OrderResolveAction, RecheckOrderCommand, ResolveOrderCommand};
    use crate::ports::core::RequestHash;
    use crate::services::consignment::deposit::encoding::COMMAND_PREFIX_V2;

    fn text(bytes: &mut Vec<u8>, value: &str) {
        bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
        bytes.extend_from_slice(value.as_bytes());
    }

    fn id(byte: u8) -> Uuid {
        Uuid::from_bytes([byte; 16])
    }

    fn resolve() -> ResolveOrderCommand {
        ResolveOrderCommand {
            admin_actor: "admin-fake".into(),
            command_id: CommandId::from_uuid(id(1)),
            contract_id: id(2),
            expected_version: 7,
            action: OrderResolveAction::RefundReturned,
            evidence_id: id(3),
            request_hash: RequestHash::new([0; 32]),
            request_id: Some("req-1".into()),
        }
    }

    /// 规范字节冻结：前缀、操作、actor 类型与引用、目标 UUID 原 16 字节、再是输入。
    #[test]
    fn canonical_bytes_are_frozen() {
        let mut expected = COMMAND_PREFIX_V2.to_vec();
        text(&mut expected, "resolve");
        text(&mut expected, "admin");
        text(&mut expected, "admin-fake");
        expected.extend_from_slice(id(2).as_bytes());
        expected.extend_from_slice(&7_u64.to_be_bytes());
        text(&mut expected, "refund_returned");
        expected.extend_from_slice(id(3).as_bytes());
        assert_eq!(
            canonical_resolve_order(&resolve()).request_hash(),
            RequestHash::from_canonical_bytes(&expected)
        );

        let recheck = RecheckOrderCommand {
            admin_actor: "admin-fake".into(),
            command_id: CommandId::from_uuid(id(1)),
            contract_id: id(2),
            expected_version: 7,
            request_hash: RequestHash::new([0; 32]),
            request_id: None,
        };
        let mut expected = COMMAND_PREFIX_V2.to_vec();
        text(&mut expected, "recheck");
        text(&mut expected, "admin");
        text(&mut expected, "admin-fake");
        expected.extend_from_slice(id(2).as_bytes());
        expected.extend_from_slice(&7_u64.to_be_bytes());
        assert_eq!(
            canonical_recheck_order(&recheck).request_hash(),
            RequestHash::from_canonical_bytes(&expected)
        );
    }

    /// 异请求必须异摘要：换动作、证据、版本、目标或 actor 都改变摘要；`request_id` 不入摘要。
    #[test]
    fn every_input_changes_the_digest_but_request_id_does_not() {
        let base = canonical_resolve_order(&resolve()).request_hash();
        let variants: [fn(&mut ResolveOrderCommand); 5] = [
            |command| command.action = OrderResolveAction::RefundLost,
            |command| command.evidence_id = Uuid::from_bytes([9; 16]),
            |command| command.expected_version = 8,
            |command| command.contract_id = Uuid::from_bytes([9; 16]),
            |command| command.admin_actor = "other-admin".into(),
        ];
        for change in variants {
            let mut other = resolve();
            change(&mut other);
            assert_ne!(base, canonical_resolve_order(&other).request_hash());
        }
        let mut same = resolve();
        same.request_id = Some("req-2".into());
        assert_eq!(base, canonical_resolve_order(&same).request_hash());
    }
}
