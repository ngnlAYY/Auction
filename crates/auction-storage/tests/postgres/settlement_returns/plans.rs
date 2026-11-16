//! 结算退件新增读路径的执行计划基准（`--ignored`，运行方式同 `crate::perf`）：偿付分类查询与 Admin
//! 退件列表在合成历史上的 `EXPLAIN (ANALYZE, BUFFERS)`。SQL 经
//! `auction_storage::solvency_benchmark_sql` 与 `auction_storage::settlement_return_benchmark_sql`
//! 读取生产原文，测量与运行不产生副本。
//!
//! 合成历史在 `crate::perf::seed` 的 1 万条成交 lot 上补齐结算义务、投递、资金分配、恢复收口、
//! 入站邮件与退件案件/领回事实：义务 3.5 万、投递约 5.6 万、入站邮件 5 万余、退件案件 2,200（未结案
//! 592）、领回 1,068、恢复 200，远高于生产规模。每条查询同时测自定义计划与通用计划（sqlx 复用预备
//! 语句后可能切换），并测一次“现金领回证明晚于余额观察”的窗口。只写一次性克隆库，不接触真实数据或
//! 外部服务。

use std::collections::BTreeMap;

use auction_application::settlement::{SettlementReturnAdminPort, SettlementReturnListRequest};
use serde_json::json;
use sqlx::{PgPool, Row};

use crate::perf::{PlanSummary, explain, explain_generic, seed, write_report};
use crate::support;

/// 偿付报告事务的单条语句超时是 5 秒；合成历史远高于生产规模，每条查询仍只允许用掉其十分之一。
const STATEMENT_BUDGET_MS: f64 = 500.0;
/// 种子取模分布决定的退件案件、未结案案件与领回事实数量。
const SEEDED_CASES: i64 = 2_200;
const SEEDED_OPEN_CASES: i64 = 592;
const SEEDED_TAKES: i64 = 1_068;
/// Admin 列表默认页长，与 HTTP 缺省一致。
const PAGE_LIMIT: i64 = 50;

/// Admin 列表总数的筛选组合：标签与 `EXECUTE` 实参（`$1..$8`）。
const LIST_COUNTS: [(&str, &str); 2] = [
    ("list_count_open", "NULL, NULL, NULL, NULL, true, NULL, NULL, NULL"),
    ("list_count_all", "NULL, NULL, NULL, NULL, false, NULL, NULL, NULL"),
];

/// Admin 列表页的筛选组合：`$1..$8` 同总数，另加页长与偏移（偏移 2000 即第 41 页）。
const LIST_PAGES: [(&str, &str); 4] = [
    ("list_page_open_first", "NULL, NULL, NULL, NULL, true, NULL, NULL, NULL, 50, 0"),
    ("list_page_all_first", "NULL, NULL, NULL, NULL, false, NULL, NULL, NULL, 50, 0"),
    ("list_page_all_offset_2000", "NULL, NULL, NULL, NULL, false, NULL, NULL, NULL, 50, 2000"),
    (
        "list_page_main_held_payout",
        "'main', NULL, 'held', 'seller_payout', false, NULL, NULL, NULL, 50, 0",
    ),
];

/// 退件合成历史（以表 owner 执行，单个隐式事务）。两张辅助表只在种子期间存在：
/// `perf_return_seed` 给每个成交 lot 一个确定编号 n，`perf_return_dispatch` 按 n 的取模分布展开
/// 每个义务的派发、履约状态与退件案件。货款每 10 个 lot 退回 1 个、退款（就绪的 3/50）与 COD 各按
/// 固定比例退回；退回后的状态覆盖代管、补寄、待决定、Take 待执行/结果未知与留箱。另有 500 封无法
/// 绑定的退件（隔离、待核验、已复核）、每 50 个 lot 一条已收回的恢复记录及其 `cod_returned` 处置，
/// 以及与退件无关的 3 万条普通投递和 4.8 万封普通入站邮件。所有时刻都早于余额观察（稳态）。
const RETURN_HISTORY_SQL: &str = r#"
CREATE TABLE public.perf_return_seed AS
SELECT row_number() OVER (ORDER BY a.sequence, l.lot_number)::int AS n,
       l.id AS lot_id, a.id AS auction_id, a.world_id, l.equipment_id,
       l.leader_user_id AS buyer_id, l.current_price AS price, s.command_id,
       a.settled_at - interval '10 days' AS base_at
