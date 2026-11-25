//! 购买状态机与确领证明：购买同事务绑定合同、position 与挂单；买家确领原件才释放实物占用，邮件证明不能代替。

use sqlx::PgPool;

use super::super::contracts::fixture::{CONTRACT, OBLIGATION_1, seed_main_custody};
use super::super::deposit::support::{
    ACCEPTED, ASSET, LISTING, POSITION, PROOF, USER_2, check, fails, succeeds,
};
use super::fixture::{
    AVAILABLE_COMMAND, HANDOFF_COMMAND, HANDOFF_PROOF, PAY_DELIVERY, PRICE, PURCHASED_AT, SOLD_AT,
    available_sql, original_dispatch_sql, purchase_command, purchase_graph, purchase_listing,
    purchase_position, purchase_sql, seed_purchased,
};
use crate::support::TestDatabase;

async fn states(pool: &PgPool) -> (String, String, Option<String>) {
    sqlx::query_as(
        "SELECT listing.state, position.state,
                (SELECT claim.position_id::text FROM public.consignment_equipment_claims AS claim
                 WHERE claim.canonical_eid = '42')
         FROM public.consignment_listings AS listing
         JOIN public.consignment_custody_positions AS position ON position.listing_id = listing.id
         WHERE listing.id = $1::uuid",
    )
    .bind(LISTING)
    .fetch_one(pool)
    .await
    .expect("读取挂单、position 与占用")
}

async fn contract_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM public.consignment_contracts")
        .fetch_one(pool)
        .await
        .expect("读取合同数")
}

#[tokio::test]
async fn postgres_consignment_ledger136_purchase_binds_contract_position_and_listing() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_main_custody(pool).await;
    succeeds(pool, &available_sql()).await;

    // 反例：缺任一同事务事实都整笔回滚，挂单仍 available、没有合同。
    // 只改挂单：没有 Sale 合同与 committed position。
    fails(
        pool,
        &(purchase_command() + &purchase_listing()),
        "23514",
        Some("consignment_listing_transition"),
    )
    .await;
    // 只改 position：没有同事务的 Sale 合同。
    fails(
        pool,
        &(purchase_command() + &purchase_position()),
        "23514",
        Some("c6_position_commit_fact"),
    )
    .await;
    // 有 purchased 回执、合同与 position，但没有挂单事件：回执缺同版本事件。
    fails(
        pool,
        &(purchase_command() + &purchase_graph() + &purchase_position()),
        "23514",
        Some("consignment_command_version_event"),
    )
    .await;
    assert_eq!(contract_count(pool).await, 0);
    assert_eq!(
        states(pool).await,
        ("available".to_owned(), "in_custody".to_owned(), Some(POSITION.to_owned()))
    );

    // 正例：完整购买一次提交。
    succeeds(pool, &purchase_sql()).await;
    assert_eq!(contract_count(pool).await, 1);
    assert_eq!(
        states(pool).await,
        ("sale_pending".to_owned(), "committed".to_owned(), Some(POSITION.to_owned()))
    );
    // sale_pending 不能回到 available，也不能被改价（购买后价格冻结在合同里）。
    fails(
        pool,
        &format!(
            "UPDATE public.consignment_listings SET price = price + 1 WHERE id = '{LISTING}';"
        ),
        "23514",
        Some("consignment_listing_transition"),
    )
    .await;
}

#[tokio::test]
async fn postgres_consignment_ledger136_position_transit_needs_an_asset_leg_attempt() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_purchased(pool).await;
    let transit = format!(
        "UPDATE public.consignment_custody_positions
         SET state = 'in_transit', version = 4, updated_at = '{PURCHASED_AT}'
         WHERE id = '{POSITION}';
         INSERT INTO public.consignment_custody_events
            (position_id, version, command_id, event_kind, from_state, to_state, proof_id,
             listing_id, listing_version, occurred_at)
         VALUES ('{POSITION}', 4, '{AVAILABLE_COMMAND}', 'in_transit', 'committed',
                 'in_transit', NULL, '{LISTING}', 6, '{PURCHASED_AT}');"
    );
    // 反例：没有原件腿的派发尝试，不能进入在途。
    fails(pool, &transit, "23514", Some("c6_position_transit_fact")).await;
    // 正例：同事务写入首派、轮次与资产腿尝试。
    succeeds(pool, &(original_dispatch_sql() + &transit)).await;
    let state: String = sqlx::query_scalar(
        "SELECT state FROM public.consignment_custody_positions WHERE id = $1::uuid",
    )
    .bind(POSITION)
    .fetch_one(pool)
    .await
    .expect("读取 position");
    assert_eq!(state, "in_transit");
}

