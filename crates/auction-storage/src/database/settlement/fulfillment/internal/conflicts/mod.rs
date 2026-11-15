//! 排除同 EID/world 的全部本系统占用，关联不全失败关闭；不替旧 unknown 洗白。

use auction_application::settlement::{FundingError, SettlementFulfillmentClaim};
use sqlx::Postgres;
use uuid::Uuid;

use crate::database::TrackedTransaction as Transaction;

mod gate;
mod intake;
mod payload;

pub(crate) use payload::assetless_mail_notification;

struct AssetConflictTarget<'a> {
    kind: auction_domain::SettlementObligationKind,
    obligation_id: auction_domain::SettlementId,
    lot_id: auction_domain::LotId,
    external_equipment_id: &'a str,
    equipment_world: auction_domain::SettlementWorld,
}

pub(super) async fn exclude_asset_conflicts(
    tx: &mut Transaction<'_, Postgres>,
    claim: &SettlementFulfillmentClaim,
) -> Result<(), FundingError> {
    let target = AssetConflictTarget {
        kind: claim.kind,
        obligation_id: claim.obligation_id,
        lot_id: claim.lot_id,
        external_equipment_id: &claim.external_equipment_id,
        equipment_world: claim.equipment_world,
    };
    gate::exclude_conflicts(tx, &target, None).await
}

/// 装备损失仅忽略已证明未外写的原投递；其余入库、恢复与出站冲突复用既有门。
pub(crate) async fn exclude_equipment_loss_conflicts(
    tx: &mut Transaction<'_, Postgres>,
    lot_id: Uuid,
    payment_id: Uuid,
    external_equipment_id: &str,
    delivery_id: Uuid,
) -> Result<(), FundingError> {
    let target = AssetConflictTarget {
        kind: auction_domain::SettlementObligationKind::WinnerEquipmentCod,
        obligation_id: auction_domain::SettlementId::from_uuid(payment_id),
        lot_id: auction_domain::LotId::from_uuid(lot_id),
        external_equipment_id,
        equipment_world: auction_domain::SettlementWorld::Main,
    };
    gate::exclude_conflicts(tx, &target, Some(delivery_id)).await
}
