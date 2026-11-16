//! 报告判定与投影的纯逻辑测试；`pg_temp` 影子表上的忽略测试覆盖难以在真实约束下构造的异常态，
//! 真实 schema 与运行账号权限由 `tests/postgres/asset_reconciliation.rs` 覆盖。

use auction_application::reconciliation::{SolvencyInTransitReason, SolvencyWorldReport};

use super::solvency::{balance_evidence_is_current, solvency_verdict};
use super::*;

fn world(verdict: &str) -> SolvencyWorldReport {
    SolvencyWorldReport {
        world: "main".to_owned(),
        pending_seller_payout_count: 0,
        pending_seller_payout_credits: 0,
        pending_refund_count: 0,
        pending_refund_credits: 0,
        in_transit_seller_payout_count: 0,
        in_transit_seller_payout_credits: 0,
        in_transit_refund_count: 0,
        in_transit_refund_credits: 0,
        in_transit_review_count: 0,
        in_transit_review_credits: 0,
        in_transit_review_reasons: Vec::new(),
        pending_outbound_mail_count: 0,
        postage_estimate_credits: 0,
        required_credits: 0,
        balance_credits: None,
        balance_observed_at: None,
        balance_status: "unknown".to_owned(),
        evidence_status: "unknown".to_owned(),
        verdict: verdict.to_owned(),
        shortfall_credits: 0,
    }
}

fn report(complete: bool, verdict: &str) -> SolvencyReport {
    SolvencyReport {
        format: SOLVENCY_REPORT_FORMAT.to_owned(),
        report_id: Uuid::nil(),
        database_name: "auction".to_owned(),
        completed_at: Utc::now(),
        postage_rate_credits: POSTAGE_RATE_CREDITS,
        postage_rate_verified: POSTAGE_RATE_VERIFIED,
        complete,
        solvent: false,
        worlds: vec![world(verdict)],
        failure_code: None,
        asset_actions_authorized: false,
    }
}

#[test]
fn sufficient_requires_fresh_observed_balance() {
    assert_eq!(
        solvency_verdict(1_000, Some(1_000), "ok", "fresh", Some(0), 300),
        ("sufficient", 0)
    );
    assert_eq!(solvency_verdict(0, Some(0), "low", "fresh", Some(60), 300), ("sufficient", 0));
}

#[test]
fn shortfall_is_reported_only_with_fresh_balance() {
    assert_eq!(
        solvency_verdict(1_500, Some(1_000), "ok", "fresh", Some(0), 300),
        ("shortfall", 500)
    );
}

#[test]
fn missing_or_stale_balance_is_unknown_and_never_shortfall() {
    assert_eq!(
        solvency_verdict(1_500, None, "unavailable", "unavailable", None, 300),
        ("unknown", 0)
    );
    assert_eq!(
        solvency_verdict(1_500, Some(1_000), "stale", "stale", Some(0), 300),
        ("unknown", 0)
    );
    assert_eq!(solvency_verdict(1_500, Some(1_000), "busy", "fresh", Some(0), 300), ("unknown", 0));
    assert_eq!(solvency_verdict(1_500, Some(-1), "ok", "fresh", Some(0), 300), ("unknown", 0));
}

/// 持久化的 ok/fresh 标签不足以证明余额仍然可信：观察年龄超过**有效配置**即回到 `unknown`。
#[test]
fn expired_balance_evidence_is_unknown_even_when_labels_are_fresh() {
    assert_eq!(
        solvency_verdict(1_000, Some(1_000), "ok", "fresh", Some(300), 300),
        ("sufficient", 0)
    );
    assert_eq!(solvency_verdict(1_000, Some(1_000), "ok", "fresh", Some(301), 300), ("unknown", 0));
    // 60 秒配置下，120 秒旧余额即使持久化标签仍为 fresh 也不能判定充足。
    assert_eq!(solvency_verdict(1_000, Some(1_000), "ok", "fresh", Some(120), 60), ("unknown", 0));
    assert_eq!(
        solvency_verdict(1_000, Some(1_000), "ok", "fresh", Some(120), 300),
        ("sufficient", 0)
    );
    assert_eq!(solvency_verdict(1_000, Some(1_000), "ok", "fresh", None, 300), ("unknown", 0));
    assert_eq!(solvency_verdict(1_000, Some(1_000), "ok", "fresh", Some(-1), 300), ("unknown", 0));
    assert!(!balance_evidence_is_current(None, 300));
    assert!(balance_evidence_is_current(Some(0), 300));
    assert!(!balance_evidence_is_current(Some(301), 300));
    assert!(!balance_evidence_is_current(Some(120), 60));
}

#[test]
fn solvent_report_requires_complete_and_all_worlds_sufficient() {
    let mut complete = report(true, "sufficient");
    complete.solvent = complete.is_solvent();
    assert!(complete.solvent);

    let mut incomplete = report(false, "sufficient");
    incomplete.solvent = incomplete.is_solvent();
    assert!(!incomplete.solvent);

    let mut unknown = report(true, "unknown");
    unknown.solvent = unknown.is_solvent();
    assert!(!unknown.solvent);
}

#[test]
fn appended_report_facts_are_inert_and_serializable() {
    let custody = CustodyReconciliationReport {
        format: CUSTODY_REPORT_FORMAT.to_owned(),
        report_id: Uuid::nil(),
        database_name: "auction".to_owned(),
        completed_at: Utc::now(),
        sample_limit_per_world: SAMPLE_LIMIT,
        complete: true,
        consistent: false,
        worlds: vec![CustodyWorldReport {
            world: "main".to_owned(),
            observation: "observed".to_owned(),
            expected_count: 2,
            observed_count: 1,
            missing_count: 1,
            in_transit_count: 2,
            in_transit_reasons: vec![CustodyInTransitReason {
                reason: "obligation_unsold_return".to_owned(),
                count: 2,
            }],
            in_transit_review_count: 1,
            in_transit_review_reasons: vec![CustodyInTransitReason {
                reason: "obligation_unsold_return_manual_reconcile".to_owned(),
                count: 1,
            }],
            stock_count: 0,
            personal_count: 0,
            unexpected_count: 0,
            missing_samples: vec![CustodyMissingSample {
                external_equipment_id: "300753957".to_owned(),
                reason: "obligation_seller_payout".to_owned(),
            }],
            unexpected_samples: Vec::new(),
            samples_truncated: false,
            failure_code: None,
        }],
        failure_code: None,
        asset_actions_authorized: false,
    };
    let encoded = serde_json::to_value(&custody).expect("报告必须可序列化");
    assert_eq!(encoded["asset_actions_authorized"], false);
    assert_eq!(encoded["format"], CUSTODY_REPORT_FORMAT);
    let decoded: CustodyReconciliationReport =
        serde_json::from_value(encoded).expect("已追加报告必须可回读");
    assert_eq!(decoded, custody);

    let mut solvency = report(true, "sufficient");
    solvency.worlds[0].in_transit_review_count = 1;
    solvency.worlds[0].in_transit_review_credits = 900;
    solvency.worlds[0].in_transit_review_reasons = vec![SolvencyInTransitReason {
        reason: "obligation_seller_payout_manual_reconcile".to_owned(),
        count: 1,
        credits: 900,
    }];
    let encoded = serde_json::to_value(&solvency).expect("报告必须可序列化");
    assert_eq!(encoded["asset_actions_authorized"], false);
    let decoded: SolvencyReport = serde_json::from_value(encoded).expect("已追加报告必须可回读");
    assert_eq!(decoded, solvency);
}

