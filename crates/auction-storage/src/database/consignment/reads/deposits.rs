//! v2 收件状态与本人寄件指引的 PostgreSQL 读取。

use auction_application::consignment::{
    ActiveDepositInstructions, ConsignmentError, ConsignmentFuture, DepositRead, DepositStatus,
    DepositWorldStatus, DepositWorlds, ListingWorld, OwnedDepositInstructions,
    ReservationSecretAad, SealedReservationSecret,
};
use auction_domain::SettlementWorld;
use auction_domain::consignment::ListingState;
use sqlx::Row;

use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::deposit::primitives::{
    current_season, mail_readiness, season_state, service_control,
};
use crate::database::consignment::deposit::repository::DepositRepository;
use crate::database::consignment::reads::orders::catalog_v2::{funding_ready, purchase_ready};

impl DepositRead for DepositRepository {
    fn status(&self) -> ConsignmentFuture<'_, DepositStatus> {
        Box::pin(async move {
            let pool = self.store().pool();
            let mut tx = pool.begin_read_snapshot().await.map_err(storage)?;
            let now: auction_time::Timestamp = sqlx::query_scalar("SELECT clock_timestamp()")
                .fetch_one(&mut *tx)
                .await
                .map_err(storage)?;
            let control = service_control(&mut tx).await.map_err(WriteError::into_application)?;
            let (admitting, worker_ready) = control.as_ref().map_or((false, false), |control| {
                (control.admitting, control.worker_ready_until.is_some_and(|ready| ready > now))
            });
            let main = mail_readiness(&mut tx, SettlementWorld::Main)
                .await
                .map_err(WriteError::into_application)?;
            let isekai = mail_readiness(&mut tx, SettlementWorld::Isekai)
                .await
                .map_err(WriteError::into_application)?;
            let window =
                current_season(&mut tx, now).await.map_err(WriteError::into_application)?;
            let funding = funding_ready(&mut tx).await.map_err(storage)?;
            tx.commit().await.map_err(storage)?;
            let main_ready = main.as_ref().is_some_and(|row| row.fully_ready(now));
            let isekai_ready = isekai.as_ref().is_some_and(|row| row.fully_ready(now));
            let isekai_state = season_state(SettlementWorld::Isekai, window.as_ref(), now);
            // R5-C10：购买能力可计算；验证器签收清单全为 false 的本期恒为 false。
            let main_purchase = purchase_ready(
                ListingWorld::Main,
                main_ready,
                auction_domain::consignment::SeasonState::NotSeasonal,
                funding,
            );
            let isekai_purchase =
                purchase_ready(ListingWorld::Isekai, isekai_ready, isekai_state, funding);
            let registration_ready = admitting && worker_ready;
            let registration_modes = if registration_ready {
                vec![
                    auction_domain::consignment::ListingMode::Direct,
                    auction_domain::consignment::ListingMode::Consigned,
                ]
            } else {
                Vec::new()
            };
            let registration_worlds = if registration_ready {
                vec![SettlementWorld::Main, SettlementWorld::Isekai]
            } else {
                Vec::new()
            };
            let settings = self.settings();
            Ok(DepositStatus {
                enabled: admitting,
                registration_ready,
                registration_modes,
                registration_worlds,
                fee_bps: settings.fee_bps,
                min_price: settings.min_price,
                max_price: settings.max_price,
                accepts_mail: main_ready || isekai_ready,
                purchasable: main_purchase || isekai_purchase,
                worlds: DepositWorlds {
                    main: DepositWorldStatus {
                        mail_ready: main_ready,
                        purchase_ready: main_purchase,
                        season_state: auction_domain::consignment::SeasonState::NotSeasonal,
                        cutoff_at: None,
                        season_end_at: None,
                    },
                    isekai: DepositWorldStatus {
                        mail_ready: isekai_ready,
                        purchase_ready: isekai_purchase,
                        season_state: isekai_state,
                        cutoff_at: window.as_ref().map(|window| window.admission_cutoff_utc),
                        season_end_at: window.as_ref().map(|window| window.end_utc),
                    },
                },
            })
        })
    }

    fn instructions(
        &self,
        seller: auction_domain::UserId,
        listing_id: uuid::Uuid,
    ) -> ConsignmentFuture<'_, Option<OwnedDepositInstructions>> {
        Box::pin(read_instructions(self, seller, listing_id, None))
    }

    fn quarantine(
        &self,
        query: auction_application::consignment::AdminDepositQuery,
    ) -> ConsignmentFuture<'_, Option<auction_application::consignment::AdminDepositDetail>> {
        // 4C 拥有隔离投影；4B 只委托，不伪造空详情。
        Box::pin(crate::database::mail::originals::admin_read::read_quarantine(self, query))
    }
}

