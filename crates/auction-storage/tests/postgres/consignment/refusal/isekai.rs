//! isekai 拒收全链路：有效交割窗口、不同合同计数、`delivery_close_at` 与季界边界的裁决，全部经真实存储入口。
//!
//! 合同都经正规写入造出：各自一条完整的寄售收件管线（上架、预约、收件、取得、成交），合同图由
//! `ConsignmentGraphStore::write_graph` 写入，不复制行、不放宽约束、不关闭触发器。

use auction_domain::consignment::refusal::{
    Deliverability, NotCounted, PendingReason, Qualification, RefusalKind,
};
use auction_domain::consignment::{
    ConsignmentContract, ConsignmentContractCore, ConsignmentSalePolicy, FeeBps,
    FundingAccountingMode, ListingPrice, ListingVersion, SaleFundingFreeze, SaleTerms,
    SaleTermsInput, build_consignment_obligations,
};
use auction_domain::{CommandId, ConsignmentContractId, SettlementWorld, UserId};
use auction_storage::{
    DatabaseStore, RefusalOutcome, ReturnEvidence, ReturnFactSubmission, test_write_graph_in_conn,
};
use auction_time::Timestamp;
use sqlx::{AssertSqlSafe, PgPool};
use uuid::Uuid;

use super::super::contracts::fixture::{
    CONTRACT, OBLIGATION_1, OBLIGATION_3, WINDOW, seed_isekai_awaiting,
};
use super::super::deposit::support::{
    ASSET, CREDENTIAL, LISTING, POSITION, REPLAY, SESSION, USER_1, USER_2, accept_listing,
    attempt_start_proof_and_custody, consume_reservation, custody_event, decide_command,
    delivery_and_work, initial_and_final, item_position_claim, listing_awaiting_deposit,
    observation_and_asset, reservation_issued, succeeds,
};
use super::super::ledger136::fixture::{
    PAY_ATTEMPT, PAY_DELIVERY, PAY_MID, PAY_ROUND, PRICE, PURCHASE_COMMAND, PURCHASED_AT,
    REFUND_DELIVERY, REFUND_MID, available_sql, original_dispatch_sql, purchase_command,
    purchase_listing, purchase_position, round_sql, send_sql,
};
use super::fixture::{at, buyer, command, count, good_evidence, intake, intake_sql_at};
use crate::support::TestDatabase;

/// 证据会话在收件之后换成 main 世界的凭据与会话：同类 cookie 全库只许一个 active，
/// 旧的 isekai 凭据先停用（`disabled`），再启用 main 的。
pub(super) const MAIN_SESSION: &str = "c4000000-0000-4000-8000-0000000000a3";
pub(super) const MAIN_CREDENTIAL: &str = "80000000-0000-0000-0000-0000000000a3";

/// 夹具里不随合同复制的全局行：用户、重放密钥、窗口、凭据与会话。
const SHARED_IDS: [&str; 7] =
    [USER_1, USER_2, REPLAY, WINDOW, CREDENTIAL, SESSION, "8d000000-0000-0000-0000-000000000001"];

/// 已购买的 isekai 合同及其付款载体义务。
pub(super) struct Seeded {
    /// 第 k 份合同的 (合同 ID, 付款载体义务 ID, 首派 delivery, 首派 Sent MID)。
    pub(super) contracts: Vec<(String, String, String, String)>,
}

