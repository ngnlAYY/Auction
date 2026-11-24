//! 根据锁内事实决定整封处理；不读取时钟、数据库或外部服务。

use auction_time::Timestamp;

use super::{
    EnvelopeDecision, EnvelopeItemKind, EnvelopeItemObservation, EnvelopeRoute,
    LockedEnvelopeFacts, MatchedDepositItem, PendingReason, QuarantineReason, ReservationBinding,
    ReservationState, ReturnItem, ReturnItemReason, ReturnManifest, SenderIdentity,
};
use crate::consignment::deposit::errors::DepositError;
use crate::delivery::OriginalMailKey;

/// 根据锁内事实决定整封处理。
///
/// 不变量 1、3、4、6、8：不产生部分接受，确定拒绝与隔离都保留可重放的证据边界。
///
/// 顺序：旧拍卖 owner → 临时读取/身份 → 已有决定 → 主题 → 系统 RTS → 隔离形状 →
/// 空信 → 一一对应且取得就绪的整封接受 → 安全原退 → 隔离。任何一件不满足都不得部分接受。
pub fn decide_envelope(facts: &LockedEnvelopeFacts) -> EnvelopeRoute {
    let observation = &facts.observation;
    if let Some(owner) = observation.legacy_auction_owner {
        return EnvelopeRoute::LegacyAuction(owner);
    }
    if facts.read_failure {
        return EnvelopeRoute::Pending(PendingReason::TransientReadFailure);
    }
    if facts.mail_key.is_none() {
        return EnvelopeRoute::Pending(PendingReason::MissingAuthenticatedUid);
    }
    if facts.already_decided {
        return EnvelopeRoute::Pending(PendingReason::ExistingDecision);
    }
    if observation.system_rts {
        return quarantine(QuarantineReason::SuspectedSystemRts);
    }
    if !observation.subject_is_consign {
        return EnvelopeRoute::Pending(PendingReason::NotConsignSubject);
    }
    if observation.sender_identity == SenderIdentity::Unverified {
        return quarantine(QuarantineReason::IdentityUnproven);
    }
    if observation.sender_identity == SenderIdentity::SameNameDifferentUid {
        return quarantine(QuarantineReason::SameNameChangedUid);
    }
    if !observation.list_detail_consistent {
        return quarantine(QuarantineReason::ListDetailConflict);
    }
    if observation.unknown_attachment
        || observation.items.iter().any(|item| item.kind == EnvelopeItemKind::Unknown)
    {
        return quarantine(QuarantineReason::UnknownAttachment);
    }
    if observation.duplicate_raw_eid {
        return quarantine(QuarantineReason::DuplicateRawEid);
    }
    if observation.canonical_alias_ambiguous {
        return quarantine(QuarantineReason::CanonicalAliasAmbiguous);
    }
    let Some(asset_count) = observation.asset_count else {
        return quarantine(QuarantineReason::AssetCountUnknown);
    };
    if usize::try_from(asset_count).ok() != Some(observation.items.len()) {
        return quarantine(QuarantineReason::AssetCountUnknown);
    }
    if observation.credits.is_none() || observation.cod.is_none() {
        return quarantine(QuarantineReason::AmountUnproven);
    }
    if duplicate_canonical_eid(&observation.items) {
        return quarantine(QuarantineReason::CanonicalAliasAmbiguous);
    }

    let amounts_zero = observation.credits == Some(0) && observation.cod == Some(0);
    if asset_count == 0 {
        if !observation.items.is_empty() {
            return quarantine(QuarantineReason::ListDetailConflict);
        }
        if amounts_zero {
            return EnvelopeRoute::Consignment(EnvelopeDecision::ArchiveEmpty);
        }
        return return_or_quarantine(facts, Vec::new());
    }

    match matched_items(facts) {
        EnvelopeMatch::Accepted(matched) => {
            return EnvelopeRoute::Consignment(EnvelopeDecision::Accept(matched));
        }
        EnvelopeMatch::TakeUnproven => {
            return quarantine(QuarantineReason::TakeUnproven);
        }
        EnvelopeMatch::NotAcceptable => {}
    }
    let reasons = return_items(facts, amounts_zero);
    return_or_quarantine(facts, reasons)
}

