//! 内存假实现的读取端口：状态、本人指引与 Admin 隔离详情。

use std::future;

use auction_domain::SettlementWorld;
use auction_domain::consignment::{FeeBps, ListingMode, ListingPrice, ListingState, SeasonState};
use uuid::Uuid;

use super::FakeDepositStore;
use crate::ports::consignment::deposit::{
    ActiveDepositInstructions, AdminDepositAction, AdminDepositDetail, AdminDepositQuery,
    AdminFinalDecision, AdminQuarantineObservation, DepositRead, DepositStatus, DepositWorldStatus,
    DepositWorlds, OwnedDepositInstructions, QuarantineState,
};
use crate::ports::consignment::{ConsignmentError, ConsignmentFuture};

impl DepositRead for FakeDepositStore {
    fn status(&self) -> ConsignmentFuture<'_, DepositStatus> {
        let state = self.state.lock().expect("假实现状态锁");
        let isekai_state = Self::season_for(&state);
        // 生产只把包含当前时刻的 isekai 季窗暴露给公开状态；冻结界只参与季态判定。
        let window = if matches!(isekai_state, SeasonState::Open | SeasonState::Preclose) {
            state.current_season.as_ref().filter(|window| {
                window.world == SettlementWorld::Isekai
                    && window.start_utc <= state.clock
                    && state.clock < window.end_utc
            })
        } else {
            None
        };
        let status = DepositStatus {
            enabled: true,
            registration_ready: true,
            registration_modes: vec![ListingMode::Consigned],
            registration_worlds: vec![SettlementWorld::Main, SettlementWorld::Isekai],
            fee_bps: FeeBps::DEFAULT_CONSIGNED,
            min_price: ListingPrice::new(1).expect("下限"),
            max_price: ListingPrice::new(ListingPrice::MAX).expect("上限"),
            accepts_mail: state.allowlisted,
            purchasable: false,
            worlds: DepositWorlds {
                main: DepositWorldStatus {
                    mail_ready: state.allowlisted,
                    purchase_ready: false,
                    season_state: SeasonState::NotSeasonal,
                    cutoff_at: None,
                    season_end_at: None,
                },
                isekai: DepositWorldStatus {
                    mail_ready: state.allowlisted,
                    purchase_ready: false,
                    season_state: isekai_state,
                    cutoff_at: window.map(|window| window.admission_cutoff_utc),
                    season_end_at: window.map(|window| window.end_utc),
                },
            },
        };
        Box::pin(future::ready(Ok(status)))
    }

    fn instructions(
        &self,
        seller: auction_domain::UserId,
        listing_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<OwnedDepositInstructions>> {
        let state = self.state.lock().expect("假实现状态锁");
        let Some(listing) = state.listings.get(&listing_id) else {
            return Box::pin(future::ready(Ok(None)));
        };
        if listing.seller_user_id != seller {
            return Box::pin(future::ready(Ok(None)));
        }
        if listing.state != ListingState::AwaitingDeposit {
            return Box::pin(future::ready(Ok(Some(OwnedDepositInstructions::None))));
        }
        let Some(reservation) = Self::active_reservation(&state, listing_id) else {
            return Box::pin(future::ready(Ok(Some(OwnedDepositInstructions::None))));
        };
        let instructions = OwnedDepositInstructions::Active(ActiveDepositInstructions {
            world: state.authenticated_world,
            bot: "test-bot".to_owned(),
            subject: "consign".to_owned(),
            body: reservation.token.clone(),
            reservation_id: reservation.reservation_id,
            expires_at: reservation.expires_at,
        });
        Box::pin(future::ready(Ok(Some(instructions))))
    }

    fn quarantine(
        &self,
        query: AdminDepositQuery,
    ) -> ConsignmentFuture<'_, Option<AdminDepositDetail>> {
        let state = self.state.lock().expect("假实现状态锁");
        let Some(quarantine) = state.quarantines.get(&query.quarantine_id) else {
            return Box::pin(future::ready(Ok(None)));
        };
        // 与 `read_quarantine` 同序：按 (observed_at, id) 倒序分页；游标必须是本隔离的观察。
        let cursor = match query.cursor {
            Some(cursor_id) => {
                let Some(cursor_observation) = quarantine
                    .observations
                    .iter()
                    .find(|observation| observation.observation_id == cursor_id)
                else {
                    return Box::pin(future::ready(Err(ConsignmentError::InvalidQuery)));
                };
                Some((cursor_observation.observed_at, cursor_id))
            }
            None => None,
        };
        let mut page: Vec<_> = quarantine.observations.iter().collect();
        page.sort_by(|left, right| {
            right
                .observed_at
                .cmp(&left.observed_at)
                .then_with(|| right.observation_id.cmp(&left.observation_id))
        });
        let observations: Vec<AdminQuarantineObservation> = page
            .into_iter()
            .filter(|observation| {
                cursor.is_none_or(|cursor| {
                    (observation.observed_at, observation.observation_id) < cursor
                })
            })
            .take(usize::from(query.limit))
            .map(|observation| AdminQuarantineObservation {
                observation_id: observation.observation_id,
                observed_at: observation.observed_at,
                parser_version: observation.parser_version.clone(),
                digest: observation.digest,
                credits: observation.credits,
                cod: observation.cod,
                returnable: Some(observation.returnable),
                takeable: observation.takeable,
                items: observation.items.clone(),
            })
            .collect();
        let final_decision =
            quarantine.release_receipt.as_ref().map(|receipt| AdminFinalDecision {
                release_id: receipt.release_id,
                decision_id: receipt.decision_id,
                kind: receipt.decision_kind,
                owner: None,
                released_at: quarantine.released_at.unwrap_or(state.clock),
                listing_changes: receipt.listing_changes.clone(),
                obligation_id: receipt.obligation_id,
                return_mail_id: receipt.return_mail_id,
                return_evidence_id: receipt.return_evidence_id,
                recovery_id: receipt.recovery_id,
                recovery_handoff_id: receipt.recovery_handoff_id,
                return_intake_id: receipt.return_intake_id,
                original_round: receipt.original_round,
                take_work_id: receipt.take_work_id,
                take_delivery_id: receipt.take_delivery_id,
            });
        let observation_total = u64::try_from(quarantine.observations.len()).unwrap_or(u64::MAX);
        let detail = AdminDepositDetail {
            quarantine_id: quarantine.quarantine_id,
            version: quarantine.version,
            state: if quarantine.active {
                QuarantineState::Active
            } else {
                QuarantineState::Released
            },
            world: quarantine.mail_key.world(),
            external_mid: quarantine.mail_key.external_mid().to_owned(),
            first_decision_id: quarantine.quarantine_id,
            reason: quarantine.reason,
            observation_total,
            observations,
            allowed_actions: if quarantine.active {
                vec![AdminDepositAction::Recheck, AdminDepositAction::ReleaseQuarantine]
            } else {
                Vec::new()
            },
            final_decision,
        };
        Box::pin(future::ready(Ok(Some(detail))))
    }
}