/// handed_over 事件与占用释放：mail_proof 是邮件证明，handoff_proof 是确领证明（各可为空）。
fn handed_over(mail_proof: Option<&str>, handoff_proof: Option<&str>, last_fact: &str) -> String {
    let quoted =
        |value: Option<&str>| value.map_or_else(|| "NULL".to_owned(), |id| format!("'{id}'"));
    format!(
        "INSERT INTO public.consignment_custody_events
            (position_id, version, command_id, event_kind, from_state, to_state, proof_id,
             handoff_proof_id, listing_id, listing_version, occurred_at)
         VALUES ('{POSITION}', 4, '{HANDOFF_COMMAND}', 'handed_over', 'committed',
                 'handed_over', {proof}, {handoff}, '{LISTING}', 7, '{SOLD_AT}');
         UPDATE public.consignment_equipment_claims
         SET reservation_id = NULL, position_id = NULL, revision = revision + 1,
             changed_at = '{SOLD_AT}', last_fact_id = '{last_fact}'
         WHERE world_id = 'main' AND canonical_eid = '42';",
        proof = quoted(mail_proof),
        handoff = quoted(handoff_proof),
    )
}

fn position_handed_over() -> String {
    format!(
        "UPDATE public.consignment_custody_positions
         SET state = 'handed_over', version = 4, updated_at = '{SOLD_AT}'
         WHERE id = '{POSITION}';"
    )
}

fn handoff_command() -> String {
    format!(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_id, listing_id, result_status, result_code,
             result_body, created_at)
         VALUES ('{HANDOFF_COMMAND}', 2, 'worker', 'fake-worker', 'prove_handoff',
                 '{OBLIGATION_1}', decode(repeat('b3', 32), 'hex'), 'obligation',
                 '{OBLIGATION_1}', NULL, 200, 'ok',
                 '{{\"schema_version\":2,\"operation\":\"prove_handoff\",
                    \"disposition\":\"original_claimed\",\"command_id\":\"{HANDOFF_COMMAND}\",
                    \"contract_id\":\"{CONTRACT}\",\"contract_version\":2,
                    \"obligation_id\":\"{OBLIGATION_1}\",\"proof_id\":\"{HANDOFF_PROOF}\",
                    \"listing_id\":\"{LISTING}\",\"listing_version\":7,
                    \"state\":\"closed\"}}', '{SOLD_AT}');"
    )
}

fn handoff_proof() -> String {
    format!(
        "INSERT INTO public.consignment_handoff_proofs
            (id, position_id, generation, contract_id, obligation_id, delivery_id, fence,
             validator_version, proof_digest, observed_at)
         VALUES ('{HANDOFF_PROOF}', '{POSITION}', 1, '{CONTRACT}', '{OBLIGATION_1}',
                 '{PAY_DELIVERY}', 1, 'v1', decode(repeat('c3', 32), 'hex'), '{SOLD_AT}');"
    )
}

/// 买家已确领原件的全部事实（确领证明、position 终态、释放占用与成交关闭同事务）；
/// 需要先有原件腿首派（`original_dispatch_sql`）。
pub(in crate::consignment) fn buyer_claimed_original_sql() -> String {
    buyer_claimed_original_from("committed", 4)
}

/// 同 `buyer_claimed_original_sql`，但 position 从 `from_state` 推进到版本 `version` 的 handed_over；
/// 供“原件已在途或已被退件消费之后又见确领”这类成对证据的用例在当前 position 版本上追加确领事实。
pub(in crate::consignment) fn buyer_claimed_original_from(
    from_state: &str,
    version: i64,
) -> String {
    let position = format!(
        "UPDATE public.consignment_custody_positions
         SET state = 'handed_over', version = {version},
             updated_at = GREATEST(updated_at, timestamptz '{SOLD_AT}')
         WHERE id = '{POSITION}';
         INSERT INTO public.consignment_custody_events
            (position_id, version, command_id, event_kind, from_state, to_state, proof_id,
             handoff_proof_id, listing_id, listing_version, occurred_at)
         VALUES ('{POSITION}', {version}, '{HANDOFF_COMMAND}', 'handed_over', '{from_state}',
                 'handed_over', NULL, '{HANDOFF_PROOF}', '{LISTING}', 7, '{SOLD_AT}');
         UPDATE public.consignment_equipment_claims
         SET reservation_id = NULL, position_id = NULL, revision = revision + 1,
             changed_at = '{SOLD_AT}', last_fact_id = '{HANDOFF_PROOF}'
         WHERE world_id = 'main' AND canonical_eid = '42';"
    );
    handoff_command() + &handoff_proof() + &position + &listing_sold()
}

fn listing_sold() -> String {
    format!(
        "INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{LISTING}', 7, '{HANDOFF_COMMAND}', 'worker', 'closed', 'sale_pending',
                 'closed', 'sold', {PRICE}, '{SOLD_AT}');
         UPDATE public.consignment_listings
         SET state = 'closed', reason_code = 'sold', state_version = 7, updated_at = '{SOLD_AT}'
         WHERE id = '{LISTING}';"
    )
}

