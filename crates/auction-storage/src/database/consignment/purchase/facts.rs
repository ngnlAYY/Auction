//! 购买与改价的锁内事实读取：挂单、position、处分代、双方账户与外部前置，按固定锁序一次读出。
//!
//! 保护不变量 2：调用方已持有命令、买卖双方用户与规范实物的会话锁并开启 Serializable；
//! 行锁顺序为 双方用户行（UUID 升序）→ 挂单 → position。处分代与合同表对运行账号是只追加表，
//! 不能加行锁，其互斥由 position 行锁与 `(position, generation)` 唯一键承担。

use auction_application::consignment::{ConsignmentError, purchase_funding};
use auction_application::settlement::{FundingError, FundingPreflightFact};
use auction_domain::UserId;
use auction_domain::consignment::deposit::PositionState;
use auction_domain::consignment::purchase::{
    PurchaseFacts, PurchaseFunding, PurchaseListing, PurchaseParty, PurchasePosition,
};
use auction_domain::consignment::{
    FeeBps, FundingAccountingMode, ListingPrice, ListingVersion, parse_settlement_world,
};
use auction_time::Timestamp;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use crate::database::consignment::commands::errors::{WriteError, invalid_projection};
use crate::database::consignment::deposit::primitives::{
    SeasonWindow, active_hold, active_withdrawal_intent, mail_readiness, season_state,
    service_control,
};
use crate::database::consignment::eligibility::accounts::lock_seller_eligibility;
use crate::database::consignment::refusal::buyer_sanction_pending;

/// 锁内读出的挂单行。
pub(super) struct ListingRow {
    pub(super) listing: PurchaseListing,
    pub(super) seller_user_id: Uuid,
    pub(super) service_epoch: i64,
    pub(super) blacklist_count_at_register: i64,
}

/// 锁内读出的 position 行。
pub(super) struct PositionRow {
    pub(super) position_id: Uuid,
    pub(super) state: PositionState,
    pub(super) version: i64,
    pub(super) generation: Option<u64>,
    pub(super) equipment_id: Uuid,
    pub(super) canonical_eid: String,
    pub(super) season_window_id: Option<Uuid>,
}

/// 成交写入需要、而判定不需要的锁内事实。
pub(super) struct WriteContext {
    pub(super) position: PositionRow,
    pub(super) buyer_identity: String,
    pub(super) seller_identity: String,
    pub(super) item_name: String,
}

/// 购买判定的事实快照与成交写入上下文。
pub(super) struct PurchaseLoad {
    pub(super) facts: PurchaseFacts,
    pub(super) context: Option<WriteContext>,
}

/// 锁定并读取挂单；不存在返回 `None`。
pub(super) async fn lock_listing_row(
    connection: &mut PgConnection,
    listing_id: Uuid,
) -> Result<Option<ListingRow>, WriteError> {
    let Some(row) = sqlx::query(
        "SELECT id, seller_user_id, world_id, mode, state, state_version, price, fee_bps,
                pricing_version, service_epoch, blacklist_count_at_register
         FROM public.consignment_listings WHERE id = $1 FOR UPDATE",
    )
    .bind(listing_id)
    .fetch_optional(connection)
    .await?
    else {
        return Ok(None);
    };
    let world: &str = row.try_get("world_id")?;
    let mode: &str = row.try_get("mode")?;
    let state: &str = row.try_get("state")?;
    Ok(Some(ListingRow {
        listing: PurchaseListing {
            listing_id,
            state: state.parse().map_err(invalid_projection)?,
            version: ListingVersion::new(row.try_get::<i64, _>("state_version")?.cast_unsigned())
                .map_err(invalid_projection)?,
            price: ListingPrice::new(row.try_get("price")?).map_err(invalid_projection)?,
            world: parse_settlement_world(world)
                .ok_or_else(|| WriteError::invariant("consignment_purchase_world"))?,
            mode: mode.parse().map_err(invalid_projection)?,
            fee_bps: FeeBps::new(i64::from(row.try_get::<i16, _>("fee_bps")?))
                .map_err(invalid_projection)?,
            pricing_version: u16::try_from(row.try_get::<i16, _>("pricing_version")?)?,
        },
        seller_user_id: row.try_get("seller_user_id")?,
        service_epoch: row.try_get("service_epoch")?,
        blacklist_count_at_register: row.try_get("blacklist_count_at_register")?,
    }))
}