/// 把夹具文本改写成第 `k` 份合同的数据：UUID 最后一段的首位 0 改 k（全局行除外），装备、MID 同步区分。
pub(super) fn nth(sql: &str, k: u32) -> String {
    if k == 0 {
        return sql.to_owned();
    }
    let bytes = sql.as_bytes();
    let mut out = String::with_capacity(sql.len());
    let mut index = 0;
    while index < sql.len() {
        let candidate = sql.get(index..index + 36);
        let is_uuid = candidate.is_some_and(|text| {
            text.bytes().enumerate().all(|(position, byte)| match position {
                8 | 13 | 18 | 23 => byte == b'-',
                _ => byte.is_ascii_hexdigit(),
            })
        });
        let boundary = index == 0 || !bytes[index - 1].is_ascii_hexdigit();
        match candidate {
            Some(text) if is_uuid && boundary => {
                if SHARED_IDS.contains(&text) || !text[24..].starts_with('0') {
                    out.push_str(text);
                } else {
                    out.push_str(&format!("{}{k}{}", &text[..24], &text[25..]));
                }
                index += 36;
            }
            _ => {
                let ch = sql[index..].chars().next().expect("字符");
                out.push(ch);
                index += ch.len_utf8();
            }
        }
    }
    let digests = remap_digests(&out, k);
    digests
        .replace("'42'", &format!("'{}'", 42 + k))
        .replace("\"42\"", &format!("\"{}\"", 42 + k))
        .replace("555", &format!("{}55", 5 + k))
        .replace("'9001'", &format!("'{}'", 9001 + 100 * k))
}

/// `repeat('xx', n)` 形式的摘要在不同合同里要互不相同：首个十六进制字符按合同序号平移（k=1 翻转最高位，
/// k=2 平移 4，k=3 平移 12），四份合同以内两两不同。
fn remap_digests(sql: &str, k: u32) -> String {
    let marker = "repeat('";
    let mut out = String::with_capacity(sql.len());
    let mut rest = sql;
    while let Some(position) = rest.find(marker) {
        let (before, after) = rest.split_at(position + marker.len());
        out.push_str(before);
        let first = after.chars().next().and_then(|digit| digit.to_digit(16));
        match first {
            Some(digit) => {
                out.push(
                    char::from_digit((digit + 8 * k + 4 * (k / 2)) % 16, 16).expect("十六进制位"),
                );
                rest = &after[1..];
            }
            None => rest = after,
        }
    }
    out.push_str(rest);
    out
}

fn uuid(text: &str) -> Uuid {
    Uuid::parse_str(text).expect("夹具 UUID")
}

/// 第 `k` 份 isekai 合同的收件管线（接受、取得）与上架，复用 5M 夹具的写法。
async fn seed_isekai_position(owner: &PgPool, k: u32) {
    if k == 0 {
        seed_isekai_awaiting(owner, "transaction_timestamp() + interval '10 days'").await;
    } else {
        let accepted = listing_awaiting_deposit()
            + &reservation_issued()
            + &observation_and_asset()
            + &decide_command()
            + &initial_and_final()
            + &item_position_claim()
            + &consume_reservation()
            + &accept_listing()
            + &delivery_and_work()
            + &custody_event();
        let accepted =
            accepted.replace("'main'", "'isekai'").replace("|main|", "|isekai|").replace(
                " 'isekai', '42',\n                 NULL, 'awaiting_take'",
                &format!(" 'isekai', '42',\n                 '{WINDOW}', 'awaiting_take'"),
            );
        succeeds(owner, &nth(&accepted, k)).await;
    }
    succeeds(owner, &nth(&attempt_start_proof_and_custody(), k)).await;
    succeeds(owner, &nth(&available_sql(), k)).await;
}