/// v1 报告没有在途字段：读侧按 0/空列表补齐，不拒绝历史报告，也不重算它的缺失与缺口。
#[test]
fn v1_reports_decode_with_empty_in_transit_fields() {
    let custody: CustodyReconciliationReport = serde_json::from_value(serde_json::json!({
        "format": "auction-custody-reconciliation.v1",
        "report_id": Uuid::nil(),
        "database_name": "auction",
        "completed_at": "2026-11-16T05:00:00Z",
        "sample_limit_per_world": 100,
        "complete": true,
        "consistent": false,
        "worlds": [{
            "world": "main",
            "observation": "observed",
            "expected_count": 5,
            "observed_count": 0,
            "missing_count": 5,
            "stock_count": 0,
            "personal_count": 0,
            "unexpected_count": 0,
            "missing_samples": [],
            "unexpected_samples": [],
            "samples_truncated": true,
            "failure_code": null
        }],
        "failure_code": null,
        "asset_actions_authorized": false
    }))
    .expect("v1 装备报告必须可回读");
    assert_eq!(custody.worlds[0].missing_count, 5, "历史缺失原样保留");
    assert_eq!(custody.worlds[0].in_transit_count, 0);
    assert!(custody.worlds[0].in_transit_reasons.is_empty());
    assert_eq!(custody.worlds[0].in_transit_review_count, 0);
    assert!(custody.worlds[0].in_transit_review_reasons.is_empty());

    let mut solvency = serde_json::to_value(report(true, "shortfall")).expect("报告必须可序列化");
    solvency["format"] = serde_json::json!("auction-solvency-report.v1");
    let world = solvency["worlds"][0].as_object_mut().expect("世界报告必须是对象");
    for key in [
        "in_transit_seller_payout_count",
        "in_transit_seller_payout_credits",
        "in_transit_refund_count",
        "in_transit_refund_credits",
        "in_transit_review_count",
        "in_transit_review_credits",
        "in_transit_review_reasons",
    ] {
        world.remove(key);
    }
    let decoded: SolvencyReport = serde_json::from_value(solvency).expect("v1 偿付报告必须可回读");
    assert_eq!(decoded.worlds[0].in_transit_seller_payout_credits, 0);
    assert_eq!(decoded.worlds[0].in_transit_refund_count, 0);
    assert_eq!(decoded.worlds[0].in_transit_review_credits, 0);
    assert!(decoded.worlds[0].in_transit_review_reasons.is_empty());
    assert_eq!(decoded.worlds[0].verdict, "shortfall");
}

