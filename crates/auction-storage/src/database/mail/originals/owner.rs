//! 隔离重检的旧拍卖 owner 只读判定：复用旧系统主题判据与本地义务/恢复记录。

use auction_application::consignment::{
    ConsignmentError, ConsignmentFuture, LegacyAuctionOwnerResolver,
};
use auction_application::settlement::{
    SYSTEM_RETURN_SENDER, system_return_matches_obligation,
    system_returned_settlement_obligation_id,
};
use auction_domain::SettlementObligationKind;
use auction_domain::consignment::OriginalMailKey;
use auction_domain::consignment::deposit::{
    EnvelopeObservation, LegacyAuctionOwner, SenderIdentity,
};
use uuid::Uuid;

use super::identity::MailKeyColumns;
use super::observations;
use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;

impl LegacyAuctionOwnerResolver for DatabaseStore {
    fn resolve(
        &self,
        key: &OriginalMailKey,
        observation: &EnvelopeObservation,
    ) -> ConsignmentFuture<'_, Option<LegacyAuctionOwner>> {
        let key = key.clone();
        let observation_id = observation.observation_id;
        // 不变量 6：身份未证明或不是系统退件时不能猜测旧 owner，保持共享隔离。
        if observation.sender_identity != SenderIdentity::Verified || !observation.system_rts {
            return Box::pin(async { Ok(None) });
        }
        Box::pin(async move { resolve(self, &key, observation_id).await })
    }
}

async fn resolve(
    store: &DatabaseStore,
    key: &OriginalMailKey,
    observation_id: uuid::Uuid,
) -> Result<Option<LegacyAuctionOwner>, ConsignmentError> {
    let columns = MailKeyColumns::from_key(key);
    // 调用方传入的观察身份已经绑定本次重读的原信；只查这一行，避免并发追加后误用更新的观察。
    let bound: Option<(Uuid, Option<String>)> = sqlx::query_as(
        "SELECT id, sender FROM public.mail_envelope_observations
         WHERE id = $1 AND bot_uid = $2 AND world_id = $3 AND external_mid = $4",
    )
    .bind(observation_id)
    .bind(columns.bot_uid)
    .bind(columns.world_id)
    .bind(columns.external_mid)
    .fetch_optional(store.pool())
    .await
    .map_err(storage_unavailable)?;
    // 来源证明与旧路由同口径：持久发件人必须恰为 MoogleMail（精确比较、不忽略大小写）。
    let Some((_, Some(sender))) = bound else { return Ok(None) };
    if sender != SYSTEM_RETURN_SENDER {
        return Ok(None);
    }
    let evidence = {
        let mut connection = store.pool().acquire().await.map_err(storage_unavailable)?;
        observations::legacy_return_evidence_for_observation(&mut connection, observation_id)
            .await
            .map_err(WriteError::into_application)?
    };
    let Some(obligation_id) = evidence.obligation_id else { return Ok(None) };
    let Some(subject) = evidence.subject.as_deref() else { return Ok(None) };
    // 主题必须仍是规范结算主题，且主题里的义务 ID 与该行一致。
    if system_returned_settlement_obligation_id(&sender, subject) != Some(obligation_id) {
        return Ok(None);
    }
    let obligation: Option<(String, bool)> = sqlx::query_as(
        "SELECT obligation_kind, contract_id IS NOT NULL FROM public.settlement_obligations
         WHERE id = $1",
    )
    .bind(obligation_id)
    .fetch_optional(store.pool())
    .await
    .map_err(storage_unavailable)?;
    // 不变量 6：寄售来源的义务 ID 不是拍卖 owner。必须在“查不到则交原案件”（E2）之前返回，
    // 否则寄售退件会被当成本地不存在的退款建成拍卖案件。
    if obligation.as_ref().is_some_and(|(_, consignment)| *consignment) {
        return Ok(None);
    }
    let Some((kind, _)) = obligation.as_ref() else {
        // E2：本地不存在的退款/卖款 UUID 仍按规范主题进入原案件；其余（尤其装备交付）
        // 无法区分退件案件与恢复流程，查不到时不猜，保持隔离。
        return Ok(subject_case_owner(subject, obligation_id));
    };
    // 义务行存在但种类无法解析（只可能来自本版闭集之外的未来迁移）：不猜旧 owner。
    let Some(kind) = parse_kind(kind) else { return Ok(None) };
    if !system_return_matches_obligation(&sender, subject, kind, obligation_id) {
        return Ok(None);
    }
    match kind {
        SettlementObligationKind::SellerPayout => Ok(Some(LegacyAuctionOwner::SellerPayout)),
        SettlementObligationKind::BuyerRefund => Ok(Some(LegacyAuctionOwner::BuyerRefund)),
        SettlementObligationKind::UnsoldReturn => Ok(Some(LegacyAuctionOwner::UnsoldReturn)),
        SettlementObligationKind::WithdrawnReturn => Ok(Some(LegacyAuctionOwner::WithdrawnReturn)),
        SettlementObligationKind::WinnerEquipmentCod
        | SettlementObligationKind::WinnerPaymentCarrierCod
        | SettlementObligationKind::WinnerEquipmentHandoff => {
            let recovery_leg = match kind {
                SettlementObligationKind::WinnerEquipmentHandoff => "equipment_obligation_id",
                _ => "payment_obligation_id",
            };
            let matching: i64 = sqlx::query_scalar(match recovery_leg {
                "equipment_obligation_id" => {
                    "SELECT count(*) FROM public.settlement_recoveries
                     WHERE equipment_obligation_id = $1"
                }
                _ => {
                    "SELECT count(*) FROM public.settlement_recoveries
                     WHERE payment_obligation_id = $1"
                }
            })
            .bind(obligation_id)
            .fetch_one(store.pool())
            .await
            .map_err(storage_unavailable)?;
            if matching != 1 {
                return Ok(None);
            }
            Ok(Some(match kind {
                SettlementObligationKind::WinnerEquipmentCod => {
                    LegacyAuctionOwner::WinnerEquipmentCod
                }
                SettlementObligationKind::WinnerPaymentCarrierCod => {
                    LegacyAuctionOwner::WinnerPaymentCarrierCod
                }
                _ => LegacyAuctionOwner::WinnerEquipmentHandoff,
            }))
        }
    }
}