/// 第 `k` 份合同的核心与成交条款；isekai 冻结阈值由调用方给出（本文件取策略允许的最小值 2，两份合同即可凑够）。
fn sale_contract(k: u32, refusal_limit: u8) -> ConsignmentContract {
    let id = |text: &str| uuid(&nth(text, k));
    let core = ConsignmentContractCore {
        contract_id: ConsignmentContractId::from_uuid(id(CONTRACT)),
        origin_command_id: CommandId::from_uuid(id(PURCHASE_COMMAND)),
        listing_id: id(LISTING),
        position_id: id(POSITION),
        generation: 1,
        custody_equipment_id: id(ASSET),
        world: SettlementWorld::Isekai,
        canonical_eid: (42 + k).to_string(),
        season_window_id: Some(uuid(WINDOW)),
        seller_user_id: UserId::from_uuid(uuid(USER_1)),
        frozen_seller_identity: "FakeSellerOne".to_owned(),
        created_at: at("2026-01-02T00:00:00Z"),
    };
    let terms = SaleTerms::new(SaleTermsInput {
        buyer_user_id: UserId::from_uuid(uuid(USER_2)),
        frozen_buyer_identity: "FakeSellerTwo".to_owned(),
        listing_version: ListingVersion::new(4).expect("版本"),
        price: ListingPrice::new(101).expect("标价"),
        fee_bps: FeeBps::new(500).expect("费率"),
        payment_deadline_at: at("2026-01-09T00:00:00Z"),
        policy: ConsignmentSalePolicy::new(7, 3, refusal_limit).expect("策略"),
        funding: SaleFundingFreeze {
            uid: "7001".to_owned(),
            mode: FundingAccountingMode::StrictCash,
            boundary_at: at("2026-01-02T00:00:00Z"),
            boundary_source_id: None,
            account_version: 0,
        },
    })
    .expect("成交条款");
    ConsignmentContract::Sale { core, terms }
}

/// 造出 `count` 份已成交 isekai 合同：收件管线、成交命令、合同图写入、position 与挂单转移、付款载体首派。
async fn seed_isekai_purchased(owner: &PgPool, count: u32) -> Seeded {
    seed_isekai_purchased_with(owner, count, 2, false).await
}

/// 付款载体首派按冻结的载体形状写入：main 派发、1 Credit、COD P+1、载体尝试；资金端口据此复核轮与义务。
fn carrier_dispatch_sql(carrier: &str) -> String {
    let cod = PRICE + 1;
    let send = send_sql(PAY_DELIVERY, carrier, PAY_MID, cod);
    let round = round_sql(PAY_ROUND, PAY_DELIVERY, carrier, PAY_ATTEMPT, "carrier");
    let frozen = format!("'{PURCHASED_AT}', 0, {PRICE},");
    assert!(send.contains("'credits', 0,") && round.contains(&frozen), "夹具 SQL 形状变了");
    send.replace("'credits', 0,", "'credits', 1,")
        + &round.replace(&frozen, &format!("'{PURCHASED_AT}', 1, {cod},"))
}

/// 同 [`seed_isekai_purchased`]，冻结 isekai 拒收阈值取 `refusal_limit`；`carrier_round` 为真时首派按载体形状写入，
/// 供资金端口（载体本金的预留、支出与退件回补）使用。
pub(super) async fn seed_isekai_purchased_with(
    owner: &PgPool,
    count: u32,
    refusal_limit: u8,
    carrier_round: bool,
) -> Seeded {
    succeeds(
        owner,
        "INSERT INTO public.settlement_funding_accounts (uid, status)
         VALUES ('7001', 'ready') ON CONFLICT DO NOTHING;",
    )
    .await;
    let mut contracts = Vec::new();
    for k in 0..count {
        seed_isekai_position(owner, k).await;
        let purchase = purchase_command().replace("\"world\":\"main\"", "\"world\":\"isekai\"");
        // 购买事务：回执、合同图、position 与挂单转移同事务提交（挂单转移与回执版本事件的约束要求）。
        let graph =
            build_consignment_obligations(&sale_contract(k, refusal_limit)).expect("义务图");
        let mut tx = owner.begin().await.expect("开启购买事务");
        sqlx::raw_sql(AssertSqlSafe(nth(&purchase, k))).execute(&mut *tx).await.expect("成交回执");
        test_write_graph_in_conn(&mut tx, &graph).await.expect("写入合同图");
        sqlx::raw_sql(AssertSqlSafe(nth(&(purchase_position() + &purchase_listing()), k)))
            .execute(&mut *tx)
            .await
            .expect("position 与挂单转移");
        tx.commit().await.expect("提交购买事务");

        let contract_id = nth(CONTRACT, k);
        let carrier: Uuid = sqlx::query_scalar(
            "SELECT id FROM public.settlement_obligations
             WHERE contract_id = $1::uuid AND obligation_kind = 'winner_payment_carrier_cod'",
        )
        .bind(&contract_id)
        .fetch_one(owner)
        .await
        .expect("付款载体义务");
        let dispatch = if carrier_round {
            carrier_dispatch_sql(&carrier.to_string())
        } else {
            original_dispatch_sql().replace(OBLIGATION_1, &carrier.to_string())
        };
        succeeds(owner, &nth(&dispatch, k)).await;
        contracts.push((
            contract_id,
            carrier.to_string(),
            nth(PAY_DELIVERY, k),
            nth(&format!("'{PAY_MID}'"), k).trim_matches('\'').to_owned(),
        ));
    }
    Seeded { contracts }
}