/// 偿付查询的金额聚合必须显式转回 BIGINT；`SUM(bigint)` 原生返回 NUMERIC。
/// 同时验证：现金负债按履约的 payment world 归属、未激活的 contingent 退款不计入、
/// 余额证据年龄由数据库时钟计算。
#[tokio::test]
#[ignore = "需要 AUCTION_TEST_DATABASE_URL 指向隔离的 PostgreSQL 18"]
async fn postgres_solvency_query_decodes_numeric_sum_as_i64()
-> Result<(), Box<dyn std::error::Error>> {
    use sqlx::{Connection, Executor, Row};
    let url = std::env::var("AUCTION_TEST_DATABASE_URL")?;
    let mut connection = sqlx::PgConnection::connect(&url).await?;
    let mut tx = connection.begin().await?;
    tx.execute(
        "CREATE TEMP TABLE settlement_obligations (
            id uuid, world_id text, obligation_kind text, business_state text, amount bigint
         );
         CREATE TEMP TABLE settlement_obligation_fulfillments (
            obligation_id uuid, execution_state text, dispatch_world text, delivery_id uuid,
            delivery_operation text
         );
         CREATE TEMP TABLE deliveries (
            id uuid, status text, payload_operation text, payload_world text,
            completed_at timestamptz
         );
         CREATE TEMP TABLE settlement_funding_allocations (
            delivery_id uuid, obligation_id uuid, state text
         );
         CREATE TEMP TABLE settlement_recovery_health (
            world_id text, balance_credits bigint, balance_observed_at timestamptz,
            balance_status text, evidence_status text
         );
         INSERT INTO pg_temp.settlement_obligations VALUES
            ('00000000-0000-0000-0000-000000000001','main','seller_payout','ready',100),
            ('00000000-0000-0000-0000-000000000002','isekai','seller_payout','ready',400),
            ('00000000-0000-0000-0000-000000000003','isekai','buyer_refund','contingent',250),
            ('00000000-0000-0000-0000-000000000004','isekai','buyer_refund','ready',300),
            ('00000000-0000-0000-0000-000000000005','isekai','buyer_refund','ready',700),
            ('00000000-0000-0000-0000-000000000006','main','seller_payout','ready',900),
            ('00000000-0000-0000-0000-000000000007','main','seller_payout','ready',800),
            ('00000000-0000-0000-0000-000000000008','main','seller_payout','ready',60),
            ('00000000-0000-0000-0000-000000000009','main','buyer_refund','ready',40);
         INSERT INTO pg_temp.settlement_obligation_fulfillments VALUES
            ('00000000-0000-0000-0000-000000000001','pending','main',NULL,'send'),
            ('00000000-0000-0000-0000-000000000002','pending','main',NULL,'send'),
            ('00000000-0000-0000-0000-000000000003','pending','main',NULL,'send'),
            ('00000000-0000-0000-0000-000000000004','pending','main',NULL,'send'),
            ('00000000-0000-0000-0000-000000000005','pending','isekai',NULL,'send'),
            ('00000000-0000-0000-0000-000000000006','satisfied','main',NULL,'send'),
            ('00000000-0000-0000-0000-000000000007','enqueued','main',
             '00000000-0000-0000-0000-000000000107','send'),
            ('00000000-0000-0000-0000-000000000008','enqueued','main',
             '00000000-0000-0000-0000-000000000108','send'),
            ('00000000-0000-0000-0000-000000000009','enqueued','main',
             '00000000-0000-0000-0000-000000000109','send');
         INSERT INTO pg_temp.deliveries VALUES
            ('00000000-0000-0000-0000-000000000107','succeeded','send','main',
             now() - interval '1 hour'),
            ('00000000-0000-0000-0000-000000000108','succeeded','send','main',
             now() - interval '1 hour'),
            ('00000000-0000-0000-0000-000000000109','succeeded','send','main',
             now() - interval '1 hour');
         -- 007 与 009 已寄出且已支出，是在途；008 已寄出但资金分配仍未知，继续按待付计。
         INSERT INTO pg_temp.settlement_funding_allocations VALUES
            ('00000000-0000-0000-0000-000000000107','00000000-0000-0000-0000-000000000007','spent'),
            ('00000000-0000-0000-0000-000000000108','00000000-0000-0000-0000-000000000008','unknown'),
            ('00000000-0000-0000-0000-000000000109','00000000-0000-0000-0000-000000000009','spent');
         INSERT INTO pg_temp.settlement_recovery_health VALUES
            ('main', 1000, now(), 'ok', 'fresh'),
            ('isekai', 900, now() - interval '600 seconds', 'ok', 'fresh');",
    )
    .await?;
    let main =
        sqlx::query(sqlx::AssertSqlSafe(queries::SOLVENCY_WORLD.replace("public.", "pg_temp.")))
            .bind("main")
            .fetch_one(&mut *tx)
            .await?;
    // main 卖方货款：本世界 100 + 异世界经 main 派发的 400 + 已寄出但资金未支出的 60；
    // 已 satisfied 的 900 不再计入，已寄出且已支出的 800 单列为在途。
    assert_eq!(main.try_get::<i64, _>("pending_seller_payout_count")?, 3);
    assert_eq!(main.try_get::<i64, _>("pending_seller_payout_credits")?, 560);
    assert_eq!(main.try_get::<i64, _>("in_transit_seller_payout_count")?, 1);
    assert_eq!(main.try_get::<i64, _>("in_transit_seller_payout_credits")?, 800);
    // main 退款：contingent 的 250 未激活不计入，只计 recovery 激活后 ready 的 300；
    // 已寄出且已支出的 40 是在途退款。
    assert_eq!(main.try_get::<i64, _>("pending_refund_count")?, 1);
    assert_eq!(main.try_get::<i64, _>("pending_refund_credits")?, 300);
    assert_eq!(main.try_get::<i64, _>("in_transit_refund_count")?, 1);
    assert_eq!(main.try_get::<i64, _>("in_transit_refund_credits")?, 40);
    let main_age: Option<i64> = main.try_get("balance_age_seconds")?;
    assert!(main_age.is_some_and(|age| (0..=60).contains(&age)));

    // 异世界不再报告经 main 派发的现金负债，只保留自己的支付世界义务。
    let isekai =
        sqlx::query(sqlx::AssertSqlSafe(queries::SOLVENCY_WORLD.replace("public.", "pg_temp.")))
            .bind("isekai")
            .fetch_one(&mut *tx)
            .await?;
    assert_eq!(isekai.try_get::<i64, _>("pending_seller_payout_count")?, 0);
    assert_eq!(isekai.try_get::<i64, _>("pending_seller_payout_credits")?, 0);
    assert_eq!(isekai.try_get::<i64, _>("pending_refund_count")?, 1);
    assert_eq!(isekai.try_get::<i64, _>("pending_refund_credits")?, 700);
    assert_eq!(isekai.try_get::<i64, _>("in_transit_seller_payout_count")?, 0);
    assert_eq!(isekai.try_get::<i64, _>("in_transit_refund_credits")?, 0);
    assert!(isekai.try_get::<Option<i64>, _>("balance_age_seconds")?.is_some_and(|age| age >= 600));
    tx.rollback().await?;
    Ok(())
}

