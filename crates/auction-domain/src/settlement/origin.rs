//! 结算义务的来源：拍卖或寄售合同，二者恰一。

use crate::{AuctionId, CommandId, ConsignmentContractId, DomainError, LotId, SubmissionId};

/// 寄售合同种类；数据库码 `sale` / `withdrawal`。
///
/// 定义在 settlement 侧，由 `consignment` 模块重导出，避免结算反向依赖寄售。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConsignmentContractKind {
    Sale,
    Withdrawal,
}

impl ConsignmentContractKind {
    /// 持久化与契约使用的稳定名称；已发布值不可改名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sale => "sale",
            Self::Withdrawal => "withdrawal",
        }
    }

    /// 只接受闭集名称；未知值返回 `None`，由调用方失败关闭。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "sale" => Some(Self::Sale),
            "withdrawal" => Some(Self::Withdrawal),
            _ => None,
        }
    }
}

/// 义务来源：恰一个。拍卖保留原命令与投稿；寄售只引用真实合同。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettlementOrigin {
    Auction {
        auction_id: AuctionId,
        lot_id: LotId,
        origin_command_id: CommandId,
        submission_id: Option<SubmissionId>,
    },
    Consignment {
        contract_id: ConsignmentContractId,
        kind: ConsignmentContractKind,
    },
}

impl SettlementOrigin {
    /// 任一标识为零 UUID 即拒绝（不变量 6：非法身份失败关闭）。
    pub fn validate(&self) -> Result<(), DomainError> {
        let valid = match self {
            Self::Auction { auction_id, lot_id, origin_command_id, submission_id } => {
                !auction_id.is_nil()
                    && !lot_id.is_nil()
                    && !origin_command_id.is_nil()
                    && !submission_id.is_some_and(SubmissionId::is_nil)
            }
            Self::Consignment { contract_id, .. } => !contract_id.is_nil(),
        };
        if valid {
            Ok(())
        } else {
            Err(DomainError::InvalidSettlementObligation("义务来源标识无效"))
        }
    }

    /// 是否来自寄售合同。
    pub const fn is_consignment(&self) -> bool {
        matches!(self, Self::Consignment { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::{ConsignmentContractKind, SettlementOrigin};
    use crate::{AuctionId, CommandId, ConsignmentContractId, LotId};

    #[test]
    fn kind_names_round_trip_and_unknown_is_rejected() {
        for kind in [ConsignmentContractKind::Sale, ConsignmentContractKind::Withdrawal] {
            assert_eq!(ConsignmentContractKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(ConsignmentContractKind::parse("Sale"), None);
    }

    #[test]
    fn origin_rejects_nil_identifiers() {
        let nil = uuid::Uuid::nil();
        let consignment = SettlementOrigin::Consignment {
            contract_id: ConsignmentContractId::new(),
            kind: ConsignmentContractKind::Sale,
        };
        assert!(consignment.validate().is_ok() && consignment.is_consignment());
        let nil_contract = SettlementOrigin::Consignment {
            contract_id: ConsignmentContractId::from_uuid(nil),
            kind: ConsignmentContractKind::Sale,
        };
        assert!(nil_contract.validate().is_err());
        let auction = |lot: LotId| SettlementOrigin::Auction {
            auction_id: AuctionId::new(),
            lot_id: lot,
            origin_command_id: CommandId::new(),
            submission_id: None,
        };
        assert!(auction(LotId::new()).validate().is_ok());
        assert!(auction(LotId::from_uuid(nil)).validate().is_err());
    }
}