/// 锁定并读取挂单对应的 position；一单至多一个，多于一个是数据损坏，失败关闭。
pub(super) async fn lock_position(
    connection: &mut PgConnection,
    listing_id: Uuid,
) -> Result<Option<PositionRow>, WriteError> {
    let rows = sqlx::query(
        "SELECT id, state, version, current_generation, equipment_id, canonical_eid,
                season_window_id
         FROM public.consignment_custody_positions WHERE listing_id = $1 ORDER BY id FOR UPDATE",
    )
    .bind(listing_id)
    .fetch_all(connection)
    .await?;
    if rows.len() > 1 {
        return Err(WriteError::invariant("consignment_purchase_position_count"));
    }
    let Some(row) = rows.into_iter().next() else {
        return Ok(None);
    };
    let state: &str = row.try_get("state")?;
    Ok(Some(PositionRow {
        position_id: row.try_get("id")?,
        state: PositionState::parse(state)
            .ok_or_else(|| WriteError::invariant("consignment_purchase_position_state"))?,
        version: row.try_get("version")?,
        generation: row
            .try_get::<Option<i64>, _>("current_generation")?
            .map(u64::try_from)
            .transpose()?,
        equipment_id: row.try_get("equipment_id")?,
        canonical_eid: row.try_get("canonical_eid")?,
        season_window_id: row.try_get("season_window_id")?,
    }))
}

/// 当前代是否已被合同消耗、取得是否由整封精确 Take 证明，以及是否存在其他代未封闭的合同
/// （已有封闭事实的旧 Sale 不算：重列后的新代可以再次出售，7C-3）。
pub(super) struct GenerationFacts {
    pub(super) consumed: bool,
    pub(super) take_proven: bool,
    pub(super) other_contract: bool,
}

pub(super) async fn generation_facts(
    connection: &mut PgConnection,
    position_id: Uuid,
    generation: Option<u64>,
) -> Result<GenerationFacts, WriteError> {
    let generation = generation.map(i64::try_from).transpose()?;
    let row = sqlx::query(
        "SELECT
            EXISTS (SELECT 1 FROM public.consignment_contracts
                    WHERE position_id = $1 AND generation = $2) AS consumed,
            EXISTS (SELECT 1 FROM public.consignment_disposition_generations AS generation
                    JOIN public.mail_disposition_proofs AS proof ON proof.id = generation.proof_id
                    WHERE generation.position_id = $1 AND generation.generation = $2
                      AND proof.kind = 'exact_take' AND proof.conclusion = 'take_proven')
                AS take_proven,
            EXISTS (SELECT 1 FROM public.consignment_contracts AS other
                    WHERE other.position_id = $1 AND other.generation IS DISTINCT FROM $2
                      AND NOT EXISTS (
                          SELECT 1 FROM public.consignment_sale_closures AS closure
                          WHERE closure.contract_id = other.contract_id)) AS other_contract",
    )
    .bind(position_id)
    .bind(generation)
    .fetch_one(connection)
    .await?;
    Ok(GenerationFacts {
        consumed: row.try_get("consumed")?,
        take_proven: row.try_get("take_proven")?,
        other_contract: row.try_get("other_contract")?,
    })
}

/// 一个账户的锁内事实；账户行缺失时视为未激活。
#[derive(Clone)]
struct Account {
    active: bool,
    eligible_now: bool,
    blacklist_count: i64,
    username: String,
    game_player_id: Option<String>,
}

