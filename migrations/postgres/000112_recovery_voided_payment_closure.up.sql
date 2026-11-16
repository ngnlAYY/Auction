-- 历史投影修复：Admin 已确认收回（reclaimed）或取消（cancelled）的成交撤销，原付款履约与依赖
-- 它的履约仍停在未终结状态时，按运行期确认事务的同一规则收口。
-- owner: auction-storage（恢复确认与结算履约投影）
--
-- 背景：此前的 Admin 确认只重列装备、落定 resolution 并冲正补偿池，原付款履约（以及依赖它的
--   卖方货款、isekai 装备交付）仍是 pending/prepared/enqueued/manual_reconcile，领取 SQL 也不按
--   resolution 过滤，Worker 可能继续领取已撤销的付款。运行期已改为在确认事务内收口，并在领取与
--   最终事务复核 resolution。
--
-- 只修复全部条件同时成立、有确凿证据的记录：
--   1. 恢复记录 resolution 为 reclaimed/cancelled，且命令账本 settlement_recovery_commands 有同一
--      恢复记录、同一 action、result_code = 'ok' 的确认命令（Admin 确认的不可变证据）；
--   2. 原付款履约处于 pending/prepared/enqueued/manual_reconcile，且没有未过期的领取；
--   3. 原付款投递（若有）已不在 prepared/pending/claimed/sending，不存在未解决的外部写入；
--   4. 依赖履约（卖方货款；isekai 另有装备交付）要么已是 failed，要么是从未准备、没有未过期领取
--      的 pending。
--   任一条件不满足的记录保持原样，留给人工核对。不改写投递、发送证明、资金或恢复记录，不把
--   任何投递改成“未发送”；已是终态的履约（含 satisfied）一律不动。
--
-- 写入：付款履约 → failed（recovery_reclaimed / recovery_cancelled）；未开始的依赖履约 → failed
--   （recovery_sale_voided）；原付款投递当前 fence 上追加一条 cod_returned 处置事实（来源引用恢复
--   记录，操作者 system/migration-000112，同一投递/fence/种类已有处置时不重复）；每条收口追加一条
--   履约终态审计，最后追加一条迁移审计（含修复数量）。处置与审计都只追加。
--
-- 等级 breaking：改写既有履约行，并写入旧程序读侧不认识的失败码（旧分类口径会把它们计为未处置
--   的失败），不是纯新增；回退须按 RECOVERY.md 处理含 breaking 迁移的版本。迁移在停机门禁下
--   执行，Worker 已停止，不会与运行中的领取竞争。

LOCK TABLE public.settlement_obligation_fulfillments, public.delivery_dispositions
    IN SHARE ROW EXCLUSIVE MODE;

