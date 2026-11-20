//! 根据已验证的责任与交付证据判定挂单的历史资格。

use std::collections::{BTreeMap, BTreeSet};

use crate::consignment::{ListingEquipmentIdentity, ListingError};
use crate::{AuctionStatus, LotId, SettlementObligationKind, UserId};

/// 同一世界内一条历史别名装备行的证据摘要。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryAliasEvidence {
    /// 此装备行仍有在拍卖中的 lot。
    pub has_active_auction: bool,
    /// 任一关联实物链仍待交割或缺少应有的返还证明。
    pub has_unsettled_transfer: bool,
    /// 任一关联实物派发的结果仍未知。
    pub has_unknown_dispatch: bool,
    /// 任一关联实物案件仍未结清。
    pub has_open_equipment_case: bool,
    /// 有确切证据表明此别名行与其他别名行的原件证据矛盾。
    pub has_conflicting_original_equipment: bool,
    /// 此行自身最近的 lot。无 lot 或无法确定唯一末端时为 `None`。
    pub latest_lot: Option<HistoryLotEvidence>,
}

/// 一条历史装备行最近的拍卖 lot 及其末端交付证据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryLotEvidence {
    /// 历史拍卖 lot 的本地标识，用于核对交接关系。
    pub lot_id: LotId,
    /// 此 lot 的归属场次状态。
    pub auction_status: AuctionStatus,
    /// 此 lot 的结局。
    pub outcome: HistoryLotOutcome,
    /// 原卖家的唯一本站用户 ID；缺失或无法唯一解析时为 `None`。
    pub seller_user_id: Option<UserId>,
    /// 对应原件的交付或寄回摘要；缺少对应义务时为 `None`。
    pub original_transfer: Option<HistoryTransferEvidence>,
    /// 从另一条别名行的末端抵达此 lot 的已核验交接关系。
    pub handoff_from: Option<HistoryAliasHandoff>,
}

/// 影响装备历史资格的 lot 结局。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryLotOutcome {
    /// 尚待开拍。
    Pending,
    /// 正在竞价。
    Live,
    /// 已成交。
    Sold {
        /// 成交赢家的本站用户 ID；缺少冻结身份时为 `None`。
        winner_user_id: Option<UserId>,
    },
    /// 已流拍。
    Unsold,
    /// 已撤拍；合同未给这个结局定义历史放行条件。
    Withdrawn,
}

/// 对应同一义务及派发的原物证据，不携带邮件载荷或装备凭证。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryTransferEvidence {
    /// 结算义务种类。
    pub obligation_kind: SettlementObligationKind,
    /// 义务与冻结投递所指的 lot、世界和原件相符。
    pub matches_original_lot_and_equipment: bool,
    /// 对应义务的履约已满足。
    pub fulfillment_satisfied: bool,
    /// 对应原件的投递已成功。
    pub original_delivery_succeeded: bool,
    /// 有与该投递、原件及收件人相符的实际领取证明。
    pub matching_collection_proven: bool,
    /// 领取证明所对应收件人的唯一本站用户 ID。
    pub recipient_user_id: Option<UserId>,
}

/// 两条别名行末端之间可证实的同一原件交接路径。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryAliasHandoff {
    /// 路径起点是另一条别名行的末端 lot。
    pub previous_lot_id: LotId,
    /// 途经中间 lot 时为第一手投稿的原卖家，且须已有逐手交接直至目标 lot 卖家的证明。
    /// 直接衔接时为 `None`，从目标 lot 取原卖家。
    pub first_intermediate_seller_user_id: Option<UserId>,
}

struct HistoryTerminal {
    handoff_from: Option<HistoryAliasHandoff>,
    seller_user_id: Option<UserId>,
    holder_user_id: Option<UserId>,
}

