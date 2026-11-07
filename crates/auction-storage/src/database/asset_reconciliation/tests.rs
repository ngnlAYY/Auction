//! 报告判定与投影的纯逻辑测试；数据库查询由隔离 PG18 的忽略测试覆盖。

use self::parent::*;
use crate::database::asset_reconciliation as parent;

fn world(verdict: &str) -> SolvencyWorldReport {
    SolvencyWorldReport {
        world: "main".to_owned(),
        pending_seller_payout_count: 0,
        pending_seller_payout_credits: 0,
        pending_refund_count: 0,
        pending_refund_credits: 0,
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
    assert_eq!(solvency_verdict(1_000, Some(1_000), "ok", "fresh"), ("sufficient", 0));
    assert_eq!(solvency_verdict(0, Some(0), "low", "fresh"), ("sufficient", 0));
}

#[test]
fn shortfall_is_reported_only_with_fresh_balance() {
    assert_eq!(solvency_verdict(1_500, Some(1_000), "ok", "fresh"), ("shortfall", 500));
}

#[test]
fn missing_or_stale_balance_is_unknown_and_never_shortfall() {
    assert_eq!(solvency_verdict(1_500, None, "unavailable", "unavailable"), ("unknown", 0));
    assert_eq!(solvency_verdict(1_500, Some(1_000), "stale", "stale"), ("unknown", 0));
    assert_eq!(solvency_verdict(1_500, Some(1_000), "busy", "fresh"), ("unknown", 0));
    assert_eq!(solvency_verdict(1_500, Some(-1), "ok", "fresh"), ("unknown", 0));
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
}

/// 偿付查询的金额聚合必须显式转回 BIGINT；`SUM(bigint)` 原生返回 NUMERIC。
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
            id uuid, world_id text, obligation_kind text, amount bigint
         );
         CREATE TEMP TABLE settlement_obligation_fulfillments (
            obligation_id uuid, execution_state text
         );
         CREATE TEMP TABLE deliveries (
            status text, payload_operation text, payload_world text
         );
         CREATE TEMP TABLE settlement_recovery_health (
            world_id text, balance_credits bigint, balance_observed_at timestamptz,
            balance_status text, evidence_status text
         );
         INSERT INTO pg_temp.settlement_obligations VALUES
            ('00000000-0000-0000-0000-000000000001','main','seller_payout',100),
            ('00000000-0000-0000-0000-000000000002','main','buyer_refund',250);
         INSERT INTO pg_temp.settlement_obligation_fulfillments VALUES
            ('00000000-0000-0000-0000-000000000001','pending'),
            ('00000000-0000-0000-0000-000000000002','pending');
         INSERT INTO pg_temp.settlement_recovery_health VALUES
            ('main', 1000, now(), 'ok', 'fresh');",
    )
    .await?;
    let row =
        sqlx::query(sqlx::AssertSqlSafe(queries::SOLVENCY_WORLD.replace("public.", "pg_temp.")))
            .bind("main")
            .fetch_one(&mut *tx)
            .await?;
    assert_eq!(row.try_get::<i64, _>("pending_seller_payout_credits")?, 100);
    assert_eq!(row.try_get::<i64, _>("pending_refund_credits")?, 250);
    tx.rollback().await?;
    Ok(())
}

/// W11.1 应持集合只包含拍卖相关装备；数据库已知普通库存计入 stock，未知才是 unexpected。
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
            id uuid, equipment_id uuid, obligation_kind text
         );
         CREATE TEMP TABLE settlement_obligation_fulfillments (
            obligation_id uuid, execution_state text
         );
         CREATE TEMP TABLE auctions (id uuid, status text, world_id text);
         CREATE TEMP TABLE auction_preview_items (auction_id uuid, equipment_id uuid);
         CREATE TEMP TABLE auction_submissions (auction_id uuid, source_reference text);
         CREATE TEMP TABLE deliveries (status text, payload jsonb);
         INSERT INTO pg_temp.equipment VALUES
            ('00000000-0000-0000-0000-000000000001','main','100'),
            ('00000000-0000-0000-0000-000000000002','main','200');
         INSERT INTO pg_temp.settlement_obligations VALUES
            ('00000000-0000-0000-0000-000000000010','00000000-0000-0000-0000-000000000001','unsold_return');
         INSERT INTO pg_temp.settlement_obligation_fulfillments VALUES
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
            id uuid, equipment_id uuid, obligation_kind text
         );
         CREATE TEMP TABLE settlement_obligation_fulfillments (
            obligation_id uuid, execution_state text
         );
         CREATE TEMP TABLE auctions (id uuid, status text, world_id text);
         CREATE TEMP TABLE auction_preview_items (auction_id uuid, equipment_id uuid);
         CREATE TEMP TABLE auction_submissions (auction_id uuid, source_reference text);
         CREATE TEMP TABLE deliveries (status text, payload jsonb);
         INSERT INTO pg_temp.equipment VALUES
            ('00000000-0000-0000-0000-000000000001','main','100'),
            ('00000000-0000-0000-0000-000000000002','main','200'),
            ('00000000-0000-0000-0000-000000000003','isekai','300');
         INSERT INTO pg_temp.settlement_obligations VALUES
            ('00000000-0000-0000-0000-000000000010','00000000-0000-0000-0000-000000000001','unsold_return');
         INSERT INTO pg_temp.settlement_obligation_fulfillments VALUES
            ('00000000-0000-0000-0000-000000000010','pending');
         INSERT INTO pg_temp.auctions VALUES
            ('00000000-0000-0000-0000-000000000020','preview','main'),
            ('00000000-0000-0000-0000-000000000030','preview','isekai');
         INSERT INTO pg_temp.auction_preview_items VALUES
            ('00000000-0000-0000-0000-000000000020','00000000-0000-0000-0000-000000000002'),
            ('00000000-0000-0000-0000-000000000030','00000000-0000-0000-0000-000000000003');
         INSERT INTO pg_temp.auction_submissions VALUES
            ('00000000-0000-0000-0000-000000000020','hentaiverse-equipment-v1:400'),
            ('00000000-0000-0000-0000-000000000030','hentaiverse-equipment-v1:500');",
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