FROM public.lots l
JOIN public.auctions a ON a.id = l.auction_id
JOIN public.auction_settlements s ON s.auction_id = a.id;

CREATE TABLE public.perf_return_dispatch AS
WITH lots AS (
  SELECT s.*,
         CASE WHEN s.n % 10 = 3 THEN (ARRAY['held', 'held', 'held', 'held', 'held', 'held', 'resent',
              'awaiting_decision', 'take_pending', 'inbox_closed'])[1 + (s.n / 10) % 10] END AS payout_case,
         CASE WHEN s.n % 50 = 7 THEN (ARRAY['held', 'held', 'held', 'resent', 'awaiting_decision',
              'take_unknown', 'inbox_closed'])[1 + (s.n / 50) % 7] END AS refund_case,
         CASE WHEN s.n % 20 = 11 THEN (ARRAY['held', 'held', 'held', 'resent', 'awaiting_decision',
              'take_pending', 'take_unknown', 'inbox_closed'])[1 + (s.n / 20) % 8] END AS cod_case,
         s.n % 50 = 24 AS recovered,
         CASE WHEN s.n % 100 BETWEEN 95 AND 96 THEN 'enqueued' WHEN s.n % 100 = 97 THEN 'pending'
              WHEN s.n % 100 = 98 THEN 'failed' WHEN s.n % 100 = 99 THEN 'manual_reconcile'
              ELSE 'satisfied' END AS payout_state
  FROM public.perf_return_seed s
), rows AS (
  SELECT n, 'cod'::text AS kind, md5('cod-' || n)::uuid AS obligation_id, world_id AS dispatch_world,
         price AS amount, base_at, cod_case AS return_case,
         CASE WHEN cod_case = 'resent' THEN 'enqueued' WHEN cod_case IS NOT NULL OR recovered THEN 'failed'
              ELSE 'satisfied' END AS f_state,
         CASE WHEN recovered THEN 'recovery_reclaimed' WHEN cod_case = 'held' THEN 'settlement_return_held'
              WHEN cod_case IS NOT NULL AND cod_case <> 'resent' THEN 'settlement_mail_returned' END AS error_code,
         true AS dispatched, 'succeeded'::text AS d_status
  FROM lots
  UNION ALL
  SELECT n, 'handoff', md5('handoff-' || n)::uuid, 'isekai', 0, base_at, NULL, 'satisfied', NULL, true, 'succeeded'
  FROM lots WHERE world_id = 'isekai'
  UNION ALL
  SELECT n, 'payout', md5('payout-' || n)::uuid, 'main', price - price / 20, base_at, payout_case,
         CASE WHEN payout_case = 'resent' THEN 'enqueued' WHEN payout_case IS NOT NULL OR recovered THEN 'failed'
              ELSE payout_state END,
         CASE WHEN recovered THEN 'recovery_reclaimed' WHEN payout_case = 'held' THEN 'settlement_return_held'
              WHEN payout_case IS NOT NULL AND payout_case <> 'resent' THEN 'settlement_mail_returned'
              WHEN payout_state = 'failed' THEN 'collection_evidence_mismatch'
              WHEN payout_state = 'manual_reconcile' THEN 'send_outcome_unknown' END,
         payout_case IS NOT NULL OR (NOT recovered AND payout_state <> 'pending'),
         CASE WHEN payout_case IS NULL AND payout_state = 'manual_reconcile' THEN 'manual_reconcile'
              ELSE 'succeeded' END
  FROM lots
  UNION ALL
  SELECT n, 'refund', md5('refund-' || n)::uuid, 'main', price, base_at, refund_case,
         CASE WHEN refund_case = 'resent' THEN 'enqueued' WHEN refund_case IS NOT NULL THEN 'failed'
              WHEN n % 50 = 8 THEN 'satisfied' ELSE 'pending' END,
         CASE WHEN refund_case = 'held' THEN 'settlement_return_held'
              WHEN refund_case IS NOT NULL AND refund_case <> 'resent' THEN 'settlement_mail_returned' END,
         n % 50 <> 9, 'succeeded'
  FROM lots WHERE n % 50 IN (7, 8, 9)
)
SELECT r.*, r.base_at + interval '1 hour' AS sent_at,
       CASE WHEN r.return_case = 'resent' THEN 2 ELSE 1 END AS dispatch_no,
       md5('dlv-' || r.kind || '-' || r.n)::uuid AS first_delivery_id,
       CASE WHEN r.return_case = 'resent' THEN md5('dlv2-' || r.kind || '-' || r.n)::uuid END AS second_delivery_id,
       CASE WHEN r.return_case IN ('held', 'resent') THEN md5('take-' || r.kind || '-' || r.n)::uuid END AS take_delivery_id,
       CASE WHEN r.return_case IS NOT NULL THEN md5('mail-' || r.kind || '-' || r.n)::uuid END AS return_mail_id
