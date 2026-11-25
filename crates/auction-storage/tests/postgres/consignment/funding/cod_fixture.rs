//! COD 收款夹具：isekai 已购成交（P=101、F=5、S=96，载体毛额 G=102）、原付款的真实 Send 与远端付款明细、
//! 旧 opening 的发行分录与可选的 record_only 账户。命令 ID 与摘要取固定字节，请求内容由参数决定。

use auction_application::settlement::{
    CertifyCod, CodLeg, DispatchFreeze, FreshCashObservation, FundingMutation, ReceiptKey,
    RegisterOpeningAttribution,
};
use auction_domain::SettlementWorld;
use auction_domain::consignment::funding::{FundingMode, OpeningSlice};
use auction_domain::consignment::{ListingPrice, ListingQuote};
use auction_time::{Duration, Timestamp};
use sqlx::PgPool;
use uuid::Uuid;

use super::super::contracts::fixture::{
    CONTRACT, Graph, OBLIGATION_1, OBLIGATION_2, seed_isekai_custody,
};
use super::super::deposit::support::succeeds;
use super::super::ledger136::fixture::{
    PAY_ATTEMPT, PAY_DELIVERY, PAY_MID, PAY_ROUND, PRICE, PURCHASE_COMMAND, PURCHASED_AT,
    available_sql, purchase_command, purchase_listing, purchase_position,
};

pub const UID: &str = "7001";
/// 原付款 Send 严格晚于 Sale 冻结的开户边界（2026-01-02）。
const SENT_AT: &str = "2026-01-03T00:00:00Z";
/// 早于开户边界的 Send 时刻：该实收的金额可能已含于 opening，不能再被当作新发行。
const SENT_BEFORE_BOUNDARY: &str = "2026-01-01T12:00:00Z";
/// 载体毛额 G = P + 1。
pub const GROSS: i64 = PRICE + 1;
pub const OPENING_SOURCE: &str = "c6a10000-0000-4000-8000-000000000001";
pub const QUARANTINE: &str = "c6a10000-0000-4000-8000-000000000002";
pub const OPENING_ENTRY: &str = "c6a10000-0000-4000-8000-000000000003";
pub const COMPENSATION: &str = "c6a10000-0000-4000-8000-000000000004";
pub const COMPENSATION_ENTRY: &str = "c6a10000-0000-4000-8000-000000000005";
const EVIDENCE: &str = "c6a20000-0000-4000-8000-000000000001";
const ACTIVATION_COMMAND: &str = "c6a30000-0000-4000-8000-000000000001";

pub fn id(text: &str) -> Uuid {
    Uuid::parse_str(text).expect("夹具 UUID")
}

pub fn mutation(byte: u8) -> FundingMutation {
    // Worker 命令 ID 必须是 UUIDv5（账本约束），由字节确定性派生。
    FundingMutation {
        command_id: Uuid::new_v5(&Uuid::NAMESPACE_OID, &[byte]),
        request_digest: [byte; 32],
    }
}

/// 一次成交的夹具形状。
#[derive(Clone, Copy)]
pub struct Shape {
    /// 账户与 Sale 冻结为 record_only。
    pub record_only: bool,
    /// 原付款的 Send 上有远端付款明细。
    pub remote_payment: bool,
    /// 原付款的 Send 不晚于 Sale 冻结的开户边界。
    pub send_before_boundary: bool,
    /// 季窗的交割界已过（`end_utc` 早于现在）。
    pub season_ended: bool,
}

impl Shape {
    pub const STRICT: Self = Self {
        record_only: false,
        remote_payment: true,
        send_before_boundary: false,
        season_ended: false,
    };
}

