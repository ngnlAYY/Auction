//! 购买夹具：代管中的 main 件已上架（available 版本 5），可注入的资金预检与首轮内容来源。

use std::sync::{Arc, Mutex};

use auction_application::consignment::{ConsignmentError, PurchaseCommand, RepriceV2Command};
use auction_application::core::RequestHash;
use auction_application::settlement::{
    ConsignmentFundingPreflight, FundingError, FundingFuture, FundingPreflightFact,
    FundingUnavailable,
};
use auction_domain::consignment::funding::FundingMode;
use auction_domain::consignment::purchase::{PurchaseInput, RepriceInput};
use auction_domain::consignment::{ConsignmentSalePolicy, ListingPrice, ListingVersion};
use auction_domain::{CommandId, SettlementWorld, UserId};
use auction_storage::{
    DatabaseStore, FirstDispatchContent, FirstDispatchContentSource, FirstDispatchRequest,
    PurchaseRepository, WithdrawalDispatchRequest,
};
use auction_time::Timestamp;
use sqlx::{AssertSqlSafe, PgPool};
use uuid::Uuid;

use super::super::contracts::fixture::seed_main_custody;
use super::super::deposit::support::{
    ACCEPTED, ASSET, ATTEMPT, DECIDE, DECISION, DELIVERY, DELIVERY_COMMAND, ENVELOPE, ISSUE, ITEM,
    LISTING, OBSERVATION, POSITION, PROOF, PROOF_COMMAND, PUBLISH, REGISTER, RESERVATION, SNAPSHOT,
    USER_1, USER_2, WORK, accept_listing, attempt_start_proof_and_custody, consume_reservation,
    custody_event, decide_command, delivery_and_work, initial_and_final, item_position_claim,
    listing_awaiting_deposit, observation_and_asset, reservation_issued, succeeds,
};
use super::super::refusal::fixture::seed_pending_sanction;
use crate::support::TestDatabase;

/// 第二个买家。
pub(super) const USER_3: &str = "10000000-0000-0000-0000-000000000003";
const AVAILABLE_COMMAND: &str = "c6200000-0000-5000-8000-000000000001";
/// 上架标价与版本。
pub(super) const PRICE: i64 = 101;
pub(super) const VERSION: u64 = 5;

/// 资金预检的可切换结论。
#[derive(Clone, Copy)]
pub(super) enum Funding {
    Ready,
    Unavailable,
    ReadFails,
    /// 严格现金却没有开户边界来源：生产读取器会报 `BoundaryUnproven`，购买必须失败关闭。
    StrictCashNoBoundary,
    /// record_only 没有现金 opening 时允许空来源（R5-C9、迁移 000136），照常成交。
    RecordOnlyNoBoundary,
}

/// 夹具里严格现金账户的开户边界来源（最近一次现金 opening）。
const BOUNDARY_SOURCE: &str = "c6a00000-0000-4000-8000-000000000001";

/// 假预检：按开关返回 Ready、不可用或读取失败；Ready 的 UID 与夹具账户一致。
///
/// 每次被调用时用旁路连接数一次本库的会话咨询锁：购买事务持有命令、用户与实物锁，
/// 预检若在事务内才被调用，数到的就不为零（预检经连接池另取连接，持锁时调用有耗尽连接池的风险）。
pub(super) struct FakePreflight {
    funding: Mutex<Funding>,
    probe: PgPool,
    advisory_locks_seen: Mutex<Vec<i64>>,
}

impl FakePreflight {
    pub(super) fn set(&self, funding: Funding) {
        *self.funding.lock().expect("夹具锁") = funding;
    }

    /// 每次预检被调用时旁路看到的会话咨询锁数。
    pub(super) fn advisory_locks_seen(&self) -> Vec<i64> {
        self.advisory_locks_seen.lock().expect("夹具锁").clone()
    }
}

/// 本库当前被持有的会话咨询锁数（旁路连接，不含自身）。
pub(super) async fn count_advisory_locks(probe: &PgPool) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM pg_locks WHERE locktype = 'advisory'
           AND database = (SELECT oid FROM pg_database WHERE datname = current_database())",
    )
    .fetch_one(probe)
    .await
    .expect("读取咨询锁")
}