async fn lock_account(connection: &mut PgConnection, user_id: Uuid) -> Result<Account, WriteError> {
    let eligibility = lock_seller_eligibility(connection, UserId::from_uuid(user_id)).await?;
    let names: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT username, game_player_id FROM public.users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(connection)
            .await?;
    let (username, game_player_id) = names.unwrap_or_default();
    Ok(match eligibility {
        Some(row) => Account {
            active: row.active,
            eligible_now: row.active && !row.active_blacklist,
            blacklist_count: row.blacklist_count,
            username,
            game_player_id,
        },
        None => Account {
            active: false,
            eligible_now: false,
            blacklist_count: 0,
            username,
            game_player_id,
        },
    })
}

/// 在已持锁的 Serializable 事务内读出购买判定所需的全部事实。
///
/// `now` 是锁后数据库时刻，季态与期限都以它为准。
pub(super) async fn load_purchase(
    connection: &mut PgConnection,
    preflight: &Result<FundingPreflightFact, FundingError>,
    listing: &ListingRow,
    buyer_user_id: Uuid,
    now: Timestamp,
) -> Result<PurchaseLoad, WriteError> {
    let seller_user_id = listing.seller_user_id;
    // 用户行按 UUID 升序锁定，与拒收、解禁入口的互斥次序一致；自买时只锁一次。
    let (buyer, seller) = if buyer_user_id == seller_user_id {
        let account = lock_account(connection, buyer_user_id).await?;
        (account.clone(), account)
    } else if buyer_user_id < seller_user_id {
        let buyer = lock_account(connection, buyer_user_id).await?;
        (buyer, lock_account(connection, seller_user_id).await?)
    } else {
        let seller = lock_account(connection, seller_user_id).await?;
        (lock_account(connection, buyer_user_id).await?, seller)
    };
    let position = lock_position(connection, listing.listing.listing_id).await?;
    let generation = match &position {
        Some(position) => {
            Some(generation_facts(connection, position.position_id, position.generation).await?)
        }
        None => None,
    };
    let control = service_control(connection).await?;
    let world = listing.listing.world;
    let window = season_window(connection, position.as_ref()).await?;
    let (active_hold, withdrawal_intent) = match &position {
        Some(position) => (
            active_hold(connection, position.position_id).await?.is_some(),
            active_withdrawal_intent(connection, position.position_id).await?.is_some(),
        ),
        None => (false, false),
    };
    let buyer_sanction_pending =
        buyer_sanction_pending(connection, UserId::from_uuid(buyer_user_id)).await?;
    let funding = match preflight {
        Ok(fact) => strict_cash_boundary_required(purchase_funding(fact)),
        // 预检读取失败不是确定的业务拒绝：不写回执，调用方以同命令 ID 原样重试。
        Err(error) => {
            tracing::warn!(%error, "寄售购买资金预检读取失败");
            return Err(ConsignmentError::Unavailable.into());
        }
    };
    // 认证 Bot 的识别：就绪行的 Bot UID 与预检冻结的资金 UID 都算；两者都缺时没有成交可言（预检不可用）。
    let mut bot_uids = Vec::new();
    if let Some(readiness) = mail_readiness(connection, world).await? {
        bot_uids.push(readiness.bot_uid);
    }
    if let PurchaseFunding::Ready(freeze) = &funding {
        bot_uids.push(freeze.uid.clone());
    }
    let is_bot = |account: &Account| {
        account.game_player_id.as_ref().is_some_and(|id| bot_uids.iter().any(|uid| uid == id))
    };
    let facts = PurchaseFacts {
        now,
        service_enabled: control.as_ref().is_some_and(|control| control.admitting),
        listing_epoch_current: control
            .as_ref()
            .is_some_and(|control| control.enabled_epoch == listing.service_epoch),
        listing: listing.listing,
        position: position.as_ref().map(|position| PurchasePosition {
            position_id: position.position_id,
            state: position.state,
            generation: position.generation,
            generation_consumed: generation.as_ref().is_some_and(|facts| facts.consumed),
            take_proven: generation.as_ref().is_some_and(|facts| facts.take_proven),
        }),
        buyer: PurchaseParty {
            user_id: UserId::from_uuid(buyer_user_id),
            active: buyer.active,
            eligible: buyer.eligible_now,
            is_authenticated_bot: is_bot(&buyer),
        },
        seller: PurchaseParty {
            user_id: UserId::from_uuid(seller_user_id),
            active: seller.active,
            // 卖家登记后新增的拉黑历史同样取消资格，与取得后上架的判定一致。
            eligible: seller.eligible_now
                && seller.blacklist_count == listing.blacklist_count_at_register,
            is_authenticated_bot: is_bot(&seller),
        },
        season: season_state(world, window.as_ref(), now),
        active_hold,
        buyer_sanction_pending,
        // 保守：取得后的撤回意愿与其他代未封闭的合同（含 Withdrawal）都按“相反未知外写 / 未封闭交割”失败关闭；
        // 其他代的 Sale 只要已有封闭事实即放行（重列由 7C-3 保证旧 Sale 封闭且原件在库）。
        opposite_unknown_write: withdrawal_intent,
        unclosed_delivery: generation.as_ref().is_some_and(|facts| facts.other_contract),
        funding,
    };
    let context = match position {
        Some(position) => Some(WriteContext {
            item_name: item_name(connection, listing).await?,
            position,
            buyer_identity: buyer.username,
            seller_identity: seller.username,
        }),
        None => None,
    };
    Ok(PurchaseLoad { facts, context })
}