/// 旧 opening：本 UID 已含 300（隔离 200 + 补偿 100），各由一笔 opening 发行分录贷入。
fn opening_sql() -> String {
    format!(
        "INSERT INTO public.settlement_funding_sources
            (id, uid, kind, source_key, amount, proof_digest, created_at)
         VALUES ('{OPENING_SOURCE}', '{UID}', 'reconciliation', '{OPENING_SOURCE}', 300,
                 decode(repeat('01', 32), 'hex'), '2026-01-01T00:00:00Z');
         INSERT INTO public.settlement_funding_allocations (id, uid, purpose, amount)
         VALUES ('{QUARANTINE}', '{UID}', 'quarantine', 200),
                ('{COMPENSATION}', '{UID}', 'compensation', 100);
         INSERT INTO public.settlement_funding_entries
            (id, command_id, uid, credit_id, source_id, amount, kind, proof_digest)
         VALUES ('{OPENING_ENTRY}', '{OPENING_SOURCE}', '{UID}', '{QUARANTINE}',
                 '{OPENING_SOURCE}', 200, 'issue', decode(repeat('01', 32), 'hex')),
                ('{COMPENSATION_ENTRY}', '{OPENING_SOURCE}', '{UID}', '{COMPENSATION}',
                 '{OPENING_SOURCE}', 100, 'issue', decode(repeat('01', 32), 'hex'));"
    )
}

/// 原付款腿（isekai 载体 COD）的真实 Send：delivery、Send 尝试、Sent MID 绑定、冻结轮次与尝试，
/// 以及激活后的履约行；`remote_payment` 为真时附上唯一一条远端付款明细。
fn dispatch_sql(shape: Shape) -> String {
    let sent_at = if shape.send_before_boundary { SENT_BEFORE_BOUNDARY } else { SENT_AT };
    let delivery = PAY_DELIVERY;
    let command = format!("c71{}", &delivery[3..]);
    let mut sql = format!(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, fencing_version, attempt_count, activation_command_id,
             activation_request_digest, activated_at, created_at, updated_at)
         VALUES ('{delivery}', '{command}', decode(repeat('c1', 32), 'hex'), 'settlement',
                 '{OBLIGATION_1}', 'FakeSellerTwo', '{{}}'::jsonb, 'prepared', 0, 0,
                 '{ACTIVATION_COMMAND}', decode(repeat('c3', 32), 'hex'), '{PURCHASED_AT}',
                 '{PURCHASED_AT}', '{PURCHASED_AT}');
         INSERT INTO public.delivery_send_attempts
            (delivery_id, fencing_version, command_id, request_digest, world_id, sending_at,
             expectation)
         VALUES ('{delivery}', 1, '{command}', decode(repeat('c1', 32), 'hex'), 'main',
                 '{sent_at}',
                 jsonb_build_object('world_id', 'main', 'recipient', 'FakeSellerTwo',
                     'subject', 'fake subject', 'body_digest',
                     (SELECT jsonb_agg(209) FROM generate_series(1, 32)), 'credits', 1,
                     'cod', {GROSS}, 'equipment_ids', '[]'::jsonb, 'credential', 'null'::jsonb));
         INSERT INTO public.delivery_sent_bindings
            (world_id, external_mail_id, delivery_id, fencing_version,
             expectation_fencing_version)
         VALUES ('main', '{PAY_MID}', '{delivery}', 1, 1);
         INSERT INTO public.consignment_dispatch_rounds
            (contract_id, obligation_id, round, round_id, delivery_id, rights_holder_identity,
             bot_uid, world_id, subject, template, body_digest, round_ref, d0, credits, cod,
             prior_round, authorized_by_intake, authorized_by_consumption)
         VALUES ('{CONTRACT}', '{OBLIGATION_1}', 1, '{PAY_ROUND}', '{delivery}',
                 'FakeSellerTwo', '{UID}', 'main', 'fake subject', 'consignment_cod',
                 decode(repeat('d1', 32), 'hex'), '{PAY_ROUND}', '{PURCHASED_AT}', 1, {GROSS},
                 NULL, NULL, NULL);
         INSERT INTO public.consignment_dispatch_attempts
            (delivery_id, fence, attempt_id, contract_id, obligation_id, round, leg, started_at)
         VALUES ('{delivery}', 1, '{PAY_ATTEMPT}', '{CONTRACT}', '{OBLIGATION_1}', 1, 'carrier',
                 '{sent_at}');
         UPDATE public.settlement_obligation_fulfillments
         SET execution_state = 'enqueued', delivery_id = '{delivery}'
         WHERE obligation_id = '{OBLIGATION_1}';"
    );
    if shape.remote_payment {
        sql += &format!(
            "INSERT INTO public.consignment_dispatch_evidence
                (id, attempt_id, category, remote_observation_id, validator_version, digest,
                 observed_at)
             VALUES ('{EVIDENCE}', '{PAY_ATTEMPT}', 'carrier_payment', 'fake-cod-paid-1',
                     'test-signed-off-validator-v1', decode(repeat('e1', 32), 'hex'), '{sent_at}');"
        );
    }
    sql
}