/// 按显式时刻读取本人有效预约；`None` 使用数据库当前时刻。
pub(crate) async fn read_instructions(
    repository: &DepositRepository,
    seller: auction_domain::UserId,
    listing_id: uuid::Uuid,
    at: Option<auction_time::Timestamp>,
) -> Result<Option<OwnedDepositInstructions>, ConsignmentError> {
    let pool = repository.store().pool();
    let mut tx = pool.begin_read_snapshot().await.map_err(storage)?;
    let now = match at {
        Some(at) => at,
        None => sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut *tx)
            .await
            .map_err(storage)?,
    };
    let listing =
        sqlx::query("SELECT seller_user_id, state FROM public.consignment_listings WHERE id = $1")
            .bind(listing_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
    let Some(listing) = listing else {
        tx.commit().await.map_err(storage)?;
        return Ok(None);
    };
    // 000138 匿名非寄售挂单没有账户卖家：本人寄件指引只属于账户挂单，其余按“非本人”返回空。
    let listing_seller: Option<uuid::Uuid> = listing.try_get("seller_user_id").map_err(storage)?;
    let listing_state: String = listing.try_get("state").map_err(storage)?;
    if listing_seller != Some(seller.as_uuid()) {
        tx.commit().await.map_err(storage)?;
        return Ok(None);
    }
    if listing_state != ListingState::AwaitingDeposit.as_str() {
        tx.commit().await.map_err(storage)?;
        return Ok(Some(OwnedDepositInstructions::None));
    }
    let reservation = sqlx::query(
        "SELECT r.id, r.world_id, r.generation, r.seller_user_id, r.expires_at,
                r.session_id, s.player_name, secret.ciphertext, secret.nonce,
                secret.encryption_key_id, secret.fingerprint
         FROM public.consignment_deposit_reservations r
         JOIN public.mail_evidence_sessions s ON s.id = r.session_id
         JOIN public.consignment_reservation_secrets secret ON secret.reservation_id = r.id
         WHERE r.listing_id = $1 AND r.state = 'active' AND r.expires_at > $2
           AND secret.retired_at IS NULL
         ORDER BY r.generation DESC LIMIT 1",
    )
    .bind(listing_id)
    .bind(now)
    .fetch_optional(&mut *tx)
    .await
    .map_err(storage)?;
    let Some(reservation) = reservation else {
        tx.commit().await.map_err(storage)?;
        return Ok(Some(OwnedDepositInstructions::None));
    };
    let reservation_id: uuid::Uuid = reservation.try_get("id").map_err(storage)?;
    let generation: i64 = reservation.try_get("generation").map_err(storage)?;
    let seller_user_id: uuid::Uuid = reservation.try_get("seller_user_id").map_err(storage)?;
    let expires_at: auction_time::Timestamp = reservation.try_get("expires_at").map_err(storage)?;
    let world_id: String = reservation.try_get("world_id").map_err(storage)?;
    let bot: String = reservation.try_get("player_name").map_err(storage)?;
    let ciphertext: &[u8] = reservation.try_get("ciphertext").map_err(storage)?;
    let nonce: &[u8] = reservation.try_get("nonce").map_err(storage)?;
    let encryption_key_id: String = reservation.try_get("encryption_key_id").map_err(storage)?;
    let fingerprint: String = reservation.try_get("fingerprint").map_err(storage)?;
    tx.commit().await.map_err(storage)?;
    let Ok(nonce) = <[u8; 12]>::try_from(nonce) else {
        return Err(ConsignmentError::Unavailable);
    };
    let sealed =
        SealedReservationSecret::new(ciphertext.to_vec(), nonce, encryption_key_id, fingerprint)?;
    let aad = ReservationSecretAad {
        reservation_id,
        generation: u64::try_from(generation).ok().ok_or(ConsignmentError::Unavailable)?,
        seller_user_id,
    };
    let body = repository.crypto().open(&sealed, aad)?;
    let world = auction_domain::consignment::parse_settlement_world(&world_id)
        .ok_or(ConsignmentError::Unavailable)?;
    Ok(Some(OwnedDepositInstructions::Active(ActiveDepositInstructions {
        world,
        bot,
        subject: "consign".to_owned(),
        body,
        reservation_id,
        expires_at,
    })))
}

#[cfg(feature = "test-db")]
impl DepositRepository {
    /// 在测试中用固定时刻读取本人指引，用于预约截止等号边界。
    pub async fn test_instructions_at(
        &self,
        seller: auction_domain::UserId,
        listing_id: uuid::Uuid,
        at: auction_time::Timestamp,
    ) -> Result<Option<OwnedDepositInstructions>, ConsignmentError> {
        read_instructions(self, seller, listing_id, Some(at)).await
    }
}

fn storage(error: sqlx::Error) -> ConsignmentError {
    crate::database::consignment::commands::errors::WriteError::from(error).into_application()
}