/// 严格现金必须有来源：开户边界是最近一次现金 opening，缺来源即边界未证明，按资金不可用失败关闭。
///
/// 只有 record_only 在没有现金 opening 时允许空来源（R5-C9、迁移 000136）。
fn strict_cash_boundary_required(funding: PurchaseFunding) -> PurchaseFunding {
    match funding {
        PurchaseFunding::Ready(freeze)
            if freeze.mode == FundingAccountingMode::StrictCash
                && freeze.boundary_source_id.is_none() =>
        {
            PurchaseFunding::Unavailable
        }
        other => other,
    }
}

/// position 冻结的季窗；没有季窗的 isekai 件季态未知（失败关闭）。
async fn season_window(
    connection: &mut PgConnection,
    position: Option<&PositionRow>,
) -> Result<Option<SeasonWindow>, WriteError> {
    let Some(window_id) = position.and_then(|position| position.season_window_id) else {
        return Ok(None);
    };
    let row = sqlx::query(
        "SELECT id, start_utc, end_utc, admission_cutoff_utc
         FROM public.consignment_season_windows WHERE id = $1",
    )
    .bind(window_id)
    .fetch_optional(connection)
    .await?;
    row.map(|row| {
        Ok(SeasonWindow {
            id: row.try_get("id")?,
            start_utc: row.try_get("start_utc")?,
            end_utc: row.try_get("end_utc")?,
            admission_cutoff_utc: row.try_get("admission_cutoff_utc")?,
        })
    })
    .transpose()
}

/// 邮件主题冻结用的装备名；来自登记时的安全快照。
async fn item_name(
    connection: &mut PgConnection,
    listing: &ListingRow,
) -> Result<String, WriteError> {
    let name: Option<String> = sqlx::query_scalar(
        "SELECT name FROM public.consignment_snapshots WHERE listing_id = $1
         ORDER BY generation DESC LIMIT 1",
    )
    .bind(listing.listing.listing_id)
    .fetch_optional(connection)
    .await?;
    name.ok_or_else(|| WriteError::invariant("consignment_purchase_snapshot_missing"))
}