fn quarantine(reason: QuarantineReason) -> EnvelopeRoute {
    EnvelopeRoute::Consignment(EnvelopeDecision::Quarantine(reason))
}

fn duplicate_canonical_eid(items: &[EnvelopeItemObservation]) -> bool {
    let mut seen = std::collections::HashSet::new();
    items.iter().filter_map(|item| item.canonical_eid.as_deref()).any(|eid| !seen.insert(eid))
}

/// 整封匹配结果；只有全部逐件条件与取得就绪门都通过才接受。
enum EnvelopeMatch {
    /// 一一对应且可接受。
    Accepted(Vec<MatchedDepositItem>),
    /// 逐件全部可对应，但取得能力或观察未证明；整封隔离。
    TakeUnproven,
    /// 不能整封接受；继续按退回清单分类。
    NotAcceptable,
}

fn matched_items(facts: &LockedEnvelopeFacts) -> EnvelopeMatch {
    let observation = &facts.observation;
    let Some(key) = facts.mail_key.as_ref() else {
        return EnvelopeMatch::NotAcceptable;
    };
    let Some(seller) = facts.envelope_seller_user_id else {
        return EnvelopeMatch::NotAcceptable;
    };
    if observation.credits != Some(0)
        || observation.cod != Some(0)
        || !matches!(observation.sender_identity, SenderIdentity::Verified)
        || observation.items.len() != facts.reservations.len()
    {
        return EnvelopeMatch::NotAcceptable;
    }
    if facts.reservations.iter().any(|reservation| {
        reservation.state != ReservationState::Active
            || reservation.expires_at <= facts.now
            || reservation.seller_user_id != seller
    }) {
        return EnvelopeMatch::NotAcceptable;
    }
    let mut matched = Vec::with_capacity(observation.items.len());
    let mut used_reservations = std::collections::HashSet::new();
    for item in &observation.items {
        if item.kind != EnvelopeItemKind::Equipment
            || !item.tradeable
            || !item.history_ok
            || item.seller_user_id != Some(seller)
            || item.world != Some(key.world())
        {
            return EnvelopeMatch::NotAcceptable;
        }
        let Some(canonical_eid) = item.canonical_eid.as_deref() else {
            return EnvelopeMatch::NotAcceptable;
        };
        let Some(reservation) = facts.reservations.iter().find(|reservation| {
            reservation.equipment.world_id() == crate::delivery::settlement_world_code(key.world())
                && reservation.equipment.external_equipment_id() == canonical_eid
                && reservation.seller_user_id == seller
        }) else {
            return EnvelopeMatch::NotAcceptable;
        };
        if !used_reservations.insert(reservation.reservation_id) {
            return EnvelopeMatch::NotAcceptable;
        }
        matched.push(MatchedDepositItem {
            ordinal: item.ordinal,
            listing_id: reservation.listing_id,
            listing_version: reservation.listing_version,
            seller_user_id: reservation.seller_user_id,
            reservation_id: reservation.reservation_id,
            reservation_generation: reservation.generation,
            equipment: reservation.equipment.clone(),
            credential_ref: item.protected_ref,
        });
    }
    // 不变量 2、4：观察未明确允许 Take，或该世界尚无已签收的取得验证器时，
    // 不得消费预约或产生 Take 工作；整封留隔离，等补证后再定。
    if observation.takeable != Some(true) || !facts.capabilities.take_ready {
        return EnvelopeMatch::TakeUnproven;
    }
    matched.sort_by_key(|item| item.ordinal);
    EnvelopeMatch::Accepted(matched)
}