FROM rows r;

INSERT INTO public.settlement_funding_accounts (uid, status, accounting_mode)
VALUES ('1001', 'ready', 'strict_cash');

INSERT INTO public.settlement_obligations
  (id, auction_id, lot_id, world_id, origin_command_id, policy_version, obligation_kind,
   business_state, recipient_user_id, recipient_game_identity, amount, carrier_credit,
   equipment_id, prerequisite_obligation_id, created_at, updated_at)
SELECT md5(o.kind || '-' || s.n)::uuid, s.auction_id, s.lot_id, s.world_id, s.command_id, 1,
       CASE o.kind
           WHEN 'cod' THEN CASE WHEN s.world_id = 'main' THEN 'winner_equipment_cod'
                                ELSE 'winner_payment_carrier_cod' END
           WHEN 'handoff' THEN 'winner_equipment_handoff'
           WHEN 'payout' THEN 'seller_payout'
           ELSE 'buyer_refund' END,
       CASE WHEN o.kind = 'refund' AND s.n % 50 NOT IN (7, 8, 9) THEN 'contingent' ELSE 'ready' END,
       CASE WHEN o.kind = 'payout' THEN NULL ELSE s.buyer_id END,
       CASE WHEN o.kind = 'payout' THEN 'perf-seller-' || (s.n % 300) END,
       CASE o.kind WHEN 'handoff' THEN 0 WHEN 'payout' THEN s.price - s.price / 20 ELSE s.price END,
       CASE WHEN o.kind = 'cod' AND s.world_id = 'isekai' THEN 1 ELSE 0 END,
       s.equipment_id,
       CASE WHEN o.kind = 'cod' THEN NULL ELSE md5('cod-' || s.n)::uuid END,
       s.base_at, s.base_at
FROM public.perf_return_seed s
CROSS JOIN (VALUES ('cod'), ('handoff'), ('payout'), ('refund')) AS o(kind)
WHERE o.kind <> 'handoff' OR s.world_id = 'isekai';

INSERT INTO public.deliveries
  (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status,
   fencing_version, attempt_count, claimed_at, sending_at, completed_at, external_reference,
   failure_code, result_metadata, created_at, updated_at)
SELECT x.delivery_id, md5('cmd-' || x.delivery_id)::uuid,
       decode(md5('dg-' || x.delivery_id) || md5('dh-' || x.delivery_id), 'hex'),
       x.origin_kind, x.origin_id, x.recipient, x.payload, x.status, 1, 1, x.at, x.at,
       x.at + interval '1 minute',
       CASE WHEN x.status = 'succeeded' THEN 'mooglemail:' || x.world || ':' || x.operation || ':'
            || (1000000 + row_number() OVER (ORDER BY x.delivery_id)) END,
       CASE WHEN x.status = 'manual_reconcile' THEN 'send_outcome_unknown' END,
       '{}'::jsonb, x.at, x.at + interval '1 minute'