/// isekai 已购成交的完整种子：代管 → 上架 → 购买（Sale、义务图）→ 原付款 Send → 旧 opening。
pub async fn seed(pool: &PgPool, shape: Shape) {
    let window_end = if shape.season_ended {
        "transaction_timestamp() - interval '1 second'"
    } else {
        "transaction_timestamp() + interval '30 days'"
    };
    seed_isekai_custody(pool, window_end).await;
    succeeds(pool, &available_sql()).await;
    if shape.record_only {
        // record_only 账户先于 Sale 建立；合同图 SQL 里的建账语句因 ON CONFLICT 不再生效。
        succeeds(
            pool,
            &format!(
                "INSERT INTO public.settlement_funding_accounts (uid, status, accounting_mode)
                 VALUES ('{UID}', 'ready', 'record_only');"
            ),
        )
        .await;
    }
    let graph = Graph {
        price: PRICE,
        fee: 5,
        seller_amount: 96,
        payout_amount: 96,
        ..Graph::sale_isekai()
    }
    .sql(PURCHASE_COMMAND);
    let graph =
        if shape.record_only { graph.replace("'strict_cash'", "'record_only'") } else { graph };
    let command = purchase_command().replace("\"world\":\"main\"", "\"world\":\"isekai\"");
    succeeds(pool, &(command + &graph + &purchase_position() + &purchase_listing())).await;
    succeeds(pool, &dispatch_sql(shape)).await;
    if !shape.record_only {
        succeeds(pool, &opening_sql()).await;
    }
}

pub fn receipt_key() -> ReceiptKey {
    ReceiptKey { uid: UID.to_owned(), payment_mail_id: PAY_MID.to_owned(), leg: CodLeg::Carrier }
}

/// 登记 opening 份额：原 opening 发行分录的区间 [start, end)。
pub fn register(byte: u8, slices: &[(&str, i64, i64)]) -> RegisterOpeningAttribution {
    RegisterOpeningAttribution {
        mutation: mutation(byte),
        receipt: receipt_key(),
        slices: slices
            .iter()
            .map(|(entry, start, end)| OpeningSlice {
                entry_id: id(entry),
                start: *start,
                end: *end,
            })
            .collect(),
    }
}

/// 认证命令：isekai 载体腿、P=101、F=5、S=96、毛额 102；`attribution` 为空表示已证全部未含。
pub fn certify(byte: u8, attribution: Option<Uuid>) -> CertifyCod {
    CertifyCod {
        mutation: mutation(byte),
        contract_id: id(CONTRACT),
        payment_obligation_id: id(OBLIGATION_1),
        original_delivery_id: id(PAY_DELIVERY),
        receipt: receipt_key(),
        world: SettlementWorld::Isekai,
        mode: FundingMode::StrictCash,
        price: ListingPrice::new(PRICE).expect("夹具价格"),
        quote: ListingQuote { fee: 5, seller_amount: 96 },
        gross: GROSS,
        freeze: DispatchFreeze {
            prepare_digest: [0xc1; 32],
            activation_digest: [0xc3; 32],
            body_digest: [0xd1; 32],
            equipment_ids: Vec::new(),
        },
        cash: Some(cash(1_000_000)),
        attribution_id: attribution,
    }
}

/// 现在刚观察到的同 UID Main 现金；窗口取一分钟，远小于证据最长时效。
pub fn cash(credits: i64) -> FreshCashObservation {
    let now = Timestamp::now();
    FreshCashObservation {
        uid: UID.to_owned(),
        credits,
        observed_at: now - Duration::seconds(5),
        expires_at: now + Duration::seconds(60),
    }
}