/// 收件完成后的凭据轮换：停用 isekai 凭据，启用 main 凭据与会话，供退件的原信与 Sent 使用。
pub(super) async fn switch_to_main_mail(owner: &PgPool) {
    succeeds(
        owner,
        &format!(
            "UPDATE public.credential_keys SET status = 'disabled', updated_at = transaction_timestamp()
             WHERE id = '{CREDENTIAL}';
             INSERT INTO public.credential_keys
                (id, credential_kind, scope, label, ciphertext, nonce, encryption_key_id,
                 fingerprint, status, state_version, created_at, updated_at)
             VALUES ('{MAIN_CREDENTIAL}', 'mail_cookie', 'main', 'fake-main', decode('11', 'hex'),
                     decode(repeat('22', 12), 'hex'), 'fake-data-key', repeat('b', 64),
                     'active', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
             INSERT INTO public.mail_evidence_sessions
                (id, bot_uid, world_id, player_name, credential_id, credential_version,
                 runtime_generation, observed_at, protocol_version, binding_digest)
             VALUES ('{MAIN_SESSION}', '12345', 'main', 'FakeBot', '{MAIN_CREDENTIAL}', 1, 1,
                     '2026-01-01T00:00:00Z', 'hv_uid_forum_profile_armory_v1',
                     decode(repeat('34', 32), 'hex'));"
        ),
    )
    .await;
}

/// 把第 n 个接管行建在第 `k` 份合同的付款载体义务上，RTS 发生在 `rts_at`。
pub(super) async fn seed_intake(owner: &PgPool, seeded: &Seeded, k: usize, n: u32, rts_at: &str) {
    let (contract, carrier, delivery, mid) = &seeded.contracts[k];
    let sql = intake_sql_at(&intake(n), rts_at)
        .replace(SESSION, MAIN_SESSION)
        .replace(OBLIGATION_3, carrier)
        .replace(CONTRACT, contract)
        .replace(REFUND_DELIVERY, delivery)
        .replace(REFUND_MID, mid);
    succeeds(owner, &sql).await;
}

/// 数据库当前时刻之前 `days` 天的整分钟（RFC 3339，UTC）。
pub(super) async fn minutes_before_now(owner: &PgPool, days: i32, extra_minutes: i32) -> String {
    sqlx::query_scalar(
        "SELECT to_char(date_trunc('minute', clock_timestamp()
                - make_interval(days => $1, mins => $2)) AT TIME ZONE 'UTC',
                'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"')",
    )
    .bind(days)
    .bind(extra_minutes)
    .fetch_one(owner)
    .await
    .expect("相对时刻")
}

fn submission(n: u32, close: Timestamp, evidence: ReturnEvidence) -> ReturnFactSubmission {
    ReturnFactSubmission {
        intake_id: Uuid::parse_str(&intake(n).intake()).expect("接管行 UUID"),
        kind: RefusalKind::IsekaiUnpaidCarrierReturn,
        proven_at: at("2026-01-01T05:00:00Z"),
        delivery_close_at: Some(close),
        evidence,
    }
}