/// 判定登记人能否通过拍卖历史门。登记和解析发布时，两种挂单模式均调用本函数。
///
/// 调用方须在同一数据库快照内读取完整别名集合，每条装备行保留一份摘要，包括无 lot 的行。
/// 合同第 4.2 节的 `public.consignment_eid_alias(text)` 加世界等值限定相同集合，公开读和补扫
/// 用该集合检测登记基线外的新 lot。
pub fn check_listing_history(
    identity: &ListingEquipmentIdentity,
    registrant_user_id: UserId,
    aliases: &[HistoryAliasEvidence],
) -> Result<(), ListingError> {
    // 不变量 6：身份和实物责任有疑义时失败关闭，历史为空仅表示可以继续预检。
    if registrant_user_id.is_nil() {
        return Err(ListingError::HistoryUnproven);
    }

    let mut terminals = BTreeMap::new();
    let mut busy = false;
    let mut unproven = false;
    for alias in aliases {
        if alias.has_conflicting_original_equipment {
            return Err(ListingError::HistoryAliasConflict);
        }
        busy |= alias.has_active_auction
            || alias.has_unsettled_transfer
            || alias.has_unknown_dispatch
            || alias.has_open_equipment_case;
        let Some(lot) = alias.latest_lot.filter(|lot| !lot.lot_id.is_nil()) else {
            unproven = true;
            continue;
        };
        let holder = match lot.holder(identity) {
            Ok(holder) => Some(holder),
            Err(ListingError::EquipmentBusy) => {
                busy = true;
                None
            }
            Err(ListingError::HistoryUnproven) => {
                unproven = true;
                None
            }
            Err(error) => return Err(error),
        };
        let terminal = HistoryTerminal {
            handoff_from: lot.handoff_from,
            seller_user_id: lot.seller_user_id,
            holder_user_id: holder,
        };
        if terminals.insert(lot.lot_id, terminal).is_some() {
            return Err(ListingError::HistoryAliasConflict);
        }
    }

    let holder = terminal_holder(&terminals)?;
    // 有尚未清偿的实物责任时，既有已证持有人不足以授权新登记。
    if busy {
        return Err(ListingError::EquipmentBusy);
    }
    if unproven {
        return Err(ListingError::HistoryUnproven);
    }
    if holder.is_some_and(|holder| holder != registrant_user_id) {
        return Err(ListingError::HistoryOwnerMismatch);
    }
    Ok(())
}

impl HistoryLotEvidence {
    fn holder(self, identity: &ListingEquipmentIdentity) -> Result<UserId, ListingError> {
        let (required_kind, expected_recipient) = match self.outcome {
            HistoryLotOutcome::Pending | HistoryLotOutcome::Live => {
                return Err(ListingError::EquipmentBusy);
            }
            HistoryLotOutcome::Sold { winner_user_id } => {
                let kind = if identity.world_id() == "main" {
                    SettlementObligationKind::WinnerEquipmentCod
                } else {
                    SettlementObligationKind::WinnerEquipmentHandoff
                };
                (kind, winner_user_id)
            }
            HistoryLotOutcome::Unsold => {
                (SettlementObligationKind::UnsoldReturn, self.seller_user_id)
            }
            // 现有拍卖另有撤拍寄回义务，合同只给成交与流拍定义了放行证据。
            HistoryLotOutcome::Withdrawn => return Err(ListingError::HistoryUnproven),
        };

        // 流拍也是结算产生的 lot 结局；有证明却没有已结算场次时仍保留责任。
        if self.auction_status != AuctionStatus::Settled {
            return Err(ListingError::EquipmentBusy);
        }
        let transfer = self.original_transfer.ok_or(ListingError::EquipmentBusy)?;
        if transfer.obligation_kind != required_kind
            || !transfer.matches_original_lot_and_equipment
            || !transfer.fulfillment_satisfied
            || !transfer.original_delivery_succeeded
            || !transfer.matching_collection_proven
        {
            return Err(ListingError::EquipmentBusy);
        }

        let (Some(recipient), Some(expected_recipient)) = (
            transfer.recipient_user_id.filter(|user_id| !user_id.is_nil()),
            expected_recipient.filter(|user_id| !user_id.is_nil()),
        ) else {
            return Err(ListingError::HistoryUnproven);
        };
        if recipient != expected_recipient {
            return Err(ListingError::EquipmentBusy);
        }
        Ok(recipient)
    }
}