/// 与资金、履约状态相关的可比较快照；失败的认证必须让它逐字不变。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ledger {
    pub sources: i64,
    pub entries: i64,
    pub issued: i64,
    pub receipts: i64,
    pub slice_uses: i64,
    pub funded: i64,
    pub fee_facts: i64,
    /// 收款侧命令的回执行数与审计行数。
    pub receipt_commands: i64,
    pub audits: i64,
    pub allocations: i64,
    pub compensation_pool: i64,
    pub compensation_rows: i64,
    pub payment_state: String,
    pub handoff_state: String,
}

pub async fn ledger(pool: &PgPool) -> Ledger {
    let (
        sources,
        entries,
        issued,
        receipts,
        slice_uses,
        funded,
        fee_facts,
        receipt_commands,
        audits,
        allocations,
        compensation_pool,
        compensation_rows,
    ): (i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.settlement_funding_sources),
                (SELECT count(*) FROM public.settlement_funding_entries),
                (SELECT COALESCE(sum(amount), 0)::bigint FROM public.settlement_funding_entries
                 WHERE kind = 'issue' AND source_id <> $1::uuid),
                (SELECT count(*) FROM public.consignment_cod_receipts),
                (SELECT count(*) FROM public.consignment_opening_slice_uses),
                (SELECT count(*) FROM public.consignment_funded_sales),
                (SELECT count(*) FROM public.consignment_fee_facts),
                (SELECT count(*) FROM public.consignment_commands
                 WHERE operation IN ('certify_cod', 'register_opening_attribution')),
                (SELECT count(*) FROM public.audit_records
                 WHERE action IN ('consignment.certify_cod',
                                  'consignment.register_opening_attribution')),
                (SELECT count(*) FROM public.settlement_funding_allocations),
                (SELECT COALESCE(sum(balance), 0)::bigint FROM public.compensation_pool),
                (SELECT count(*) FROM public.compensation_ledger)",
    )
    .bind(OPENING_SOURCE)
    .fetch_one(pool)
    .await
    .expect("读取资金快照");
    let payment_state = obligation_state(pool, OBLIGATION_1, true).await;
    let handoff_state = obligation_state(pool, OBLIGATION_2, false).await;
    Ledger {
        sources,
        entries,
        issued,
        receipts,
        slice_uses,
        funded,
        fee_facts,
        receipt_commands,
        audits,
        allocations,
        compensation_pool,
        compensation_rows,
        payment_state,
        handoff_state,
    }
}

/// 义务状态：付款义务取履约的执行状态，其余取业务状态。
async fn obligation_state(pool: &PgPool, obligation: &str, fulfillment: bool) -> String {
    let sql = if fulfillment {
        "SELECT execution_state FROM public.settlement_obligation_fulfillments
         WHERE obligation_id = $1::uuid"
    } else {
        "SELECT business_state FROM public.settlement_obligations WHERE id = $1::uuid"
    };
    sqlx::query_scalar(sql).bind(obligation).fetch_one(pool).await.expect("读取义务状态")
}

/// 某用途分配（按合同或 UID 级）的当前金额；不存在为 0。
pub async fn allocation_amount(pool: &PgPool, purpose: &str, contract_bound: bool) -> i64 {
    sqlx::query_scalar(
        "SELECT COALESCE(sum(amount), 0)::bigint FROM public.settlement_funding_allocations
         WHERE uid = $1 AND purpose = $2
           AND (consignment_contract_id IS NOT NULL) = $3",
    )
    .bind(UID)
    .bind(purpose)
    .bind(contract_bound)
    .fetch_one(pool)
    .await
    .expect("读取分配金额")
}

/// 隔离用途（opening 原余额）当前金额。
pub async fn quarantine_amount(pool: &PgPool) -> i64 {
    sqlx::query_scalar(
        "SELECT amount FROM public.settlement_funding_allocations WHERE id = $1::uuid",
    )
    .bind(QUARANTINE)
    .fetch_one(pool)
    .await
    .expect("读取隔离余额")
}