FROM (
  SELECT d.first_delivery_id AS delivery_id, 'settlement' AS origin_kind,
         d.obligation_id::text AS origin_id, 'perf-recipient-' || (d.n % 500) AS recipient,
         d.dispatch_world AS world, 'send' AS operation, d.d_status AS status, d.sent_at AS at,
         jsonb_build_object('schema_version', 1, 'operation', 'send', 'world', d.dispatch_world,
             'template', 'settlement_' || d.kind,
             'credits', CASE WHEN d.kind IN ('payout', 'refund') THEN d.amount ELSE 0 END,
             'cod', CASE WHEN d.kind = 'cod' THEN d.amount ELSE 0 END,
             'attachment_ids', '[]'::jsonb) AS payload
  FROM public.perf_return_dispatch d WHERE d.dispatched
  UNION ALL
  SELECT d.second_delivery_id, 'settlement', d.obligation_id::text,
         'perf-recipient-' || (d.n % 500), d.dispatch_world, 'send', 'succeeded',
         d.sent_at + interval '5 days',
         jsonb_build_object('schema_version', 1, 'operation', 'send', 'world', d.dispatch_world,
             'template', 'settlement_' || d.kind,
             'credits', CASE WHEN d.kind IN ('payout', 'refund') THEN d.amount ELSE 0 END,
             'cod', CASE WHEN d.kind = 'cod' THEN d.amount ELSE 0 END,
             'attachment_ids', '[]'::jsonb)
  FROM public.perf_return_dispatch d WHERE d.second_delivery_id IS NOT NULL
  UNION ALL
  SELECT d.take_delivery_id, 'settlement_return_action',
         'settlement_return:' || d.return_mail_id, 'MoogleMail', d.dispatch_world, 'take',
         'succeeded', d.sent_at + interval '3 days',
         jsonb_build_object('schema_version', 1, 'operation', 'take', 'world', d.dispatch_world,
             'template', 'settlement_return_take')
  FROM public.perf_return_dispatch d WHERE d.take_delivery_id IS NOT NULL
  UNION ALL
  SELECT md5('dlv-misc-' || g)::uuid,
         (ARRAY['user_key', 'operations_alert', 'lot_watch'])[1 + g % 3], 'misc-' || g,
         'perf-recipient-' || (g % 500), 'main', 'send', 'succeeded',
         now() - ((g * 1200) || ' seconds')::interval,
         jsonb_build_object('schema_version', 1, 'operation', 'send', 'world', 'main',
             'template', 'misc', 'credits', 0, 'cod', 0, 'attachment_ids', '[]'::jsonb)
  FROM generate_series(1, 30000) AS g
) AS x;

INSERT INTO public.settlement_obligation_fulfillments
  (obligation_id, execution_state, state_version, dispatch_world, delivery_operation, delivery_id,
   last_error_code, completed_at, created_at, updated_at, dispatch_no)
SELECT d.obligation_id, d.f_state, 3, d.dispatch_world, 'send',
       CASE WHEN NOT d.dispatched THEN NULL
            ELSE COALESCE(d.second_delivery_id, d.first_delivery_id) END,
       d.error_code,
       CASE WHEN d.f_state IN ('satisfied', 'failed', 'manual_reconcile') THEN d.sent_at + interval '6 days' END,
       d.base_at, d.sent_at + interval '6 days', d.dispatch_no
FROM public.perf_return_dispatch d;

INSERT INTO public.settlement_funding_allocations
  (id, uid, purpose, lot_id, obligation_id, amount, state, delivery_id, state_version, created_at)