fn storage_unavailable(error: sqlx::Error) -> ConsignmentError {
    // 不变量 6、7：读取失败只投影为不可用；SQLSTATE/约束之外的错误正文不进入调用方。
    WriteError::from(error).into_application()
}

fn subject_case_owner(subject: &str, obligation_id: uuid::Uuid) -> Option<LegacyAuctionOwner> {
    if system_return_matches_obligation(
        SYSTEM_RETURN_SENDER,
        subject,
        SettlementObligationKind::SellerPayout,
        obligation_id,
    ) {
        return Some(LegacyAuctionOwner::SellerPayout);
    }
    if system_return_matches_obligation(
        SYSTEM_RETURN_SENDER,
        subject,
        SettlementObligationKind::BuyerRefund,
        obligation_id,
    ) {
        return Some(LegacyAuctionOwner::BuyerRefund);
    }
    None
}

fn parse_kind(value: &str) -> Option<SettlementObligationKind> {
    Some(match value {
        "winner_equipment_cod" => SettlementObligationKind::WinnerEquipmentCod,
        "winner_payment_carrier_cod" => SettlementObligationKind::WinnerPaymentCarrierCod,
        "winner_equipment_handoff" => SettlementObligationKind::WinnerEquipmentHandoff,
        "seller_payout" => SettlementObligationKind::SellerPayout,
        "buyer_refund" => SettlementObligationKind::BuyerRefund,
        "unsold_return" => SettlementObligationKind::UnsoldReturn,
        "withdrawn_return" => SettlementObligationKind::WithdrawnReturn,
        _ => return None,
    })
}