async fn submit_isekai(
    store: &DatabaseStore,
    n: u32,
    close: Timestamp,
    evidence: ReturnEvidence,
) -> RefusalOutcome {
    store
        .test_refusal_submit(
            command(u128::from(n)),
            &[buyer()],
            &submission(n, close, evidence),
            None,
        )
        .await
        .expect("拒收提交应成功")
}

#[tokio::test]
async fn postgres_consignment_refusal_isekai_window_distinct_contracts_and_delivery_close() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let owner = owner.postgres_pool();
    let store = database.store().await.expect("以运行账号连接");
    let seeded = seed_isekai_purchased(owner, 2).await;
    switch_to_main_mail(owner).await;

    // 第一份合同四个退件：一份在 90 天窗口之外，其余三份在窗口内；第二份合同一份在窗口内。
    let rts = [
        (1, minutes_before_now(owner, 100, 0).await, 0),
        (2, minutes_before_now(owner, 20, 0).await, 0),
        (3, minutes_before_now(owner, 10, 0).await, 0),
        (5, minutes_before_now(owner, 5, 0).await, 1),
    ];
    for (n, rts_at, contract) in &rts {
        seed_intake(owner, &seeded, *contract, *n, rts_at).await;
    }
    // 交割窗口关闭在未来：退件时刻整分钟都早于它，证据可交付即合格。
    let close = at(&minutes_before_now(owner, 0, -60 * 24).await);

    // 窗外的第 1 份自己不计；第 2 份只有窗外的同合同池成员；第 3 份池里是同合同的第 2 份：都不处罚。
    for n in [1, 2, 3] {
        let outcome = submit_isekai(&store, n, close, good_evidence()).await;
        assert_eq!(outcome.qualification, Qualification::Qualified);
        assert!(outcome.sanction.is_none(), "第 {n} 份不应触发处罚");
    }

    // 第二份合同的退件：池里每份合同取窗内最早一份（窗外的第 1 份不得入选），与触发事实凑够阈值 2。
    let sanction = submit_isekai(&store, 5, close, good_evidence())
        .await
        .sanction
        .expect("不同合同的第二份应触发处罚");
    let members: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT member.return_fact_id FROM public.consignment_sanction_members AS member
         JOIN public.consignment_return_facts AS fact ON fact.return_fact_id = member.return_fact_id
         JOIN public.consignment_return_intakes AS intake ON intake.id = fact.intake_id
         WHERE member.sanction_id = $1 ORDER BY intake.rts_returned_at",
    )
    .bind(sanction.sanction_id)
    .fetch_all(owner)
    .await
    .expect("构成项");
    assert_eq!(members.len(), 2);
    let member_intakes: Vec<(String,)> = sqlx::query_as(
        "SELECT intake.id::text FROM public.consignment_sanction_members AS member
         JOIN public.consignment_return_facts AS fact ON fact.return_fact_id = member.return_fact_id
         JOIN public.consignment_return_intakes AS intake ON intake.id = fact.intake_id
         WHERE member.sanction_id = $1 ORDER BY intake.rts_returned_at",
    )
    .bind(sanction.sanction_id)
    .fetch_all(owner)
    .await
    .expect("构成项接管行");
    assert_eq!(
        member_intakes.into_iter().map(|(id,)| id).collect::<Vec<_>>(),
        vec![intake(2).intake(), intake(5).intake()],
        "构成项是窗内最早的第 2 份与触发的第 5 份"
    );

    // 处罚落库的窗口与规则、事实保存的交割关闭界点。
    let (rule, threshold, window_from, window_to): (
        String,
        i16,
        Option<Timestamp>,
        Option<Timestamp>,
    ) = sqlx::query_as(
        "SELECT rule, threshold, window_from, window_to FROM public.consignment_sanctions
             WHERE id = $1",
    )
    .bind(sanction.sanction_id)
    .fetch_one(owner)
    .await
    .expect("处罚");
    assert_eq!(rule, "isekai_unpaid_carrier_returns");
    assert_eq!(threshold, 2);
    let (from, to) = (window_from.expect("窗口起点"), window_to.expect("窗口终点"));
    assert_eq!(to, sanction.t0);
    assert_eq!(to - from, auction_time::Duration::days(90));
    let stored_close: Vec<(Option<Timestamp>,)> =
        sqlx::query_as("SELECT delivery_close_at FROM public.consignment_return_facts")
            .fetch_all(owner)
            .await
            .expect("事实界点");
    assert_eq!(stored_close.len(), 4);
    assert!(stored_close.iter().all(|(stored,)| *stored == Some(close.trunc_subsecs(6))));
    assert_eq!(count(owner, "consignment_account_work").await, 1);
}