fn return_items(facts: &LockedEnvelopeFacts, amounts_zero: bool) -> Vec<ReturnItem> {
    let observation = &facts.observation;
    let key = facts.mail_key.as_ref();
    observation
        .items
        .iter()
        .map(|item| {
            let reservation = item.canonical_eid.as_deref().and_then(|eid| {
                facts.reservations.iter().find(|reservation| {
                    reservation.equipment.external_equipment_id() == eid
                        && key.is_some_and(|key| {
                            reservation.equipment.world_id()
                                == crate::delivery::settlement_world_code(key.world())
                        })
                })
            });
            let reason = if item.kind != EnvelopeItemKind::Equipment || !amounts_zero {
                ReturnItemReason::AmountsPresent
            } else if !item.tradeable {
                ReturnItemReason::NotTradeable
            } else if !item.history_ok {
                ReturnItemReason::HistoryUnproven
            } else if item.seller_user_id != facts.envelope_seller_user_id
                || item.world != key.map(OriginalMailKey::world)
            {
                ReturnItemReason::WrongRecipient
            } else if let Some(reservation) = reservation {
                if reservation.state != ReservationState::Active {
                    ReturnItemReason::ReservationNotActive
                } else if reservation.expires_at <= facts.now {
                    ReturnItemReason::ReservationExpired
                } else {
                    ReturnItemReason::ReservationCountMismatch
                }
            } else {
                ReturnItemReason::Unregistered
            };
            ReturnItem { ordinal: item.ordinal, canonical_eid: item.canonical_eid.clone(), reason }
        })
        .collect()
}

fn return_or_quarantine(facts: &LockedEnvelopeFacts, items: Vec<ReturnItem>) -> EnvelopeRoute {
    if facts.observation.returnable && facts.capabilities.safe_original_return {
        let manifest = if items.is_empty() {
            ReturnManifest {
                items: facts
                    .observation
                    .items
                    .iter()
                    .map(|item| ReturnItem {
                        ordinal: item.ordinal,
                        canonical_eid: item.canonical_eid.clone(),
                        reason: ReturnItemReason::AmountsPresent,
                    })
                    .collect(),
            }
        } else {
            ReturnManifest { items }
        };
        EnvelopeRoute::Consignment(EnvelopeDecision::Return(manifest))
    } else {
        quarantine(QuarantineReason::ReturnUnproven)
    }
}

/// 把预约绑定的业务键用于集合排序：世界、规范 EID、预约 UUID。
pub fn reservation_sort_key(reservation: &ReservationBinding) -> (&'static str, &str, uuid::Uuid) {
    (
        reservation.equipment.world_id(),
        reservation.equipment.external_equipment_id(),
        reservation.reservation_id,
    )
}

/// 验证一件预约与一件附件是否严格对应；供隔离解除与测试使用。
pub fn reservation_matches_item(
    reservation: &ReservationBinding,
    item: &EnvelopeItemObservation,
) -> bool {
    matches!(item.kind, EnvelopeItemKind::Equipment)
        && item.canonical_eid.as_deref() == Some(reservation.equipment.external_equipment_id())
        && item.world.is_some_and(|world| {
            crate::delivery::settlement_world_code(world) == reservation.equipment.world_id()
        })
        && reservation.state == ReservationState::Active
}

/// 供应用层把 `DepositError` 留在安全分类内的辅助：预约截止等号即失效。
pub fn reservation_is_live(reservation: &ReservationBinding, now: Timestamp) -> bool {
    reservation.state == ReservationState::Active && now < reservation.expires_at
}

/// 两个原信键是否相同；调用方不能靠显示名或配置名比较。
pub fn same_mail_key(left: &OriginalMailKey, right: &OriginalMailKey) -> bool {
    left == right
}

/// 把列表/详情条数缺失映射为安全错误，供调用方在持久化边界使用。
pub fn require_asset_count(facts: &LockedEnvelopeFacts) -> Result<u32, DepositError> {
    facts.observation.asset_count.ok_or(DepositError::EvidenceUnproven)
}