#[tokio::test]
async fn postgres_consignment_ledger136_handoff_proof_releases_the_claim_and_mail_proof_cannot() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_purchased(pool).await;
    succeeds(pool, &original_dispatch_sql()).await;

    // 反例：没有确领证明，position 不能进入 handed_over。
    fails(
        pool,
        &(handoff_command() + &position_handed_over()),
        "23514",
        Some("c6_position_handoff_fact"),
    )
    .await;
    // 反例：handed_over 事件不能引用邮件证明（取得 / 退回的证明），必须是确领证明。
    check(
        pool,
        &(handoff_proof() + &handed_over(Some(PROOF), None, PROOF)),
        "c6_custody_event_handoff_ck",
    )
    .await;
    // 反例：邮件证明与确领证明同时出现也不行（handed_over 只认确领证明）。
    check(
        pool,
        &(handoff_proof() + &handed_over(Some(PROOF), Some(HANDOFF_PROOF), HANDOFF_PROOF)),
        "c6_custody_event_handoff_ck",
    )
    .await;
    // 反例：占用释放的 last_fact 必须是该确领证明。
    fails(
        pool,
        &(handoff_command()
            + &handoff_proof()
            + &position_handed_over()
            + &handed_over(None, Some(HANDOFF_PROOF), POSITION)),
        "23514",
        Some("c6_claim_handoff_fact"),
    )
    .await;
    // 反例：没有 position 终态与确领证明，成交关闭被挂单守卫拒绝。
    fails(
        pool,
        &(handoff_command() + &listing_sold()),
        "23514",
        Some("consignment_listing_transition"),
    )
    .await;
    // 反例：只有释放没有同事务终止事件，占用不放。
    fails(
        pool,
        &format!(
            "UPDATE public.consignment_equipment_claims
             SET reservation_id = NULL, position_id = NULL, revision = revision + 1,
                 changed_at = '{ACCEPTED}', last_fact_id = '{HANDOFF_PROOF}'
             WHERE world_id = 'main' AND canonical_eid = '42';"
        ),
        "23514",
        Some("c4_claim_transition_fact_ck"),
    )
    .await;
    assert_eq!(
        states(pool).await,
        ("sale_pending".to_owned(), "committed".to_owned(), Some(POSITION.to_owned()))
    );

    // 正例：确领证明、position、事件、释放与成交关闭同一事务。
    succeeds(
        pool,
        &(handoff_command()
            + &handoff_proof()
            + &position_handed_over()
            + &handed_over(None, Some(HANDOFF_PROOF), HANDOFF_PROOF)
            + &listing_sold()),
    )
    .await;
    assert_eq!(states(pool).await, ("closed".to_owned(), "handed_over".to_owned(), None));
    // 同一 (合同, 义务) 只有一份确领证明。
    fails(
        pool,
        &handoff_proof().replace(HANDOFF_PROOF, "c6400000-0000-4000-8000-000000000002"),
        "23505",
        Some("c6_handoff_proof_obligation_uq"),
    )
    .await;
}

fn ledger_probe(actor: &str, status: u16, code: &str, disposition: &str, extra: &str) -> String {
    let command = "c6100000-0000-4000-8000-0000000000f1";
    format!(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_id, listing_id, result_status, result_code,
             result_body, created_at)
         VALUES ('{command}', 2, '{actor}', '{USER_2}', 'purchase', '{LISTING}',
                 decode(repeat('b4', 32), 'hex'), 'listing', '{LISTING}', '{LISTING}', {status},
                 '{code}', '{{\"schema_version\":2,\"operation\":\"purchase\",
                 \"disposition\":\"{disposition}\",\"command_id\":\"{command}\"{extra}}}',
                 '{PURCHASED_AT}');"
    )
}

#[tokio::test]
async fn postgres_consignment_ledger136_command_ledger_closes_purchase_codes_and_actors() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_main_custody(pool).await;

    // 反例：purchase 只属于买家（public_user）；worker 发出的 purchase 被 actor 闭集拒绝。
    check(
        pool,
        &ledger_probe("worker", 403, "self_purchase", "rejected", ",\"code\":\"self_purchase\""),
        "consignment_command_actor",
    )
    .await;
    // 反例：self_purchase 只能是 403；同码配 409 同时被回执与结果码闭集拒绝（按约束名先后，先报回执）。
    check(
        pool,
        &ledger_probe(
            "public_user",
            409,
            "self_purchase",
            "rejected",
            ",\"code\":\"self_purchase\"",
        ),
        "consignment_command_receipt",
    )
    .await;
    // 反例：成功回执的价格不是正整数。
    check(
        pool,
        &ledger_probe(
            "public_user",
            201,
            "ok",
            "purchased",
            &format!(
                ",\"listing_id\":\"{LISTING}\",\"listing_version\":6,\"state\":\"sale_pending\",
                 \"contract_id\":\"{CONTRACT}\",\"contract_version\":1,\"equipment_id\":\"{ASSET}\",
                 \"world\":\"main\",\"price\":0,\"order_state\":\"awaiting_payment\""
            ),
        ),
        "consignment_command_receipt",
    )
    .await;
    // 正例：买家的 403 self_purchase 确定拒绝留下安全回执。
    succeeds(
        pool,
        &ledger_probe(
            "public_user",
            403,
            "self_purchase",
            "rejected",
            ",\"code\":\"self_purchase\"",
        ),
    )
    .await;
}