#[tokio::test]
async fn postgres_consignment_refusal_isekai_season_boundary_verdicts() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let owner = owner.postgres_pool();
    let store = database.store().await.expect("以运行账号连接");
    let seeded = seed_isekai_purchased(owner, 1).await;
    switch_to_main_mail(owner).await;
    let rts_text = minutes_before_now(owner, 10, 0).await;
    for n in 1..=4 {
        seed_intake(owner, &seeded, 0, n, &rts_text).await;
    }
    let rts = at(&rts_text);
    let after = |seconds: i64| {
        rts.checked_add_signed(auction_time::Duration::seconds(seconds)).expect("时刻")
    };
    let closed = ReturnEvidence { deliverability: Deliverability::Closed, ..good_evidence() };

    // 可交付，但退件时刻不早于季界：待判，不写拒收；同证据重放不追加裁决。
    let at_boundary = submit_isekai(&store, 1, rts, good_evidence()).await;
    assert_eq!(
        at_boundary.qualification,
        Qualification::Pending(PendingReason::DeliverabilityDisputed)
    );
    assert_eq!(count(owner, "consignment_refusals").await, 0);
    let again = submit_isekai(&store, 1, rts, good_evidence()).await;
    assert!(again.replayed);
    assert_eq!(count(owner, "consignment_return_fact_verdicts").await, 1);
    // 之后证清合同已关闭：重裁为不计（终局），仍不写拒收。
    let cleared = submit_isekai(&store, 1, rts, closed).await;
    assert_eq!(cleared.qualification, Qualification::NotCounted(NotCounted::AfterDeliveryClosed));
    assert!(!cleared.replayed);
    assert_eq!(count(owner, "consignment_return_fact_verdicts").await, 2);
    let terminal = submit_isekai(&store, 1, rts, good_evidence()).await;
    assert_eq!(terminal.qualification, Qualification::NotCounted(NotCounted::AfterDeliveryClosed));
    assert!(terminal.replayed);

    // 季界落在退件所在分钟之内：整分钟无法确定先后，待判。
    let straddle = submit_isekai(&store, 2, after(30), good_evidence()).await;
    assert_eq!(
        straddle.qualification,
        Qualification::Pending(PendingReason::BoundaryMinuteAmbiguous)
    );

    // 合同已关闭且退件早于季界：不计。
    let closed_early = submit_isekai(&store, 3, after(600), closed).await;
    assert_eq!(
        closed_early.qualification,
        Qualification::NotCounted(NotCounted::AfterDeliveryClosed)
    );

    // 合同已关闭且退件时刻不早于季界：同样不计（只有可交付且整分钟都早于季界才计入）。
    let closed_late = submit_isekai(&store, 4, rts, closed).await;
    assert_eq!(
        closed_late.qualification,
        Qualification::NotCounted(NotCounted::AfterDeliveryClosed)
    );
    assert_eq!(count(owner, "consignment_refusals").await, 0);
}
