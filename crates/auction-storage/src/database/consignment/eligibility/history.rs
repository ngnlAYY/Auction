//! 从同一快照读取完整的别名与 lot 证据，由领域判定登记资格。

use std::collections::{BTreeMap, BTreeSet};

use auction_domain::consignment::{
    HistoryAliasEvidence, HistoryAliasHandoff, HistoryLotEvidence, HistoryLotOutcome,
    HistoryTransferEvidence, ListingEquipmentIdentity, ListingError, check_listing_history,
};
use auction_domain::{AuctionStatus, LotId, SettlementObligationKind, UserId};
use auction_time::Timestamp;
use sqlx::postgres::PgRow;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::history_query::HISTORY_SQL;

/// 历史检查的完整 lot 集合及每条同身份装备行的领域证据（包括无 lot 的行）。
#[allow(dead_code, reason = "由后续任务 2D 调用")]
pub(crate) struct ListingHistory {
    /// 在调用方的快照中实际存在的 lot UUID。
    pub(crate) lot_ids: Vec<Uuid>,
    /// 每条装备行的历史实物证据。
    pub(crate) aliases: Vec<HistoryAliasEvidence>,
}

/// 在登记或发布事务中读取历史；不会锁定拍卖行，也不会读取装备凭证。
#[allow(dead_code, reason = "由后续任务 2D 调用")]
pub(crate) async fn read_listing_history(
    tx: &mut PgConnection,
    identity: &ListingEquipmentIdentity,
) -> Result<ListingHistory, sqlx::Error> {
    let rows = sqlx::query(HISTORY_SQL)
        .bind(identity.world_id())
        .bind(identity.external_equipment_id())
        .fetch_all(tx)
        .await?;

    let mut aliases = BTreeMap::<Uuid, Alias>::new();
    let mut lot_ids = Vec::new();
    let mut comparable_aliases = BTreeSet::new();
    for row in &rows {
        let equipment_id = row.try_get("equipment_id")?;
        let alias = aliases.entry(equipment_id).or_default();
        alias.conflicting_original = row.try_get("conflicting_original")?;
        if row.try_get("comparable_original")? {
            comparable_aliases.insert(equipment_id);
        }
        if let Some(lot_id) = row.try_get("lot_id")? {
            lot_ids.push(lot_id);
            alias.lots.push(Lot::from_row(row, lot_id)?);
        }
    }

    let handoffs = proven_alias_handoffs(identity, &aliases, &comparable_aliases);
    let aliases = aliases
        .into_iter()
        .map(|(equipment_id, alias)| {
            let handoff = handoffs.as_ref().and_then(|proved| proved.get(&equipment_id)).copied();
            alias.summarize(identity, handoff, handoffs.is_some())
        })
        .collect();
    Ok(ListingHistory { lot_ids, aliases })
}

#[derive(Default)]
struct Alias {
    conflicting_original: bool,
    lots: Vec<Lot>,
}

impl Alias {
    fn summarize(
        mut self,
        identity: &ListingEquipmentIdentity,
        handoff: Option<HistoryAliasHandoff>,
        has_complete_chain: bool,
    ) -> HistoryAliasEvidence {
        self.lots.sort_unstable_by_key(|lot| lot.created_at);
        // 同一装备行的两笔投稿之间可能经另一条别名行交接，用已核实的完整链解释这段间隔。
        let continuous = has_complete_chain
            || self.lots.windows(2).all(|pair| pair[0].hands_off_to(&pair[1], identity));
        let latest_lot = self
            .lots
            .last()
            .filter(|_| continuous)
            .map(|lot| HistoryLotEvidence { handoff_from: handoff, ..lot.evidence });
        HistoryAliasEvidence {
            has_active_auction: self.lots.iter().any(Lot::active),
            has_unsettled_transfer: self.lots.iter().any(|lot| lot.unsettled(identity)),
            has_unknown_dispatch: self.lots.iter().any(|lot| lot.unknown_dispatch),
            has_open_equipment_case: self.lots.iter().any(|lot| lot.open_case),
            has_conflicting_original_equipment: self.conflicting_original,
            latest_lot,
        }
    }
}

struct Lot {
    evidence: HistoryLotEvidence,
    submission_id: Option<Uuid>,
    created_at: Timestamp,
    submitted_at: Option<Timestamp>,
    collected_at: Option<Timestamp>,
    unknown_dispatch: bool,
    open_case: bool,
}