/// 现金在途只认持久事实，并与余额观察时点对齐：寄出（投递完成时刻）不晚于余额观察且资金
/// 分配与同一投递、义务绑定并已 `spent` 才算在途；寄出晚于观察、时刻缺失、分配缺失或绑定
/// 错误、操作不是 `send`、投递未成功的一律仍是应付。寄出后转人工核对或失败的按寄出事实计
/// 在途待核对，按原因给出笔数与金额，不重新计入应付。没有余额观察的世界全部按应付计。
#[tokio::test]
#[ignore = "需要 AUCTION_TEST_DATABASE_URL 指向隔离的 PostgreSQL 18"]
async fn postgres_solvency_in_transit_aligns_with_balance_observation()
-> Result<(), Box<dyn std::error::Error>> {
    use sqlx::{Connection, Executor, Row};
    let url = std::env::var("AUCTION_TEST_DATABASE_URL")?;
    let mut connection = sqlx::PgConnection::connect(&url).await?;
    let mut tx = connection.begin().await?;
    // 同一事务内 now() 不变：余额观察时刻固定为 now() - 1 小时，103 与之恰好相等。
    tx.execute(
        "CREATE TEMP TABLE settlement_obligations (
            id uuid, world_id text, obligation_kind text, business_state text, amount bigint
         );
         CREATE TEMP TABLE settlement_obligation_fulfillments (
            obligation_id uuid, execution_state text, dispatch_world text, delivery_id uuid,
            delivery_operation text
         );
         CREATE TEMP TABLE deliveries (
            id uuid, status text, payload_operation text, payload_world text,
            completed_at timestamptz
         );
         CREATE TEMP TABLE settlement_funding_allocations (
            delivery_id uuid, obligation_id uuid, state text
         );
         CREATE TEMP TABLE settlement_recovery_health (
            world_id text, balance_credits bigint, balance_observed_at timestamptz,
            balance_status text, evidence_status text
         );
         INSERT INTO pg_temp.settlement_obligations VALUES
            ('00000000-0000-0000-0000-000000000101','main','seller_payout','ready',1000),
            ('00000000-0000-0000-0000-000000000102','main','seller_payout','ready',2000),
            ('00000000-0000-0000-0000-000000000103','main','seller_payout','ready',3000),
            ('00000000-0000-0000-0000-000000000104','main','seller_payout','ready',4000),
            ('00000000-0000-0000-0000-000000000105','main','buyer_refund','ready',500),
            ('00000000-0000-0000-0000-000000000106','main','seller_payout','ready',600),
            ('00000000-0000-0000-0000-000000000107','main','seller_payout','ready',700),
            ('00000000-0000-0000-0000-000000000108','main','seller_payout','ready',800),
            ('00000000-0000-0000-0000-000000000109','main','seller_payout','ready',900),
            ('00000000-0000-0000-0000-000000000110','main','seller_payout','ready',1100),
            ('00000000-0000-0000-0000-000000000111','main','seller_payout','ready',1200),
            ('00000000-0000-0000-0000-000000000121','isekai','seller_payout','ready',50);
         INSERT INTO pg_temp.settlement_obligation_fulfillments VALUES
            ('00000000-0000-0000-0000-000000000101','enqueued','main',
             '00000000-0000-0000-0000-000000000201','send'),
            ('00000000-0000-0000-0000-000000000102','enqueued','main',
             '00000000-0000-0000-0000-000000000202','send'),
            ('00000000-0000-0000-0000-000000000103','enqueued','main',
             '00000000-0000-0000-0000-000000000203','send'),
            ('00000000-0000-0000-0000-000000000104','manual_reconcile','main',
             '00000000-0000-0000-0000-000000000204','send'),
            ('00000000-0000-0000-0000-000000000105','failed','main',
             '00000000-0000-0000-0000-000000000205','send'),
            ('00000000-0000-0000-0000-000000000106','enqueued','main',
             '00000000-0000-0000-0000-000000000206','send'),
            ('00000000-0000-0000-0000-000000000107','enqueued','main',
             '00000000-0000-0000-0000-000000000207','send'),
            ('00000000-0000-0000-0000-000000000108','enqueued','main',
             '00000000-0000-0000-0000-000000000208','send'),
            ('00000000-0000-0000-0000-000000000109','enqueued','main',
             '00000000-0000-0000-0000-000000000209','return'),
            ('00000000-0000-0000-0000-000000000110','manual_reconcile','main',
             '00000000-0000-0000-0000-000000000210','send'),
            ('00000000-0000-0000-0000-000000000111','manual_reconcile','main',
             '00000000-0000-0000-0000-000000000211','send'),
            ('00000000-0000-0000-0000-000000000121','enqueued','isekai',
             '00000000-0000-0000-0000-000000000221','send');
         INSERT INTO pg_temp.deliveries VALUES
            ('00000000-0000-0000-0000-000000000201','succeeded','send','main',
             now() - interval '70 minutes'),
            ('00000000-0000-0000-0000-000000000202','succeeded','send','main',
             now() - interval '50 minutes'),
            ('00000000-0000-0000-0000-000000000203','succeeded','send','main',
             now() - interval '1 hour'),
            ('00000000-0000-0000-0000-000000000204','succeeded','send','main',
             now() - interval '2 hours'),
            ('00000000-0000-0000-0000-000000000205','succeeded','send','main',
             now() - interval '2 hours'),
            ('00000000-0000-0000-0000-000000000206','succeeded','send','main',
             now() - interval '2 hours'),
            ('00000000-0000-0000-0000-000000000207','succeeded','send','main',
             now() - interval '2 hours'),
            ('00000000-0000-0000-0000-000000000208','succeeded','send','main',
             now() - interval '2 hours'),
            ('00000000-0000-0000-0000-000000000209','succeeded','send','main',
             now() - interval '2 hours'),
            ('00000000-0000-0000-0000-000000000210','succeeded','send','main', NULL),
            ('00000000-0000-0000-0000-000000000211','manual_reconcile','send','main',
             now() - interval '2 hours'),
            ('00000000-0000-0000-0000-000000000221','succeeded','send','isekai',
             now() - interval '2 hours');
         -- 106 没有资金分配；107 的分配绑定在另一投递，108 的绑定在另一义务。
         INSERT INTO pg_temp.settlement_funding_allocations VALUES
            ('00000000-0000-0000-0000-000000000201','00000000-0000-0000-0000-000000000101','spent'),
            ('00000000-0000-0000-0000-000000000202','00000000-0000-0000-0000-000000000102','spent'),
            ('00000000-0000-0000-0000-000000000203','00000000-0000-0000-0000-000000000103','spent'),
            ('00000000-0000-0000-0000-000000000204','00000000-0000-0000-0000-000000000104','spent'),
            ('00000000-0000-0000-0000-000000000205','00000000-0000-0000-0000-000000000105','spent'),
            ('00000000-0000-0000-0000-000000000299','00000000-0000-0000-0000-000000000107','spent'),
            ('00000000-0000-0000-0000-000000000208','00000000-0000-0000-0000-000000000999','spent'),
            ('00000000-0000-0000-0000-000000000209','00000000-0000-0000-0000-000000000109','spent'),
            ('00000000-0000-0000-0000-000000000210','00000000-0000-0000-0000-000000000110','spent'),
            ('00000000-0000-0000-0000-000000000211','00000000-0000-0000-0000-000000000111','spent'),
            ('00000000-0000-0000-0000-000000000221','00000000-0000-0000-0000-000000000121','spent');
         INSERT INTO pg_temp.settlement_recovery_health VALUES
            ('main', 10000, now() - interval '1 hour', 'ok', 'fresh');",
    )
    .await?;
    let query = |sql: &str| sqlx::AssertSqlSafe(sql.replace("public.", "pg_temp."));
    let main = sqlx::query(query(queries::SOLVENCY_WORLD)).bind("main").fetch_one(&mut *tx).await?;
    // 在途：寄出早于观察的 101 与恰好等于观察时刻的 103。
    assert_eq!(main.try_get::<i64, _>("in_transit_seller_payout_count")?, 2);
    assert_eq!(main.try_get::<i64, _>("in_transit_seller_payout_credits")?, 4_000);
    assert_eq!(main.try_get::<i64, _>("in_transit_refund_count")?, 0);
    // 应付：寄出晚于观察的 102、无分配 106、绑定错误 107/108、操作为 return 的 109、
    // 寄出时刻缺失的 110 与投递结果未知的 111。
    assert_eq!(main.try_get::<i64, _>("pending_seller_payout_count")?, 7);
    assert_eq!(main.try_get::<i64, _>("pending_seller_payout_credits")?, 7_300);
    assert_eq!(main.try_get::<i64, _>("pending_refund_count")?, 0);
    assert_eq!(main.try_get::<i64, _>("pending_refund_credits")?, 0);

    let review = sqlx::query(query(queries::SOLVENCY_IN_TRANSIT_REVIEW))
        .bind("main")
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .map(|row| {
            Ok((
                row.try_get::<String, _>("reason")?,
                row.try_get::<i64, _>("item_count")?,
                row.try_get::<i64, _>("credits")?,
            ))
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    assert_eq!(
        review,
        [
            ("obligation_buyer_refund_failed".to_owned(), 1, 500),
            ("obligation_seller_payout_manual_reconcile".to_owned(), 1, 4_000),
        ]
    );

    // 异世界从未观察到余额：已寄出且已支出的 121 也只能按应付计，不能凭空移出。
    let isekai =
        sqlx::query(query(queries::SOLVENCY_WORLD)).bind("isekai").fetch_one(&mut *tx).await?;
    assert_eq!(isekai.try_get::<i64, _>("pending_seller_payout_credits")?, 50);
    assert_eq!(isekai.try_get::<i64, _>("in_transit_seller_payout_count")?, 0);
    assert!(isekai.try_get::<Option<i64>, _>("balance_credits")?.is_none());
    let isekai_review = sqlx::query(query(queries::SOLVENCY_IN_TRANSIT_REVIEW))
        .bind("isekai")
        .fetch_all(&mut *tx)
        .await?;
    assert!(isekai_review.is_empty());
    tx.rollback().await?;
    Ok(())
}

/// 应持集合只包含拍卖相关装备；数据库已知普通库存计入 stock，未知才是 unexpected。
#[tokio::test]
#[ignore = "需要 AUCTION_TEST_DATABASE_URL 指向隔离的 PostgreSQL 18"]
async fn postgres_custody_counts_separate_expected_stock_and_unknown()
-> Result<(), Box<dyn std::error::Error>> {
    use sqlx::{Connection, Executor, Row};
    let url = std::env::var("AUCTION_TEST_DATABASE_URL")?;
    let mut connection = sqlx::PgConnection::connect(&url).await?;
    let mut tx = connection.begin().await?;
    tx.execute(
        "CREATE TEMP TABLE equipment (id uuid, world_id text, external_equipment_id text);
         CREATE TEMP TABLE settlement_obligations (
            id uuid, equipment_id uuid, obligation_kind text, submission_id uuid
         );
         CREATE TEMP TABLE settlement_obligation_fulfillments (
            obligation_id uuid, execution_state text, delivery_id uuid, delivery_operation text
         );
         CREATE TEMP TABLE auctions (id uuid, status text, world_id text);
         CREATE TEMP TABLE auction_preview_items (auction_id uuid, equipment_id uuid);
         CREATE TEMP TABLE auction_submissions (id uuid, auction_id uuid, source_reference text);
         CREATE TEMP TABLE deliveries (id uuid, status text, payload jsonb);
         INSERT INTO pg_temp.equipment VALUES
            ('00000000-0000-0000-0000-000000000001','main','100'),
            ('00000000-0000-0000-0000-000000000002','main','200');
         INSERT INTO pg_temp.settlement_obligations VALUES
            ('00000000-0000-0000-0000-000000000010','00000000-0000-0000-0000-000000000001','unsold_return',NULL);
         INSERT INTO pg_temp.settlement_obligation_fulfillments (obligation_id, execution_state) VALUES
            ('00000000-0000-0000-0000-000000000010','pending');",
    ).await?;
    let row =
        sqlx::query(sqlx::AssertSqlSafe(queries::CUSTODY_COUNTS.replace("public.", "pg_temp.")))
            .bind("main")
            .bind(&["100".to_owned(), "200".to_owned(), "999".to_owned()][..])
            .bind(100_i64)
            .fetch_one(&mut *tx)
            .await?;
    assert_eq!(row.try_get::<i64, _>("expected_count")?, 1);
    assert_eq!(row.try_get::<i64, _>("observed_count")?, 3);
    assert_eq!(row.try_get::<i64, _>("missing_count")?, 0);
    assert_eq!(row.try_get::<i64, _>("stock_count")?, 1);
    assert_eq!(row.try_get::<i64, _>("personal_count")?, 1);
    assert_eq!(row.try_get::<i64, _>("unexpected_count")?, 0);
    tx.rollback().await?;
    Ok(())
}

/// 预览与投稿只属于各自世界；差异样本必须包含与计数相同的应持来源。
#[tokio::test]
#[ignore = "需要 AUCTION_TEST_DATABASE_URL 指向隔离的 PostgreSQL 18"]
async fn postgres_custody_scopes_preview_and_submission_samples_by_world()
-> Result<(), Box<dyn std::error::Error>> {
    use sqlx::{Connection, Executor, Row};
    let url = std::env::var("AUCTION_TEST_DATABASE_URL")?;
    let mut connection = sqlx::PgConnection::connect(&url).await?;
    let mut tx = connection.begin().await?;
    tx.execute(
        "CREATE TEMP TABLE equipment (id uuid, world_id text, external_equipment_id text);
         CREATE TEMP TABLE settlement_obligations (
            id uuid, equipment_id uuid, obligation_kind text, submission_id uuid
         );
         CREATE TEMP TABLE settlement_obligation_fulfillments (
            obligation_id uuid, execution_state text, delivery_id uuid, delivery_operation text
         );
         CREATE TEMP TABLE auctions (id uuid, status text, world_id text);
         CREATE TEMP TABLE auction_preview_items (auction_id uuid, equipment_id uuid);
         CREATE TEMP TABLE auction_submissions (id uuid, auction_id uuid, source_reference text);
         CREATE TEMP TABLE deliveries (id uuid, status text, payload jsonb);
         INSERT INTO pg_temp.equipment VALUES
            ('00000000-0000-0000-0000-000000000001','main','100'),
            ('00000000-0000-0000-0000-000000000002','main','200'),
            ('00000000-0000-0000-0000-000000000003','isekai','300');
         INSERT INTO pg_temp.settlement_obligations VALUES
            ('00000000-0000-0000-0000-000000000010','00000000-0000-0000-0000-000000000001','unsold_return',NULL);
         INSERT INTO pg_temp.settlement_obligation_fulfillments (obligation_id, execution_state) VALUES
            ('00000000-0000-0000-0000-000000000010','pending');
         INSERT INTO pg_temp.auctions VALUES
            ('00000000-0000-0000-0000-000000000020','preview','main'),
            ('00000000-0000-0000-0000-000000000030','preview','isekai');
         INSERT INTO pg_temp.auction_preview_items VALUES
            ('00000000-0000-0000-0000-000000000020','00000000-0000-0000-0000-000000000002'),
            ('00000000-0000-0000-0000-000000000030','00000000-0000-0000-0000-000000000003');
         INSERT INTO pg_temp.auction_submissions VALUES
            ('00000000-0000-0000-0000-000000000041','00000000-0000-0000-0000-000000000020','hentaiverse-equipment-v1:400'),
            ('00000000-0000-0000-0000-000000000042','00000000-0000-0000-0000-000000000030','hentaiverse-equipment-v1:500');",
    )
    .await?;
    let row =
        sqlx::query(sqlx::AssertSqlSafe(queries::CUSTODY_COUNTS.replace("public.", "pg_temp.")))
            .bind("main")
            .bind(&["100".to_owned(), "200".to_owned(), "400".to_owned()][..])
            .bind(100_i64)
            .fetch_one(&mut *tx)
            .await?;
    assert_eq!(row.try_get::<i64, _>("expected_count")?, 3);
    assert_eq!(row.try_get::<i64, _>("missing_count")?, 0);

    let rows =
        sqlx::query(sqlx::AssertSqlSafe(queries::CUSTODY_SAMPLES.replace("public.", "pg_temp.")))
            .bind("main")
            .bind(&["100".to_owned(), "500".to_owned()][..])
            .bind(100_i64)
            .fetch_all(&mut *tx)
            .await?;
    let samples = rows
        .into_iter()
        .map(|row| {
            Ok((
                row.try_get::<String, _>("external_equipment_id")?,
                row.try_get::<String, _>("reason")?,
            ))
        })
        .collect::<Result<std::collections::BTreeMap<_, _>, sqlx::Error>>()?;
    assert_eq!(samples.get("200").map(String::as_str), Some("preview_preview"));
    assert_eq!(samples.get("400").map(String::as_str), Some("submission_preview"));
    assert!(!samples.contains_key("300"));
    assert!(!samples.contains_key("500"));
    tx.rollback().await?;
    Ok(())
}

/// 已满足的 withdrawn_return 让投稿装备离开应持集合；拍卖重新进入 preview 时，
/// 留存下来的投稿行与来源引用不能再把已退回的装备算作应持（计数与样本同规则）。
#[tokio::test]
#[ignore = "需要 AUCTION_TEST_DATABASE_URL 指向隔离的 PostgreSQL 18"]
async fn postgres_custody_excludes_withdrawn_submissions_after_return()
-> Result<(), Box<dyn std::error::Error>> {
    use sqlx::{Connection, Executor, Row};
    let url = std::env::var("AUCTION_TEST_DATABASE_URL")?;
    let mut connection = sqlx::PgConnection::connect(&url).await?;
    let mut tx = connection.begin().await?;
    tx.execute(
        "CREATE TEMP TABLE equipment (id uuid, world_id text, external_equipment_id text);
         CREATE TEMP TABLE settlement_obligations (
            id uuid, equipment_id uuid, obligation_kind text, submission_id uuid
         );
         CREATE TEMP TABLE settlement_obligation_fulfillments (
            obligation_id uuid, execution_state text, delivery_id uuid, delivery_operation text
         );
         CREATE TEMP TABLE auctions (id uuid, status text, world_id text);
         CREATE TEMP TABLE auction_preview_items (auction_id uuid, equipment_id uuid);
         CREATE TEMP TABLE auction_submissions (id uuid, auction_id uuid, source_reference text);
         CREATE TEMP TABLE deliveries (id uuid, status text, payload jsonb);
         INSERT INTO pg_temp.equipment VALUES
            ('00000000-0000-0000-0000-000000000001','main','900'),
            ('00000000-0000-0000-0000-000000000002','main','901');
         INSERT INTO pg_temp.auctions VALUES
            ('00000000-0000-0000-0000-000000000020','preview','main');
         INSERT INTO pg_temp.auction_submissions VALUES
            ('00000000-0000-0000-0000-000000000041','00000000-0000-0000-0000-000000000020','hentaiverse-equipment-v1:900'),
            ('00000000-0000-0000-0000-000000000042','00000000-0000-0000-0000-000000000020','hentaiverse-equipment-v1:901');
         INSERT INTO pg_temp.settlement_obligations VALUES
            ('00000000-0000-0000-0000-000000000010','00000000-0000-0000-0000-000000000001','withdrawn_return','00000000-0000-0000-0000-000000000041'),
            ('00000000-0000-0000-0000-000000000011','00000000-0000-0000-0000-000000000002','withdrawn_return','00000000-0000-0000-0000-000000000042');
         INSERT INTO pg_temp.settlement_obligation_fulfillments (obligation_id, execution_state) VALUES
            ('00000000-0000-0000-0000-000000000010','satisfied'),
            ('00000000-0000-0000-0000-000000000011','pending');",
    )
    .await?;
    let row =
        sqlx::query(sqlx::AssertSqlSafe(queries::CUSTODY_COUNTS.replace("public.", "pg_temp.")))
            .bind("main")
            .bind(&[] as &[String])
            .bind(100_i64)
            .fetch_one(&mut *tx)
            .await?;
    assert_eq!(row.try_get::<i64, _>("expected_count")?, 1);
    assert_eq!(row.try_get::<i64, _>("missing_count")?, 1);

    let rows =
        sqlx::query(sqlx::AssertSqlSafe(queries::CUSTODY_SAMPLES.replace("public.", "pg_temp.")))
            .bind("main")
            .bind(&[] as &[String])
            .bind(100_i64)
            .fetch_all(&mut *tx)
            .await?;
    let samples = rows
        .into_iter()
        .map(|row| {
            Ok((
                row.try_get::<String, _>("external_equipment_id")?,
                row.try_get::<String, _>("reason")?,
            ))
        })
        .collect::<Result<std::collections::BTreeMap<_, _>, sqlx::Error>>()?;
    assert_eq!(samples.get("901").map(String::as_str), Some("obligation_withdrawn_return"));
    assert!(!samples.contains_key("900"));
    tx.rollback().await?;
    Ok(())
}

/// 投递分支按结算生成方的 canonical 载荷识别装备附件：kind 为 `auction_equipment_key`；
/// 早期载荷的 `equipment` 与缺省 kind 兼容识别，其他 kind 不是装备附件，已结束的投递不再
/// 应持。这里只有投递、没有义务，应持只能来自投递分支。
#[tokio::test]
#[ignore = "需要 AUCTION_TEST_DATABASE_URL 指向隔离的 PostgreSQL 18"]
async fn postgres_custody_delivery_branch_reads_canonical_attachment_kind()
-> Result<(), Box<dyn std::error::Error>> {
    use sqlx::{Connection, Executor, Row};
    let url = std::env::var("AUCTION_TEST_DATABASE_URL")?;
    let mut connection = sqlx::PgConnection::connect(&url).await?;
    let mut tx = connection.begin().await?;
    tx.execute(
        r#"CREATE TEMP TABLE equipment (id uuid, world_id text, external_equipment_id text);
         CREATE TEMP TABLE settlement_obligations (
            id uuid, equipment_id uuid, obligation_kind text, submission_id uuid
         );
         CREATE TEMP TABLE settlement_obligation_fulfillments (
            obligation_id uuid, execution_state text, delivery_id uuid, delivery_operation text
         );
         CREATE TEMP TABLE auctions (id uuid, status text, world_id text);
         CREATE TEMP TABLE auction_preview_items (auction_id uuid, equipment_id uuid);
         CREATE TEMP TABLE auction_submissions (id uuid, auction_id uuid, source_reference text);
         CREATE TEMP TABLE deliveries (id uuid, status text, payload jsonb);
         INSERT INTO pg_temp.equipment VALUES
            ('00000000-0000-0000-0000-000000000001','main','100'),
            ('00000000-0000-0000-0000-000000000002','main','200'),
            ('00000000-0000-0000-0000-000000000003','main','300'),
            ('00000000-0000-0000-0000-000000000004','main','400'),
            ('00000000-0000-0000-0000-000000000005','main','500');
         -- 101 与结算生成方逐字段同形（异世界交付模板的 canonical 载荷）。
         INSERT INTO pg_temp.deliveries VALUES
            ('00000000-0000-0000-0000-000000000101','pending',
             '{"schema_version":1,"operation":"send","world":"main",
               "template":"settlement_equipment_handoff",
               "variables":{"item_name":"Fixture Sword",
                            "reference":"00000000-0000-0000-0000-000000000901"},
               "credits":0,"cod":0,
               "equipment_secret_refs":[{"kind":"auction_equipment_key",
                                         "id":"00000000-0000-0000-0000-000000000001"}]}'),
            ('00000000-0000-0000-0000-000000000102','claimed',
             '{"equipment_secret_refs":[{"kind":"equipment",
                                         "id":"00000000-0000-0000-0000-000000000002"}]}'),
            ('00000000-0000-0000-0000-000000000103','sending',
             '{"equipment_secret_refs":[{"id":"00000000-0000-0000-0000-000000000003"}]}'),
            ('00000000-0000-0000-0000-000000000104','pending',
             '{"equipment_secret_refs":[{"kind":"user_key",
                                         "id":"00000000-0000-0000-0000-000000000004"}]}'),
            ('00000000-0000-0000-0000-000000000105','succeeded',
             '{"equipment_secret_refs":[{"kind":"auction_equipment_key",
                                         "id":"00000000-0000-0000-0000-000000000005"}]}');"#,
    )
    .await?;
    let row =
        sqlx::query(sqlx::AssertSqlSafe(queries::CUSTODY_COUNTS.replace("public.", "pg_temp.")))
            .bind("main")
            .bind(&[] as &[String])
            .bind(100_i64)
            .fetch_one(&mut *tx)
            .await?;
    assert_eq!(row.try_get::<i64, _>("expected_count")?, 3);
    assert_eq!(row.try_get::<i64, _>("missing_count")?, 3);

    let rows =
        sqlx::query(sqlx::AssertSqlSafe(queries::CUSTODY_SAMPLES.replace("public.", "pg_temp.")))
            .bind("main")
            .bind(&[] as &[String])
            .bind(100_i64)
            .fetch_all(&mut *tx)
            .await?;
    let samples = rows
        .into_iter()
        .map(|row| {
            Ok((
                row.try_get::<String, _>("external_equipment_id")?,
                row.try_get::<String, _>("reason")?,
            ))
        })
        .collect::<Result<std::collections::BTreeMap<_, _>, sqlx::Error>>()?;
    assert_eq!(samples.get("100").map(String::as_str), Some("delivery_pending_equipment"));
    assert_eq!(samples.get("200").map(String::as_str), Some("delivery_claimed_equipment"));
    assert_eq!(samples.get("300").map(String::as_str), Some("delivery_sending_equipment"));
    assert!(!samples.contains_key("400"), "非装备 kind 不是装备附件");
    assert!(!samples.contains_key("500"), "已成功寄出的投递不再应持");
    tx.rollback().await?;
    Ok(())
}