SELECT md5('alloc-' || d.kind || '-' || d.n)::uuid, '1001',
       CASE d.kind WHEN 'payout' THEN 'seller' ELSE 'refund' END, s.lot_id, d.obligation_id,
       d.amount,
       CASE WHEN NOT d.dispatched OR d.return_case = 'held' OR d.error_code = 'recovery_reclaimed'
                THEN 'available'
            WHEN d.f_state = 'manual_reconcile' THEN 'unknown' ELSE 'spent' END,
       CASE WHEN NOT d.dispatched OR d.return_case = 'held' OR d.error_code = 'recovery_reclaimed'
                THEN NULL
            ELSE COALESCE(d.second_delivery_id, d.first_delivery_id) END,
       3, d.base_at
FROM public.perf_return_dispatch d
JOIN public.perf_return_seed s ON s.n = d.n
WHERE d.kind IN ('payout', 'refund')
UNION ALL
SELECT md5('alloc-tax-' || s.n)::uuid, '1001', 'tax', s.lot_id, NULL, s.price / 20, 'available',
       NULL, 1, s.base_at
FROM public.perf_return_seed s;

INSERT INTO public.settlement_recoveries
  (id, auction_id, lot_id, world_id, equipment_id, winner_user_id, payment_obligation_id,
   equipment_obligation_id, seller_payout_obligation_id, buyer_refund_obligation_id,
   policy_version, payment_deadline_at, stage, resolution, state_version, created_at, updated_at,
   completed_at)
SELECT md5('recovery-' || s.n)::uuid, s.auction_id, s.lot_id, s.world_id, s.equipment_id,
       s.buyer_id, md5('cod-' || s.n)::uuid,
       CASE WHEN s.world_id = 'isekai' THEN md5('handoff-' || s.n)::uuid END,
       md5('payout-' || s.n)::uuid, md5('refund-' || s.n)::uuid, 1,
       s.base_at + interval '7 days', 'completed', 'reclaimed', 4, s.base_at,
       s.base_at + interval '9 days', s.base_at + interval '9 days'
FROM public.perf_return_seed s WHERE s.n % 50 = 24;

INSERT INTO public.delivery_dispositions
  (delivery_id, fencing_version, kind, settlement_recovery_id, actor_source, actor_id, reason_code,
   resolves_alert, occurred_at)
SELECT md5('dlv-cod-' || s.n)::uuid, 1, 'cod_returned', md5('recovery-' || s.n)::uuid, 'worker',
       'recovery', 'recovery_reclaimed', true, s.base_at + interval '9 days'
FROM public.perf_return_seed s WHERE s.n % 50 = 24;

INSERT INTO public.inbound_mail
  (id, mailbox_key, external_mail_id, world_id, mail_kind, content_digest, received_at, status,
   decision_code, decision_metadata, processed_at, created_at)
SELECT x.id, x.world || ':bot', x.external_mail_id, x.world, x.mail_kind,
       decode(md5('in-' || x.id) || md5('in2-' || x.id), 'hex'), x.received_at, 'accepted',
       x.decision_code, x.metadata, x.received_at + interval '5 seconds',
       x.received_at + interval '5 seconds'
FROM (
  SELECT md5('inbound-' || g)::uuid AS id, (100000 + g)::text AS external_mail_id,
         CASE WHEN g % 4 = 0 THEN 'isekai' ELSE 'main' END AS world,
         (ARRAY['bid', 'delivery', 'equipment', 'registration', 'delivery', 'bid'])[1 + g % 6] AS mail_kind,
         now() - ((g * 900) || ' seconds')::interval AS received_at,
         (ARRAY['bid_accepted', 'cod_paid', 'equipment_registered', 'user_registered', 'cod_paid',
                'bid_rejected'])[1 + g % 6] AS decision_code,
         '{}'::jsonb AS metadata
  FROM generate_series(1, 48000) AS g
  UNION ALL
  SELECT d.return_mail_id, (700000 + row_number() OVER (ORDER BY d.return_mail_id))::text,
         d.dispatch_world, 'delivery', d.sent_at + interval '2 days', 'settlement_recovery_returned',
         jsonb_build_object('obligation_id', d.obligation_id::text,
             'credits', CASE WHEN d.kind IN ('payout', 'refund') THEN d.amount ELSE 0 END, 'cod', 0,
             'attachment_ids', CASE WHEN d.kind = 'cod' THEN jsonb_build_array((900000 + d.n)::text)
                                    ELSE '[]'::jsonb END)
  FROM public.perf_return_dispatch d WHERE d.return_mail_id IS NOT NULL
  UNION ALL
  SELECT md5('mail-unbound-' || g)::uuid, (800000 + g)::text, 'main', 'delivery',
         now() - ((g * 21600) || ' seconds')::interval, 'settlement_recovery_returned',
         jsonb_build_object('obligation_id', md5('payout-' || (g * 7 % 10000 + 1))::text,
             'credits', 1000, 'cod', 0, 'attachment_ids', '[]'::jsonb)
  FROM generate_series(1, 500) AS g
) AS x;