impl Lot {
    fn from_row(row: &PgRow, lot_id: Uuid) -> Result<Self, sqlx::Error> {
        let auction_status = match row.try_get("auction_status")? {
            "draft" => AuctionStatus::Draft,
            "preview" => AuctionStatus::Preview,
            "live" => AuctionStatus::Live,
            "final" => AuctionStatus::Final,
            "settling" => AuctionStatus::Settling,
            "settled" => AuctionStatus::Settled,
            "cancelled" => AuctionStatus::Cancelled,
            _ => return Err(invalid_history("未知的拍卖场次状态")),
        };
        let outcome = match row.try_get("lot_status")? {
            "pending" => HistoryLotOutcome::Pending,
            "live" => HistoryLotOutcome::Live,
            "sold" => HistoryLotOutcome::Sold {
                winner_user_id: row
                    .try_get::<Option<Uuid>, _>("winner_user_id")?
                    .map(UserId::from_uuid),
            },
            "unsold" => HistoryLotOutcome::Unsold,
            "withdrawn" => HistoryLotOutcome::Withdrawn,
            _ => return Err(invalid_history("未知的拍卖 lot 状态")),
        };
        let original_transfer = row
            .try_get::<Option<&str>, _>("obligation_kind")?
            .map(|kind| {
                Ok::<_, sqlx::Error>(HistoryTransferEvidence {
                    obligation_kind: SettlementObligationKind::from_name(kind)
                        .ok_or_else(|| invalid_history("未知的实物履约义务"))?,
                    matches_original_lot_and_equipment: row.try_get("matches_original")?,
                    fulfillment_satisfied: row.try_get("satisfied")?,
                    original_delivery_succeeded: row.try_get("delivery_succeeded")?,
                    matching_collection_proven: row.try_get("matching_collection")?,
                    recipient_user_id: row
                        .try_get::<Option<Uuid>, _>("recipient_user_id")?
                        .map(UserId::from_uuid),
                })
            })
            .transpose()?;
        Ok(Self {
            evidence: HistoryLotEvidence {
                lot_id: LotId::from_uuid(lot_id),
                auction_status,
                outcome,
                seller_user_id: row
                    .try_get::<Option<Uuid>, _>("seller_user_id")?
                    .map(UserId::from_uuid),
                original_transfer,
                handoff_from: None,
            },
            submission_id: row.try_get("submission_id")?,
            created_at: row.try_get("lot_created_at")?,
            submitted_at: row.try_get("submitted_at")?,
            collected_at: row.try_get("collected_at")?,
            unknown_dispatch: row.try_get("unknown_dispatch")?,
            open_case: row.try_get("open_case")?,
        })
    }

    fn active(&self) -> bool {
        matches!(self.evidence.outcome, HistoryLotOutcome::Pending | HistoryLotOutcome::Live)
    }

    fn as_alias(&self) -> HistoryAliasEvidence {
        HistoryAliasEvidence {
            has_active_auction: self.active(),
            has_unsettled_transfer: false,
            has_unknown_dispatch: self.unknown_dispatch,
            has_open_equipment_case: self.open_case,
            has_conflicting_original_equipment: false,
            latest_lot: Some(self.evidence),
        }
    }

    fn unsettled(&self, identity: &ListingEquipmentIdentity) -> bool {
        let recipient =
            self.evidence.original_transfer.and_then(|transfer| transfer.recipient_user_id);
        let verdict = check_listing_history(
            identity,
            recipient.unwrap_or_else(|| UserId::from_uuid(Uuid::nil())),
            &[self.as_alias()],
        );
        matches!(verdict, Err(ListingError::EquipmentBusy))
            || self.evidence.outcome == HistoryLotOutcome::Withdrawn
                && self.evidence.original_transfer.is_some_and(|transfer| {
                    !transfer.fulfillment_satisfied
                        || !transfer.original_delivery_succeeded
                        || !transfer.matching_collection_proven
                })
    }

    fn hands_off_to(&self, next: &Self, identity: &ListingEquipmentIdentity) -> bool {
        // 不变量 6：时间用于排除逆序和重叠，持有人、原件与独立投稿才是交接证据。
        let (Some(seller), Some(collected_at), Some(next_submitted_at)) =
            (next.evidence.seller_user_id, self.collected_at, next.submitted_at)
        else {
            return false;
        };
        self.submission_id.is_some()
            && self.submission_id != next.submission_id
            && self.created_at < next.created_at
            && collected_at < next_submitted_at
            && next_submitted_at <= next.created_at
            && check_listing_history(identity, seller, &[self.as_alias()]).is_ok()
    }
}

fn proven_alias_handoffs(
    identity: &ListingEquipmentIdentity,
    aliases: &BTreeMap<Uuid, Alias>,
    comparable_aliases: &BTreeSet<Uuid>,
) -> Option<BTreeMap<Uuid, HistoryAliasHandoff>> {
    if aliases.len() < 2 || comparable_aliases.len() != aliases.len() {
        return None;
    }
    let mut chain: Vec<_> = aliases
        .iter()
        .flat_map(|(equipment_id, alias)| alias.lots.iter().map(move |lot| (*equipment_id, lot)))
        .collect();
    chain.sort_unstable_by_key(|(_, lot)| lot.created_at);
    // 旧表没有跨别名交接外键。只有可比较的同原件证据和逐手领取→投稿能建立关系，歧义留给领域拒绝。
    if !chain.windows(2).all(|pair| pair[0].1.hands_off_to(pair[1].1, identity)) {
        return None;
    }
    let mut ends = BTreeMap::new();
    for (index, (equipment_id, _)) in chain.iter().enumerate() {
        ends.insert(*equipment_id, index);
    }
    let mut ends: Vec<_> = ends.into_values().collect();
    ends.sort_unstable();
    let handoffs = ends
        .windows(2)
        .map(|pair| {
            let (previous, current) = (pair[0], pair[1]);
            let intermediate = (previous + 1 < current)
                .then(|| chain[previous + 1].1.evidence.seller_user_id)
                .flatten();
            (
                chain[current].0,
                HistoryAliasHandoff {
                    previous_lot_id: chain[previous].1.evidence.lot_id,
                    first_intermediate_seller_user_id: intermediate,
                },
            )
        })
        .collect();
    Some(handoffs)
}

fn invalid_history(message: &'static str) -> sqlx::Error {
    sqlx::Error::Decode(Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, message)))
}