impl ConsignmentFundingPreflight for FakePreflight {
    fn preflight(&self, _world: SettlementWorld) -> FundingFuture<'_, FundingPreflightFact> {
        let funding = *self.funding.lock().expect("夹具锁");
        Box::pin(async move {
            let seen = count_advisory_locks(&self.probe).await;
            self.advisory_locks_seen.lock().expect("夹具锁").push(seen);
            let ready = |mode, boundary_source_id| FundingPreflightFact::Ready {
                uid: "7001".to_owned(),
                mode,
                boundary_at: at("2026-01-02T00:00:00Z"),
                boundary_source_id,
                account_version: 0,
            };
            match funding {
                Funding::Ready => Ok(ready(
                    FundingMode::StrictCash,
                    Some(Uuid::parse_str(BOUNDARY_SOURCE).expect("夹具来源")),
                )),
                Funding::StrictCashNoBoundary => Ok(ready(FundingMode::StrictCash, None)),
                Funding::RecordOnlyNoBoundary => Ok(ready(FundingMode::RecordOnly, None)),
                Funding::Unavailable => {
                    Ok(FundingPreflightFact::Unavailable(FundingUnavailable::AccountMissing))
                }
                Funding::ReadFails => Err(FundingError::Storage),
            }
        })
    }
}

/// 假内容来源：明显的假文本与固定摘要，标的装备就是规范装备。
struct FakeContent;

impl FirstDispatchContentSource for FakeContent {
    fn content(
        &self,
        request: &FirstDispatchRequest<'_>,
    ) -> Result<FirstDispatchContent, ConsignmentError> {
        Ok(FirstDispatchContent {
            template: "consignment_cod".to_owned(),
            subject: "fake subject".to_owned(),
            body_digest: [0xd1; 32],
            equipment_ids: vec![request.canonical_eid.to_owned()],
        })
    }

    fn withdrawal_content(
        &self,
        request: &WithdrawalDispatchRequest<'_>,
    ) -> Result<FirstDispatchContent, ConsignmentError> {
        Ok(FirstDispatchContent {
            template: "consignment_equipment_withdrawal".to_owned(),
            subject: "fake withdrawal subject".to_owned(),
            body_digest: [0xd2; 32],
            equipment_ids: vec![request.canonical_eid.to_owned()],
        })
    }
}

pub(super) fn at(text: &str) -> Timestamp {
    Timestamp::parse_from_rfc3339(text).expect("夹具时刻")
}

pub(super) fn user(text: &str) -> UserId {
    UserId::from_uuid(Uuid::parse_str(text).expect("夹具用户"))
}

pub(super) fn listing_id() -> Uuid {
    Uuid::parse_str(LISTING).expect("夹具挂单")
}

/// 玩家 UUIDv4 命令 ID。
pub(super) fn command_id(seed: u128) -> CommandId {
    CommandId::from_uuid(Uuid::from_u128(0xc610_0000_0000_4000_8000_0000_0000_0000 + seed))
}

pub(super) fn digest(seed: u8) -> RequestHash {
    RequestHash::new([seed; 32])
}

pub(super) fn purchase(buyer: &str, seed: u128, hash: u8) -> PurchaseCommand {
    purchase_at(buyer, seed, hash, VERSION, PRICE)
}

pub(super) fn purchase_at(
    buyer: &str,
    seed: u128,
    hash: u8,
    version: u64,
    price: i64,
) -> PurchaseCommand {
    PurchaseCommand {
        command_id: command_id(seed),
        request_hash: digest(hash),
        buyer_user_id: user(buyer),
        input: PurchaseInput {
            listing_id: listing_id(),
            expected_version: ListingVersion::new(version).expect("版本"),
            expected_price: ListingPrice::new(price).expect("价格"),
        },
        request_id: None,
    }
}

pub(super) fn reprice(seed: u128, hash: u8, version: u64, price: i64) -> RepriceV2Command {
    RepriceV2Command {
        command_id: command_id(seed),
        request_hash: digest(hash),
        seller_user_id: user(USER_1),
        input: RepriceInput {
            seller_user_id: user(USER_1),
            listing_id: listing_id(),
            expected_version: ListingVersion::new(version).expect("版本"),
            price: ListingPrice::new(price).expect("价格"),
        },
        request_id: None,
    }
}