INSERT INTO public.settlement_return_evidence
  (return_mail_id, match_state, reason_code, obligation_id, delivery_id, previous_execution_state,
   recorded_at)
SELECT d.return_mail_id, 'fulfillment_terminal', 'returned_assets_match', d.obligation_id,
       d.first_delivery_id, 'satisfied', d.sent_at + interval '2 days 5 seconds'
FROM public.perf_return_dispatch d WHERE d.return_mail_id IS NOT NULL
UNION ALL
SELECT md5('mail-unbound-' || g)::uuid, CASE WHEN g % 2 = 0 THEN 'unbound' ELSE 'untrusted' END,
       CASE WHEN g % 2 = 0 THEN 'dispatch_already_returned' ELSE 'source_unproven' END,
       CASE WHEN g % 2 = 0 THEN md5('payout-' || (g * 7 % 10000 + 1))::uuid END, NULL, NULL,
       now() - ((g * 21600) || ' seconds')::interval + interval '5 seconds'
FROM generate_series(1, 500) AS g;

INSERT INTO public.settlement_return_cases
  (return_mail_id, world_id, obligation_id, delivery_id, dispatch_no, status, reason_code, plan,
   resend_limit, resend_dispatch_no, case_version, created_at, updated_at, closed_at)
SELECT d.return_mail_id, d.dispatch_world, d.obligation_id, d.first_delivery_id, 1, d.return_case,
       'returned_assets_match',
       CASE WHEN d.return_case IN ('take_pending', 'take_unknown', 'held') THEN 'hold'
            WHEN d.return_case = 'resent' THEN 'resend' END,
       3, CASE WHEN d.return_case = 'resent' THEN 2 END,
       CASE WHEN d.return_case IN ('held', 'resent', 'inbox_closed') THEN 4 ELSE 2 END,
       d.sent_at + interval '2 days 5 seconds', d.sent_at + interval '4 days',
       CASE WHEN d.return_case IN ('held', 'resent', 'inbox_closed') THEN d.sent_at + interval '4 days' END
FROM public.perf_return_dispatch d WHERE d.return_mail_id IS NOT NULL
UNION ALL
SELECT md5('mail-unbound-' || g)::uuid, 'main',
       CASE WHEN g % 2 = 0 THEN md5('payout-' || (g * 7 % 10000 + 1))::uuid END, NULL, NULL,
       CASE WHEN g % 10 = 0 THEN 'recheck_pending' WHEN g % 5 = 1 THEN 'quarantined' ELSE 'reviewed' END,
       CASE WHEN g % 2 = 0 THEN 'dispatch_already_returned' ELSE 'source_unproven' END,
       NULL, 3, NULL, CASE WHEN g % 10 = 0 OR g % 5 = 1 THEN 1 ELSE 2 END,
       now() - ((g * 21600) || ' seconds')::interval + interval '5 seconds',
       now() - ((g * 21600) || ' seconds')::interval + interval '1 day',
       CASE WHEN g % 10 <> 0 AND g % 5 <> 1
            THEN now() - ((g * 21600) || ' seconds')::interval + interval '1 day' END
FROM generate_series(1, 500) AS g;

INSERT INTO public.settlement_return_takes
  (return_mail_id, take_delivery_id, original_delivery_id, obligation_id, world_id,
   external_mail_id, credits, cod, attachment_ids, proof_digest, proven_at)