WITH eligible AS (
    SELECT recovery.id AS recovery_id,
           recovery.resolution,
           recovery.payment_obligation_id,
           recovery.seller_payout_obligation_id,
           recovery.equipment_obligation_id,
           recovery.cod_return_count,
           recovery.cod_return_limit,
           payment.execution_state AS payment_state,
           delivery.id AS delivery_id,
           delivery.fencing_version AS delivery_fence
    FROM public.settlement_recoveries AS recovery
    JOIN public.settlement_obligation_fulfillments AS payment
      ON payment.obligation_id = recovery.payment_obligation_id
    LEFT JOIN public.deliveries AS delivery ON delivery.id = payment.delivery_id
    WHERE recovery.resolution IN ('reclaimed', 'cancelled')
      AND EXISTS (
          SELECT 1
          FROM public.settlement_recovery_commands AS command
          WHERE command.recovery_id = recovery.id
            AND command.action = recovery.resolution
            AND command.result_code = 'ok'
      )
      AND payment.execution_state IN ('pending', 'prepared', 'enqueued', 'manual_reconcile')
      AND (payment.claim_token IS NULL OR payment.claim_expires_at <= statement_timestamp())
      AND (delivery.id IS NULL OR delivery.status IN ('succeeded', 'failed', 'manual_reconcile'))
      AND NOT EXISTS (
          SELECT 1
          FROM public.settlement_obligation_fulfillments AS dependent
          WHERE dependent.obligation_id IN (
                    recovery.seller_payout_obligation_id, recovery.equipment_obligation_id
                )
            AND NOT (
                dependent.execution_state = 'failed'
                OR (dependent.execution_state = 'pending'
                    AND dependent.delivery_id IS NULL
                    AND dependent.prepare_command_id IS NULL
                    AND (dependent.claim_token IS NULL
                         OR dependent.claim_expires_at <= statement_timestamp()))
            )
      )
),
closed_payments AS (
    UPDATE public.settlement_obligation_fulfillments AS fulfillment
    SET execution_state = 'failed',
        last_error_code = CASE eligible.resolution
            WHEN 'reclaimed' THEN 'recovery_reclaimed'
            ELSE 'recovery_cancelled'
        END,
        claim_token = NULL,
        claimed_at = NULL,
        claim_expires_at = NULL,
        next_attempt_at = NULL,
        proof_kind = NULL,
        proof_reference = NULL,
        proof_digest = NULL,
        proof_observed_at = NULL,
        completed_at = statement_timestamp(),
        state_version = fulfillment.state_version + 1,
        updated_at = statement_timestamp()
    FROM eligible
    WHERE fulfillment.obligation_id = eligible.payment_obligation_id
    RETURNING fulfillment.obligation_id, eligible.recovery_id,
              eligible.payment_state AS previous_state, fulfillment.last_error_code
),
closed_dependents AS (
    UPDATE public.settlement_obligation_fulfillments AS fulfillment
    SET execution_state = 'failed',
        last_error_code = 'recovery_sale_voided',
        claim_token = NULL,
        claimed_at = NULL,
        claim_expires_at = NULL,
        next_attempt_at = NULL,
        completed_at = statement_timestamp(),
        state_version = fulfillment.state_version + 1,
        updated_at = statement_timestamp()
    FROM eligible
    WHERE fulfillment.obligation_id IN (
              eligible.seller_payout_obligation_id, eligible.equipment_obligation_id
          )
      AND fulfillment.execution_state = 'pending'
      AND fulfillment.delivery_id IS NULL
      AND fulfillment.prepare_command_id IS NULL
    RETURNING fulfillment.obligation_id, eligible.recovery_id,
              'pending'::text AS previous_state, fulfillment.last_error_code
),
dispositions AS (
    INSERT INTO public.delivery_dispositions
        (delivery_id, fencing_version, kind, settlement_recovery_id, actor_source, actor_id,
         reason, reason_code, resolves_alert, detail)
    SELECT eligible.delivery_id,
           eligible.delivery_fence,
           'cod_returned',
           eligible.recovery_id,
           'system',
           'migration-000112',
           format('Admin confirmed %s after %s COD returns (limit %s); projection repaired by migration 000112',
                  eligible.resolution, eligible.cod_return_count, eligible.cod_return_limit),
           CASE eligible.resolution
               WHEN 'reclaimed' THEN 'recovery_reclaimed'
               ELSE 'recovery_cancelled'
           END,
           true,
           jsonb_build_object(
               'recovery_id', eligible.recovery_id,
               'payment_obligation_id', eligible.payment_obligation_id,
               'seller_payout_obligation_id', eligible.seller_payout_obligation_id,
               'equipment_obligation_id', eligible.equipment_obligation_id,
               'migration', '000112'
           )
    FROM eligible
    WHERE eligible.delivery_id IS NOT NULL
      AND eligible.delivery_fence >= 1
    ON CONFLICT (delivery_id, kind, fencing_version) DO NOTHING
    RETURNING id
),
closures AS (
    SELECT obligation_id, recovery_id, previous_state, last_error_code FROM closed_payments
    UNION ALL
    SELECT obligation_id, recovery_id, previous_state, last_error_code FROM closed_dependents
),
closure_audits AS (
    INSERT INTO public.audit_records
        (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
    SELECT gen_random_uuid(), 'system', 'migration-000112', 'auction.fulfillment.failed',
           'settlement_obligation', closures.obligation_id::text, 'success', 'system',
           jsonb_build_object(
               'state', 'failed',
               'previous_state', closures.previous_state,
               'error_code', closures.last_error_code,
               'recovery_id', closures.recovery_id,
               'migration', '000112'
           )
    FROM closures
    RETURNING id
)
INSERT INTO public.audit_records
    (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
SELECT gen_random_uuid(), 'system', 'migration-000112', 'system.migration', 'migration',
       '000112', 'success', 'system',
       jsonb_build_object(
           'summary', 'close payment and dependent fulfillments of admin-voided sales',
           'repaired_recoveries', (SELECT count(*) FROM eligible),
           'closed_payments', (SELECT count(*) FROM closed_payments),
           'closed_dependents', (SELECT count(*) FROM closed_dependents),
           'dispositions', (SELECT count(*) FROM dispositions),
           'closure_audits', (SELECT count(*) FROM closure_audits),
           'compatibility', 'breaking'
       );

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (112, 'breaking');