pub(super) struct Fixture {
    _database: TestDatabase,
    /// 维护账号：夹具与计数。
    pub(super) owner: DatabaseStore,
    /// 运行账号：被测端口所用。
    pub(super) runtime: DatabaseStore,
    pub(super) preflight: Arc<FakePreflight>,
    pub(super) repository: PurchaseRepository,
    /// 被测的可购挂单；`available` 里就是夹具挂单，`beside_pending_sanction` 里是另一件。
    pub(super) listing: Uuid,
}

/// 第二件可购挂单的 ID：夹具挂单 ID 把末组首位换成 `b`，与已购存量的挂单互不相干。
const SECOND_LISTING: &str = "20000000-0000-0000-0000-b00000000001";

impl Fixture {
    /// 代管中的 main 件已上架，寄售闸门打开，资金预检 Ready。
    pub(super) async fn available() -> Self {
        Self::build(false).await
    }

    /// 在 6D3 的“达阈值待执行处罚”存量（买家 USER_2 已有三份合格拒收、一项 pending 处罚）之外，
    /// 另有一件同卖家的代管中挂单已上架；存量由拒收提交链造出，不手写处罚行。买家是 `BUYER`。
    pub(super) async fn beside_pending_sanction() -> Self {
        Self::build(true).await
    }

    async fn build(sanction_stock: bool) -> Self {
        let database = TestDatabase::create().await.expect("克隆模板库");
        let owner = database.maintenance_store().await.expect("以维护账号连接");
        let runtime = database.store().await.expect("以运行账号连接");
        let pool = owner.postgres_pool();
        if sanction_stock {
            seed_pending_sanction(pool, &runtime).await;
            // 与夹具一样分三批提交：整封接受的延迟校验要求提交时挂单仍是 deposit_accepted。
            for batch in second_listing_sql() {
                succeeds(pool, &batch).await;
            }
        } else {
            seed_main_custody(pool).await;
            succeeds(pool, &available_sql()).await;
        }
        succeeds(
            pool,
            &format!(
                "UPDATE public.consignment_lookup_control
                 SET admitting = true, enabled_epoch = 1,
                     worker_ready_until = clock_timestamp() + interval '1 hour' WHERE id = 1;
                 INSERT INTO public.settlement_funding_accounts (uid, status)
                 VALUES ('7001', 'ready') ON CONFLICT DO NOTHING;
                 INSERT INTO public.settlement_funding_sources
                    (id, uid, kind, source_key, amount, proof_digest)
                 VALUES ('{BOUNDARY_SOURCE}', '7001', 'reconciliation',
                         'c6a00000-0000-4000-8000-0000000000f1', 0, decode(repeat('a1', 32), 'hex'));
                 INSERT INTO public.users (id, username, public_key_hash)
                 VALUES ('{USER_3}', 'FakeBuyerThree', decode(repeat('13', 32), 'hex'));"
            ),
        )
        .await;
        let preflight = Arc::new(FakePreflight {
            funding: Mutex::new(Funding::Ready),
            probe: pool.clone(),
            advisory_locks_seen: Mutex::new(Vec::new()),
        });
        let repository = PurchaseRepository::new(
            runtime.clone(),
            preflight.clone(),
            Arc::new(FakeContent),
            ConsignmentSalePolicy::new(7, 3, 5).expect("策略"),
        );
        let listing = if sanction_stock {
            Uuid::parse_str(SECOND_LISTING).expect("夹具挂单")
        } else {
            listing_id()
        };
        Self { _database: database, owner, runtime, preflight, repository, listing }
    }

    pub(super) fn pool(&self) -> &PgPool {
        self.owner.postgres_pool()
    }

    pub(super) async fn count(&self, table: &str) -> i64 {
        sqlx::query_scalar(AssertSqlSafe(format!("SELECT count(*) FROM public.{table}")))
            .fetch_one(self.pool())
            .await
            .expect("计数")
    }

    /// 挂单的 (状态, 版本, 价格)。
    pub(super) async fn listing(&self) -> (String, i64, i64) {
        sqlx::query_as(
            "SELECT state, state_version, price FROM public.consignment_listings WHERE id = $1",
        )
        .bind(self.listing)
        .fetch_one(self.pool())
        .await
        .expect("读取挂单")
    }

    pub(super) async fn position_state(&self) -> String {
        sqlx::query_scalar(
            "SELECT state FROM public.consignment_custody_positions WHERE listing_id = $1",
        )
        .bind(self.listing)
        .fetch_one(self.pool())
        .await
        .expect("读取 position")
    }
}