SELECT d.return_mail_id, d.take_delivery_id, d.first_delivery_id, d.obligation_id, i.world_id,
       i.external_mail_id, CASE WHEN d.kind IN ('payout', 'refund') THEN d.amount ELSE 0 END, 0,
       CASE WHEN d.kind = 'cod' THEN ARRAY[(900000 + d.n)::text] ELSE '{}'::text[] END,
       decode(md5('proof-' || d.return_mail_id) || md5('proof2-' || d.return_mail_id), 'hex'),
       d.sent_at + interval '3 days 1 minute'
FROM public.perf_return_dispatch d
JOIN public.inbound_mail i ON i.id = d.return_mail_id
WHERE d.take_delivery_id IS NOT NULL;

UPDATE public.settlement_recovery_health
SET balance_credits = 123456789, balance_observed_at = now() - interval '5 minutes',
    balance_status = 'ok', evidence_last_success_at = now() - interval '5 minutes',
    evidence_status = 'fresh', updated_at = now();

DROP TABLE public.perf_return_dispatch;

DROP TABLE public.perf_return_seed;
"#;

/// 写入退件合成历史并以表 owner 收集统计信息（运行账号执行 ANALYZE 只会跳过）。
async fn seed_returns(owner: &PgPool) -> Result<(), String> {
    sqlx::raw_sql(RETURN_HISTORY_SQL)
        .execute(owner)
        .await
        .map_err(|error| format!("退件合成历史写入失败：{error}"))?;
    sqlx::query("ANALYZE")
        .execute(owner)
        .await
        .map_err(|error| format!("退件合成历史统计失败：{error}"))?;
    Ok(())
}

/// 偿付查询（main）的两项结论：代管负债计数与“余额早于领回证明”。
async fn solvency_main(pool: &PgPool, sql: &'static str) -> (i64, bool) {
    let row = sqlx::query(sql).bind("main").fetch_one(pool).await.expect("执行偿付查询");
    (
        row.try_get("custody_count").expect("代管计数"),
        row.try_get("balance_before_take").expect("余额早于领回"),
    )
}

/// 与偿付查询独立计算的代管负债：已领回且代管结案的现金义务案件。
async fn expected_custody(pool: &PgPool) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM public.settlement_return_cases AS c
         JOIN public.settlement_return_takes AS t ON t.return_mail_id = c.return_mail_id
         JOIN public.settlement_obligations AS o ON o.id = c.obligation_id
         WHERE c.status = 'held' AND o.obligation_kind IN ('seller_payout', 'buyer_refund')",
    )
    .fetch_one(pool)
    .await
    .expect("统计代管负债")
}

/// 同一条查询分别以自定义计划与强制通用计划测量，按标签记入两张表。
async fn measure(
    pool: &PgPool,
    sql: &str,
    arguments: &str,
    label: &str,
    custom: &mut BTreeMap<String, PlanSummary>,
    generic: &mut BTreeMap<String, PlanSummary>,
) {
    custom.insert(
        label.to_owned(),
        explain(pool, sql, arguments).await.unwrap_or_else(|error| panic!("{label}：{error}")),
    );
    generic.insert(
        label.to_owned(),
        explain_generic(pool, sql, arguments)
            .await
            .unwrap_or_else(|error| panic!("{label} 通用计划：{error}")),
    );
}