/// 已成功寄出的装备离开应持集合并按原因单列：履约仍在等待领取的是在途，寄出后转人工核对或
/// 失败的是在途待核对（不再算缺失）；本轮已在 Armory 观察到的不算在途。尚未寄出、投递操作不是
/// `send` 的仍按应持核对；撤拍退回已寄出时，投稿分支同样不再把它算作应持。同一装备另有尚未
/// 寄出的义务时按应持核对、不算在途；两类在途义务并存时只按待核对计一次。
#[tokio::test]
#[ignore = "需要 AUCTION_TEST_DATABASE_URL 指向隔离的 PostgreSQL 18"]
async fn postgres_custody_moves_sent_equipment_to_in_transit()
-> Result<(), Box<dyn std::error::Error>> {
    use sqlx::{Connection, Executor, Row};
    let url = std::env::var("AUCTION_TEST_DATABASE_URL")?;
    let mut connection = sqlx::PgConnection::connect(&url).await?;
    let mut tx = connection.begin().await?;
    tx.execute(
        "CREATE TEMP TABLE equipment (id uuid, world_id text, external_equipment_id text);
         CREATE TEMP TABLE settlement_obligations (
            id uuid, equipment_id uuid, obligation_kind text, submission_id uuid
         );
         CREATE TEMP TABLE settlement_obligation_fulfillments (
            obligation_id uuid, execution_state text, delivery_id uuid, delivery_operation text
         );
         CREATE TEMP TABLE auctions (id uuid, status text, world_id text);
         CREATE TEMP TABLE auction_preview_items (auction_id uuid, equipment_id uuid);
         CREATE TEMP TABLE auction_submissions (id uuid, auction_id uuid, source_reference text);
         CREATE TEMP TABLE deliveries (id uuid, status text, payload jsonb);
         INSERT INTO pg_temp.equipment VALUES
            ('00000000-0000-0000-0000-000000000001','main','100'),
            ('00000000-0000-0000-0000-000000000002','main','200'),
            ('00000000-0000-0000-0000-000000000003','main','300'),
            ('00000000-0000-0000-0000-000000000004','main','400'),
            ('00000000-0000-0000-0000-000000000005','main','500'),
            ('00000000-0000-0000-0000-000000000006','main','600'),
            ('00000000-0000-0000-0000-000000000007','main','700'),
            ('00000000-0000-0000-0000-000000000008','main','800'),
            ('00000000-0000-0000-0000-000000000009','main','900');
         INSERT INTO pg_temp.auctions VALUES
            ('00000000-0000-0000-0000-000000000020','preview','main');
         INSERT INTO pg_temp.auction_submissions VALUES
            ('00000000-0000-0000-0000-000000000041','00000000-0000-0000-0000-000000000020','hentaiverse-equipment-v1:400');
         INSERT INTO pg_temp.settlement_obligations VALUES
            ('00000000-0000-0000-0000-000000000010','00000000-0000-0000-0000-000000000001','unsold_return',NULL),
            ('00000000-0000-0000-0000-000000000011','00000000-0000-0000-0000-000000000002','winner_equipment_cod',NULL),
            ('00000000-0000-0000-0000-000000000012','00000000-0000-0000-0000-000000000003','unsold_return',NULL),
            ('00000000-0000-0000-0000-000000000013','00000000-0000-0000-0000-000000000004','withdrawn_return','00000000-0000-0000-0000-000000000041'),
            ('00000000-0000-0000-0000-000000000014','00000000-0000-0000-0000-000000000005','winner_equipment_cod',NULL),
            ('00000000-0000-0000-0000-000000000015','00000000-0000-0000-0000-000000000006','winner_equipment_cod',NULL),
            ('00000000-0000-0000-0000-000000000016','00000000-0000-0000-0000-000000000006','unsold_return',NULL),
            ('00000000-0000-0000-0000-000000000017','00000000-0000-0000-0000-000000000007','unsold_return',NULL),
            ('00000000-0000-0000-0000-000000000018','00000000-0000-0000-0000-000000000008','winner_equipment_handoff',NULL),
            ('00000000-0000-0000-0000-000000000019','00000000-0000-0000-0000-000000000009','winner_equipment_cod',NULL),
            ('00000000-0000-0000-0000-000000000020','00000000-0000-0000-0000-000000000009','unsold_return',NULL);
         INSERT INTO pg_temp.settlement_obligation_fulfillments VALUES
            ('00000000-0000-0000-0000-000000000010','enqueued','00000000-0000-0000-0000-000000001001','send'),
            ('00000000-0000-0000-0000-000000000011','enqueued','00000000-0000-0000-0000-000000001002','send'),
            ('00000000-0000-0000-0000-000000000012','manual_reconcile','00000000-0000-0000-0000-000000001003','send'),
            ('00000000-0000-0000-0000-000000000013','enqueued','00000000-0000-0000-0000-000000001004','send'),
            ('00000000-0000-0000-0000-000000000014','enqueued','00000000-0000-0000-0000-000000001005','send'),
            ('00000000-0000-0000-0000-000000000015','manual_reconcile','00000000-0000-0000-0000-000000001006','send'),
            ('00000000-0000-0000-0000-000000000016','pending',NULL,'send'),
            ('00000000-0000-0000-0000-000000000017','enqueued','00000000-0000-0000-0000-000000001007','return'),
            ('00000000-0000-0000-0000-000000000018','failed','00000000-0000-0000-0000-000000001008','send'),
            ('00000000-0000-0000-0000-000000000019','manual_reconcile','00000000-0000-0000-0000-000000001009','send'),
            ('00000000-0000-0000-0000-000000000020','enqueued','00000000-0000-0000-0000-000000001010','send');
         INSERT INTO pg_temp.deliveries VALUES
            ('00000000-0000-0000-0000-000000001001','succeeded','{}'),
            ('00000000-0000-0000-0000-000000001002','succeeded','{}'),
            ('00000000-0000-0000-0000-000000001003','succeeded','{}'),
            ('00000000-0000-0000-0000-000000001004','succeeded','{}'),
            ('00000000-0000-0000-0000-000000001005','sending','{}'),
            ('00000000-0000-0000-0000-000000001006','succeeded','{}'),
            ('00000000-0000-0000-0000-000000001007','succeeded','{}'),
            ('00000000-0000-0000-0000-000000001008','succeeded','{}'),
            ('00000000-0000-0000-0000-000000001009','succeeded','{}'),
            ('00000000-0000-0000-0000-000000001010','succeeded','{}');",
    )
    .await?;
    let observed = ["200".to_owned()];
    let row =
        sqlx::query(sqlx::AssertSqlSafe(queries::CUSTODY_COUNTS.replace("public.", "pg_temp.")))
            .bind("main")
            .bind(&observed[..])
            .bind(100_i64)
            .fetch_one(&mut *tx)
            .await?;
    // 只有尚未寄出的 500、另有待寄义务的 600 与操作为 return 的 700 仍应持；已在 Armory 的
    // 在途 200 计为普通库存。
    assert_eq!(row.try_get::<i64, _>("expected_count")?, 3);
    assert_eq!(row.try_get::<i64, _>("missing_count")?, 3);
    assert_eq!(row.try_get::<i64, _>("stock_count")?, 1);
    assert_eq!(row.try_get::<i64, _>("personal_count")?, 0);

    let rows = sqlx::query(sqlx::AssertSqlSafe(
        queries::CUSTODY_IN_TRANSIT.replace("public.", "pg_temp."),
    ))
    .bind("main")
    .bind(&observed[..])
    .fetch_all(&mut *tx)
    .await?;
    let in_transit = rows
        .into_iter()
        .map(|row| {
            Ok((
                row.try_get::<bool, _>("review")?,
                row.try_get::<String, _>("reason")?,
                row.try_get::<i64, _>("item_count")?,
            ))
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    assert_eq!(
        in_transit,
        [
            (false, "obligation_unsold_return".to_owned(), 1),
            (false, "obligation_withdrawn_return".to_owned(), 1),
            (true, "obligation_unsold_return_manual_reconcile".to_owned(), 1),
            (true, "obligation_winner_equipment_cod_manual_reconcile".to_owned(), 1),
            (true, "obligation_winner_equipment_handoff_failed".to_owned(), 1),
        ]
    );

    let rows =
        sqlx::query(sqlx::AssertSqlSafe(queries::CUSTODY_SAMPLES.replace("public.", "pg_temp.")))
            .bind("main")
            .bind(&observed[..])
            .bind(100_i64)
            .fetch_all(&mut *tx)
            .await?;
    let samples = rows
        .into_iter()
        .map(|row| {
            Ok((
                row.try_get::<String, _>("external_equipment_id")?,
                row.try_get::<String, _>("reason")?,
            ))
        })
        .collect::<Result<std::collections::BTreeMap<_, _>, sqlx::Error>>()?;
    assert_eq!(samples.get("500").map(String::as_str), Some("obligation_winner_equipment_cod"));
    assert_eq!(samples.get("600").map(String::as_str), Some("obligation_unsold_return"));
    assert_eq!(samples.get("700").map(String::as_str), Some("obligation_unsold_return"));
    assert_eq!(samples.len(), 3, "在途与在途待核对装备不得出现在缺失样本中");
    tx.rollback().await?;
    Ok(())
}