fn terminal_holder(
    terminals: &BTreeMap<LotId, HistoryTerminal>,
) -> Result<Option<UserId>, ListingError> {
    // 旧表没有跨别名的前驱外键。只有经存储层核验的交接才可接续，禁止按时间或 UUID 选择末端。
    let mut superseded = BTreeSet::new();
    for terminal in terminals.values() {
        if let Some(handoff) = terminal.handoff_from {
            let previous =
                terminals.get(&handoff.previous_lot_id).ok_or(ListingError::HistoryUnproven)?;
            let seller = terminal
                .seller_user_id
                .filter(|user_id| !user_id.is_nil())
                .ok_or(ListingError::HistoryUnproven)?;
            let first_seller = handoff.first_intermediate_seller_user_id.unwrap_or(seller);
            if first_seller.is_nil() {
                return Err(ListingError::HistoryUnproven);
            }
            if previous.holder_user_id.is_some_and(|holder| holder != first_seller)
                || !superseded.insert(handoff.previous_lot_id)
            {
                return Err(ListingError::HistoryAliasConflict);
            }
        }
    }

    let mut holder = None;
    let mut reached = 0;
    for (lot_id, terminal) in terminals {
        if superseded.contains(lot_id) {
            continue;
        }
        if let Some(terminal_holder) = terminal.holder_user_id {
            if holder.is_some_and(|holder| holder != terminal_holder) {
                return Err(ListingError::HistoryAliasConflict);
            }
            holder = Some(terminal_holder);
        }
        reached += 1;
        let mut predecessor = terminal.handoff_from;
        // 每个 lot 最多有一个后继，从任何末端回溯都不能进入闭环。
        while let Some(handoff) = predecessor {
            reached += 1;
            predecessor = terminals[&handoff.previous_lot_id].handoff_from;
        }
    }
    if reached != terminals.len() {
        return Err(ListingError::HistoryAliasConflict);
    }
    Ok(holder)
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{
        HistoryAliasEvidence, HistoryAliasHandoff, HistoryLotEvidence, HistoryLotOutcome,
        HistoryTransferEvidence, check_listing_history,
    };
    use crate::consignment::{ListingEquipmentIdentity, ListingError};
    use crate::{AuctionStatus, LotId, SettlementObligationKind, UserId};

    fn user(value: u128) -> UserId {
        UserId::from_uuid(Uuid::from_u128(value))
    }

    fn lot_id(value: u128) -> LotId {
        LotId::from_uuid(Uuid::from_u128(value))
    }

    fn delivered(kind: SettlementObligationKind, recipient: u128) -> HistoryTransferEvidence {
        HistoryTransferEvidence {
            obligation_kind: kind,
            matches_original_lot_and_equipment: true,
            fulfillment_satisfied: true,
            original_delivery_succeeded: true,
            matching_collection_proven: true,
            recipient_user_id: Some(user(recipient)),
        }
    }

    fn sold(lot: u128, seller: u128, winner: u128) -> HistoryLotEvidence {
        HistoryLotEvidence {
            lot_id: lot_id(lot),
            auction_status: AuctionStatus::Settled,
            outcome: HistoryLotOutcome::Sold { winner_user_id: Some(user(winner)) },
            seller_user_id: Some(user(seller)),
            original_transfer: Some(delivered(
                SettlementObligationKind::WinnerEquipmentCod,
                winner,
            )),
            handoff_from: None,
        }
    }

    fn unsold(lot: u128, seller: u128) -> HistoryLotEvidence {
        HistoryLotEvidence {
            lot_id: lot_id(lot),
            auction_status: AuctionStatus::Settled,
            outcome: HistoryLotOutcome::Unsold,
            seller_user_id: Some(user(seller)),
            original_transfer: Some(delivered(SettlementObligationKind::UnsoldReturn, seller)),
            handoff_from: None,
        }
    }

    fn alias(latest_lot: Option<HistoryLotEvidence>) -> HistoryAliasEvidence {
        HistoryAliasEvidence {
            has_active_auction: false,
            has_unsettled_transfer: false,
            has_unknown_dispatch: false,
            has_open_equipment_case: false,
            has_conflicting_original_equipment: false,
            latest_lot,
        }
    }

    fn handoff(previous_lot_id: LotId) -> Option<HistoryAliasHandoff> {
        Some(HistoryAliasHandoff { previous_lot_id, first_intermediate_seller_user_id: None })
    }

    #[test]
    fn no_history_continues_but_an_isolated_historical_equipment_row_does_not()
    -> Result<(), ListingError> {
        let equipment = ListingEquipmentIdentity::new("main", "42")?;
        let registrant = user(2);
        assert_eq!(check_listing_history(&equipment, registrant, &[]), Ok(()));
        assert_eq!(
            check_listing_history(&equipment, user(0), &[]),
            Err(ListingError::HistoryUnproven)
        );
        assert_eq!(
            check_listing_history(&equipment, registrant, &[alias(None)]),
            Err(ListingError::HistoryUnproven)
        );
        assert_eq!(
            check_listing_history(
                &equipment,
                registrant,
                &[alias(Some(sold(5, 1, 2))), alias(None)]
            ),
            Err(ListingError::HistoryUnproven)
        );
        Ok(())
    }

    #[test]
    fn active_or_unresolved_older_responsibilities_block_a_proven_terminal_lot()
    -> Result<(), ListingError> {
        let equipment = ListingEquipmentIdentity::new("main", "42")?;
        let proven = alias(Some(sold(9, 1, 2)));
        let older = alias(Some(sold(5, 1, 2)));
        assert_eq!(check_listing_history(&equipment, user(2), &[older, proven]), Ok(()));
        for blocked in [
            HistoryAliasEvidence { has_active_auction: true, ..older },
            HistoryAliasEvidence { has_unsettled_transfer: true, ..older },
            HistoryAliasEvidence { has_unknown_dispatch: true, ..older },
            HistoryAliasEvidence { has_open_equipment_case: true, ..older },
        ] {
            assert_eq!(
                check_listing_history(&equipment, user(2), &[blocked, proven]),
                Err(ListingError::EquipmentBusy),
                "{blocked:?}"
            );
        }
        for outcome in [HistoryLotOutcome::Pending, HistoryLotOutcome::Live] {
            let active = HistoryLotEvidence { outcome, ..sold(5, 1, 2) };
            assert_eq!(
                check_listing_history(&equipment, user(2), &[alias(Some(active))]),
                Err(ListingError::EquipmentBusy)
            );
        }
        Ok(())
    }

    #[test]
    fn sold_lot_requires_the_worlds_equipment_obligation_instead_of_carrier_payment()
    -> Result<(), ListingError> {
        let main = ListingEquipmentIdentity::new("main", "42")?;
        let isekai = ListingEquipmentIdentity::new("isekai", "42")?;
        let cod = sold(4, 1, 2);
        let handoff = HistoryLotEvidence {
            original_transfer: Some(delivered(SettlementObligationKind::WinnerEquipmentHandoff, 2)),
            ..cod
        };
        let carrier = HistoryLotEvidence {
            original_transfer: Some(delivered(
                SettlementObligationKind::WinnerPaymentCarrierCod,
                2,
            )),
            ..cod
        };
        for (equipment, right, wrong) in [(&main, cod, handoff), (&isekai, handoff, cod)] {
            assert_eq!(check_listing_history(equipment, user(2), &[alias(Some(right))]), Ok(()));
            for wrong in [wrong, carrier] {
                assert_eq!(
                    check_listing_history(equipment, user(2), &[alias(Some(wrong))]),
                    Err(ListingError::EquipmentBusy)
                );
            }
        }
        Ok(())
    }

    #[test]
    fn sold_lot_requires_settlement_original_dispatch_and_matching_collection()
    -> Result<(), ListingError> {
        let equipment = ListingEquipmentIdentity::new("main", "42")?;
        let sold = sold(5, 1, 2);
        let proof = delivered(SettlementObligationKind::WinnerEquipmentCod, 2);
        assert_eq!(check_listing_history(&equipment, user(2), &[alias(Some(sold))]), Ok(()));
        let unsettled = HistoryLotEvidence { auction_status: AuctionStatus::Settling, ..sold };
        assert_eq!(
            check_listing_history(&equipment, user(2), &[alias(Some(unsettled))]),
            Err(ListingError::EquipmentBusy)
        );
        for incomplete in [
            None,
            Some(HistoryTransferEvidence { matches_original_lot_and_equipment: false, ..proof }),
            Some(HistoryTransferEvidence { fulfillment_satisfied: false, ..proof }),
            Some(HistoryTransferEvidence { original_delivery_succeeded: false, ..proof }),
            Some(HistoryTransferEvidence { matching_collection_proven: false, ..proof }),
            Some(HistoryTransferEvidence { recipient_user_id: Some(user(3)), ..proof }),
        ] {
            let lot = HistoryLotEvidence { original_transfer: incomplete, ..sold };
            assert_eq!(
                check_listing_history(&equipment, user(2), &[alias(Some(lot))]),
                Err(ListingError::EquipmentBusy),
                "{incomplete:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn unsold_lot_requires_the_original_seller_equipment_and_collection_proof()
    -> Result<(), ListingError> {
        let equipment = ListingEquipmentIdentity::new("isekai", "42")?;
        let returned = unsold(5, 2);
        let proof = delivered(SettlementObligationKind::UnsoldReturn, 2);
        assert_eq!(check_listing_history(&equipment, user(2), &[alias(Some(returned))]), Ok(()));
        for incomplete in [
            None,
            Some(delivered(SettlementObligationKind::WithdrawnReturn, 2)),
            Some(HistoryTransferEvidence { matches_original_lot_and_equipment: false, ..proof }),
            Some(HistoryTransferEvidence { fulfillment_satisfied: false, ..proof }),
            Some(HistoryTransferEvidence { original_delivery_succeeded: false, ..proof }),
            Some(HistoryTransferEvidence { matching_collection_proven: false, ..proof }),
            Some(HistoryTransferEvidence { recipient_user_id: Some(user(3)), ..proof }),
        ] {
            let lot = HistoryLotEvidence { original_transfer: incomplete, ..returned };
            assert_eq!(
                check_listing_history(&equipment, user(2), &[alias(Some(lot))]),
                Err(ListingError::EquipmentBusy),
                "{incomplete:?}"
            );
        }
        let unsettled = HistoryLotEvidence { auction_status: AuctionStatus::Settling, ..returned };
        assert_eq!(
            check_listing_history(&equipment, user(2), &[alias(Some(unsettled))]),
            Err(ListingError::EquipmentBusy)
        );
        Ok(())
    }

    #[test]
    fn completed_collection_requires_a_uniquely_resolved_local_user() -> Result<(), ListingError> {
        let equipment = ListingEquipmentIdentity::new("main", "42")?;
        let sold = sold(5, 1, 2);
        let returned = unsold(5, 2);
        let proof = delivered(SettlementObligationKind::WinnerEquipmentCod, 2);
        for lot in [sold, returned] {
            assert_eq!(check_listing_history(&equipment, user(2), &[alias(Some(lot))]), Ok(()));
        }
        for lot in [
            HistoryLotEvidence {
                original_transfer: Some(HistoryTransferEvidence {
                    recipient_user_id: None,
                    ..proof
                }),
                ..sold
            },
            HistoryLotEvidence {
                original_transfer: Some(HistoryTransferEvidence {
                    recipient_user_id: Some(user(0)),
                    ..proof
                }),
                ..sold
            },
            HistoryLotEvidence {
                outcome: HistoryLotOutcome::Sold { winner_user_id: None },
                ..sold
            },
            HistoryLotEvidence { seller_user_id: None, ..returned },
        ] {
            assert_eq!(
                check_listing_history(&equipment, user(2), &[alias(Some(lot))]),
                Err(ListingError::HistoryUnproven),
                "{lot:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn withdrawn_lot_does_not_gain_eligibility_from_a_distinct_return_kind()
    -> Result<(), ListingError> {
        let equipment = ListingEquipmentIdentity::new("main", "42")?;
        let withdrawn = alias(Some(HistoryLotEvidence {
            outcome: HistoryLotOutcome::Withdrawn,
            auction_status: AuctionStatus::Cancelled,
            original_transfer: Some(delivered(SettlementObligationKind::WithdrawnReturn, 2)),
            ..unsold(5, 2)
        }));
        assert_eq!(
            check_listing_history(&equipment, user(2), &[withdrawn]),
            Err(ListingError::HistoryUnproven)
        );
        assert_eq!(
            check_listing_history(
                &equipment,
                user(2),
                &[HistoryAliasEvidence { has_unsettled_transfer: true, ..withdrawn }]
            ),
            Err(ListingError::EquipmentBusy)
        );
        Ok(())
    }

    #[test]
    fn proven_holder_allows_only_the_same_user_and_denies_off_platform_transfers()
    -> Result<(), ListingError> {
        let equipment = ListingEquipmentIdentity::new("main", "42")?;
        let history = [alias(Some(sold(5, 1, 2)))];
        assert_eq!(check_listing_history(&equipment, user(2), &history), Ok(()));
        for registrant in [user(1), user(3)] {
            assert_eq!(
                check_listing_history(&equipment, registrant, &history),
                Err(ListingError::HistoryOwnerMismatch)
            );
        }
        Ok(())
    }

    #[test]
    fn independent_alias_terminals_must_agree_even_if_one_matches_registrant()
    -> Result<(), ListingError> {
        let equipment = ListingEquipmentIdentity::new("main", "42")?;
        let first = alias(Some(sold(5, 1, 2)));
        let same_holder = alias(Some(sold(9, 1, 2)));
        let different_holder = alias(Some(sold(9, 2, 3)));
        assert_eq!(check_listing_history(&equipment, user(2), &[first, same_holder]), Ok(()));
        for history in [[first, different_holder], [different_holder, first]] {
            for registrant in [user(2), user(3)] {
                assert_eq!(
                    check_listing_history(&equipment, registrant, &history),
                    Err(ListingError::HistoryAliasConflict)
                );
            }
        }
        Ok(())
    }

    #[test]
    fn contradictory_original_equipment_is_rejected_even_with_matching_terminal_owners()
    -> Result<(), ListingError> {
        let equipment = ListingEquipmentIdentity::new("main", "42")?;
        let first = alias(Some(sold(5, 1, 2)));
        let second = alias(Some(sold(9, 1, 2)));
        assert_eq!(check_listing_history(&equipment, user(2), &[first, second]), Ok(()));
        let conflicting =
            HistoryAliasEvidence { has_conflicting_original_equipment: true, ..second };
        assert_eq!(
            check_listing_history(&equipment, user(2), &[first, conflicting]),
            Err(ListingError::HistoryAliasConflict)
        );
        Ok(())
    }

    #[test]
    fn verified_handoffs_select_the_last_holder_independent_of_row_and_uuid_order()
    -> Result<(), ListingError> {
        let equipment = ListingEquipmentIdentity::new("main", "42")?;
        let first = sold(9, 1, 2);
        let second = HistoryLotEvidence { handoff_from: handoff(first.lot_id), ..sold(5, 2, 3) };
        let last = HistoryLotEvidence { handoff_from: handoff(second.lot_id), ..sold(3, 3, 4) };
        let history = [alias(Some(second)), alias(Some(last)), alias(Some(first))];
        assert_eq!(check_listing_history(&equipment, user(4), &history), Ok(()));
        for registrant in [user(1), user(2), user(3)] {
            assert_eq!(
                check_listing_history(&equipment, registrant, &history),
                Err(ListingError::HistoryOwnerMismatch)
            );
        }

        let bridge = HistoryLotEvidence {
            handoff_from: Some(HistoryAliasHandoff {
                previous_lot_id: first.lot_id,
                first_intermediate_seller_user_id: Some(user(2)),
            }),
            ..last
        };
        assert_eq!(
            check_listing_history(&equipment, user(4), &[alias(Some(bridge)), alias(Some(first))]),
            Ok(())
        );
        Ok(())
    }

    #[test]
    fn a_handoff_requires_an_existing_predecessor_and_the_right_seller() -> Result<(), ListingError>
    {
        let equipment = ListingEquipmentIdentity::new("main", "42")?;
        let first = sold(5, 1, 2);
        let second = HistoryLotEvidence { handoff_from: handoff(first.lot_id), ..sold(9, 2, 3) };
        assert_eq!(
            check_listing_history(&equipment, user(3), &[alias(Some(first)), alias(Some(second))]),
            Ok(())
        );
        for (lot, error) in [
            (
                HistoryLotEvidence { handoff_from: handoff(lot_id(7)), ..second },
                ListingError::HistoryUnproven,
            ),
            (HistoryLotEvidence { seller_user_id: None, ..second }, ListingError::HistoryUnproven),
            (
                HistoryLotEvidence { seller_user_id: Some(user(8)), ..second },
                ListingError::HistoryAliasConflict,
            ),
        ] {
            assert_eq!(
                check_listing_history(&equipment, user(3), &[alias(Some(first)), alias(Some(lot))]),
                Err(error)
            );
        }
        Ok(())
    }

    #[test]
    fn contradictory_handoffs_cannot_fork_or_cycle_even_with_agreeing_endpoints()
    -> Result<(), ListingError> {
        let equipment = ListingEquipmentIdentity::new("main", "42")?;
        let first = sold(5, 3, 2);
        let second = HistoryLotEvidence { handoff_from: handoff(first.lot_id), ..sold(9, 2, 3) };
        assert_eq!(
            check_listing_history(&equipment, user(3), &[alias(Some(first)), alias(Some(second))]),
            Ok(())
        );
        let fork = HistoryLotEvidence { lot_id: lot_id(1), ..second };
        assert_eq!(
            check_listing_history(
                &equipment,
                user(3),
                &[alias(Some(first)), alias(Some(second)), alias(Some(fork))]
            ),
            Err(ListingError::HistoryAliasConflict)
        );
        let cycle = HistoryLotEvidence { handoff_from: handoff(second.lot_id), ..first };
        assert_eq!(
            check_listing_history(&equipment, user(3), &[alias(Some(cycle)), alias(Some(second))]),
            Err(ListingError::HistoryAliasConflict)
        );
        Ok(())
    }
}