#[tokio::test]
#[ignore = "热点基准：显式运行，不在默认 test-db 门禁中执行"]
async fn perf_settlement_return_solvency_and_list_plans() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接测试数据库");
    let pool = store.postgres_pool().clone();
    let maintenance = database.maintenance_store().await.expect("维护账号连接");
    let owner_pool = maintenance.postgres_pool().clone();
    seed(&pool, &owner_pool).await.expect("写入合成成交历史");
    seed_returns(&owner_pool).await.expect("写入合成退件历史");

    let seeded: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.settlement_return_cases),
                (SELECT count(*) FROM public.settlement_return_cases WHERE closed_at IS NULL),
                (SELECT count(*) FROM public.settlement_return_takes)",
    )
    .fetch_one(&pool)
    .await
    .expect("统计合成退件");
    assert_eq!(seeded, (SEEDED_CASES, SEEDED_OPEN_CASES, SEEDED_TAKES), "合成退件规模应与种子一致");

    // 结果先于计划核对：列表经真实端口映射全部种子行，偿付查询读到代管负债且稳态不判定余额过期。
    let open_page = store
        .list_settlement_returns(SettlementReturnListRequest {
            open_only: true,
            limit: PAGE_LIMIT,
            ..Default::default()
        })
        .await
        .expect("Admin 列表首页");
    assert_eq!(
        (open_page.total, open_page.items.len()),
        (SEEDED_OPEN_CASES, 50),
        "未结案首页应按真实端口映射出整页"
    );
    let solvency_sql = auction_storage::solvency_benchmark_sql::solvency_world_sql();
    let (custody, before_take) = solvency_main(&pool, solvency_sql).await;
    assert_eq!(custody, expected_custody(&pool).await, "偿付查询的代管负债应与案件事实一致");
    assert!(!before_take, "余额观察晚于全部领回证明时不得判定余额早于领回");

    let mut custom = BTreeMap::new();
    let mut generic = BTreeMap::new();
    for world in ["main", "isekai"] {
        let label = format!("solvency_{world}");
        measure(&pool, solvency_sql, &format!("'{world}'"), &label, &mut custom, &mut generic)
            .await;
    }
    let count_sql = auction_storage::settlement_return_benchmark_sql::list_count_sql();
    for (label, arguments) in LIST_COUNTS {
        measure(&pool, count_sql, arguments, label, &mut custom, &mut generic).await;
    }
    let page_sql = auction_storage::settlement_return_benchmark_sql::list_page_sql();
    for (label, arguments) in LIST_PAGES {
        measure(&pool, page_sql, arguments, label, &mut custom, &mut generic).await;
    }

    // 领回窗口：最新现金领回证明晚于余额观察，`balance_before_take` 必须读到它。
    sqlx::query(
        "UPDATE public.settlement_recovery_health
         SET balance_observed_at = (
             SELECT max(proven_at) - interval '1 day'
             FROM public.settlement_return_takes WHERE credits > 0)
         WHERE world_id = 'main'",
    )
    .execute(&owner_pool)
    .await
    .expect("把余额观察移到最新领回之前");
    let (_, window_before_take) = solvency_main(&pool, solvency_sql).await;
    assert!(window_before_take, "现金领回证明晚于余额观察时必须判定余额早于领回");
    measure(&pool, solvency_sql, "'main'", "solvency_main_take_window", &mut custom, &mut generic)
        .await;

    for (mode, plans) in [("custom", &custom), ("generic", &generic)] {
        for (label, summary) in plans {
            println!(
                "{mode} {label}: {:.3}ms, buffers {}",
                summary.execution_ms,
                summary.shared_hit_blocks + summary.shared_read_blocks
            );
            assert!(
                summary.execution_ms <= STATEMENT_BUDGET_MS,
                "{mode} {label} 用时 {:.1}ms，超过语句预算 {STATEMENT_BUDGET_MS}ms",
                summary.execution_ms
            );
        }
    }

    let report = json!({
        "schema": "auction.settlement_return_plans.v1",
        "seeded_cases": SEEDED_CASES,
        "seeded_open_cases": SEEDED_OPEN_CASES,
        "seeded_takes": SEEDED_TAKES,
        "statement_budget_ms": STATEMENT_BUDGET_MS,
        "custom_plan": custom.iter().map(|(label, summary)| (label.clone(), summary.to_json())).collect::<BTreeMap<_, _>>(),
        "generic_plan": generic.iter().map(|(label, summary)| (label.clone(), summary.to_json())).collect::<BTreeMap<_, _>>(),
    });
    let path = write_report("settlement-return-plans", &report);
    println!("结算退件计划报告：{}", path.display());
    pool.close().await;
}