/// 取得证明后上架（deposit_accepted -> available 版本 5）的回执、事件与挂单推进。
fn available_sql() -> String {
    let body = format!(
        "{{\"schema_version\":2,\"operation\":\"apply_mail_proof\",
          \"disposition\":\"take_proven\",\"command_id\":\"{AVAILABLE_COMMAND}\",
          \"decision_id\":\"{decision}\",\"proof_id\":\"{proof}\",
          \"listing_changes\":[{{\"listing_id\":\"{LISTING}\",\"listing_version\":5,
          \"state\":\"available\"}}]}}",
        decision = super::super::deposit::support::DECISION,
        proof = super::super::deposit::support::PROOF,
    );
    format!(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_id, listing_id, result_status, result_code,
             result_body, created_at)
         VALUES ('{AVAILABLE_COMMAND}', 2, 'worker', 'fake-worker', 'apply_mail_proof',
                 '{ATTEMPT}', decode(repeat('b1', 32), 'hex'),
                 'original_attempt', '{ATTEMPT}', NULL, 200, 'ok',
                 '{body}', '{ACCEPTED}');
         INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{LISTING}', 5, '{AVAILABLE_COMMAND}', 'worker', 'available',
                 'deposit_accepted', 'available', NULL, {PRICE}, '{ACCEPTED}');
         UPDATE public.consignment_listings
         SET state = 'available', state_version = 5, updated_at = '{ACCEPTED}'
         WHERE id = '{LISTING}';"
    )
}

/// 夹具里的买家（USER_2）；卖家是 USER_1。
pub(super) const BUYER: &str = USER_2;

/// 第二件代管中、已上架（available 版本 5）的挂单：把夹具各步 SQL 里的标识整体换成另一组，
/// 用户、回放密钥与邮件会话沿用。
fn second_listing_sql() -> [String; 3] {
    let envelope = [
        listing_awaiting_deposit(),
        reservation_issued(),
        observation_and_asset(),
        decide_command(),
        initial_and_final(),
        item_position_claim(),
        consume_reservation(),
        accept_listing(),
        delivery_and_work(),
        custody_event(),
    ]
    .concat();
    let batches = [envelope, attempt_start_proof_and_custody(), available_sql()];
    batches.map(|sql| second_listing_batch(&sql))
}

fn second_listing_batch(sql: &str) -> String {
    let mut sql = sql.to_owned();
    for id in [
        LISTING,
        SNAPSHOT,
        REGISTER,
        PUBLISH,
        DECIDE,
        ISSUE,
        RESERVATION,
        ENVELOPE,
        OBSERVATION,
        ASSET,
        DECISION,
        ITEM,
        POSITION,
        DELIVERY,
        DELIVERY_COMMAND,
        WORK,
        ATTEMPT,
        PROOF,
        PROOF_COMMAND,
        AVAILABLE_COMMAND,
        "86000000-0000-0000-0000-000000000001",
    ] {
        sql = sql.replace(id, &shifted(id));
    }
    // 装备编号、邮件编号与远端观察编号也各自唯一。
    let sql = shift_digests(&sql);
    sql.replace("'42'", "'43'")
        .replace("\"42\"", "\"43\"")
        .replace("555", "655")
        .replace("obs-1", "obs-2")
}

/// 把 UUID 末组的首位换成 `b`。
fn shifted(id: &str) -> String {
    let (head, last) = id.split_at(id.len() - 12);
    format!("{head}b{}", &last[1..])
}

/// 把每个 `repeat('xx', n)` 字节摘要的首半字节错开 8，使唯一键上的摘要不与原挂单相撞。
fn shift_digests(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut rest = sql;
    while let Some(at) = rest.find("repeat('") {
        let (head, tail) = rest.split_at(at + "repeat('".len());
        out.push_str(head);
        let mut chars = tail.chars();
        match (chars.next().and_then(|c| c.to_digit(16)), chars.next()) {
            (Some(high), Some(low)) if low.is_ascii_hexdigit() => {
                out.push(char::from_digit((high + 8) % 16, 16).expect("半字节"));
                rest = &tail[1..];
            }
            _ => rest = tail,
        }
    }
    out.push_str(rest);
    out
}
