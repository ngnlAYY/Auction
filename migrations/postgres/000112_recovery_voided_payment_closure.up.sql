-- 历史投影修复：Admin 已确认收回（reclaimed）或取消（cancelled）的成交撤销，原付款履约与依赖
-- 它的履约仍停在未终结状态时，按运行期确认事务的同一规则收口。
-- owner: auction-storage（恢复确认与结算履约投影）
--
-- 背景：此前的 Admin 确认只重列装备、落定 resolution 并冲正补偿池，原付款履约（以及依赖它的
--   卖方货款、isekai 装备交付）仍是 pending/prepared/enqueued/manual_reconcile，领取 SQL 也不按
--   resolution 过滤，Worker 可能继续领取已撤销的付款。运行期已改为在确认事务内收口，并在领取与
--   最终事务复核 resolution。
--
-- 只修复能证明“确认、退件、Take 与被处置的发送尝试”彼此一致的记录，全部条件同时成立：
--   1. 确认：恢复记录 resolution 为 reclaimed/cancelled，且命令账本 settlement_recovery_commands 有
--      同一恢复记录、同一 action、result_code = 'ok' 的确认命令（Admin 确认的不可变证据）；
--   2. 原付款：履约为 enqueued（已寄出、等待领取证据）且没有未过期的领取；原付款投递是该义务唯一
--      的结算派发，已成功寄出（status = succeeded）。结果未知（manual_reconcile、写入未知）、失败
--      或仍在队列的投递，以及履约已转人工核对的记录都不修；
--   3. 退件：恢复记录当前退件来源可证明（系统发件人 MoogleMail，主题为 `[RTS] ` + 付款义务的
--      canonical 主题），证据绑定原付款义务，并已被该恢复记录消费；
--   4. Take：确认种类对应的收回动作（reclaimed 为 reclaim_equipment_take，cancelled 为
--      cancel_carrier_take）已满足，来源邮件、证明引用与恢复记录的 custody 证明都指向这封退件，
--      Take 投递按同一 MID 在退件所在世界成功领取；
--   5. 被处置的发送能唯一归属（与运行期确认同一规则）：原付款义务全部结算投递上每一次可能已到达
--      远端的发送（冻结发送尝试、没有冻结尝试的旧发送围栏、恢复后外部核对隔离的发送、没有冻结
--      尝试的旧 Sent 绑定）都对应一封本恢复记录已消费的系统退件，两者计数相等；只有投递当前发送
--      能凭投递行证明确定没有外写（failed、无远端引用、结果明确记录 write_started=false 与
--      outcome_unknown=false，当前 fence 无 Sent 绑定），其余一律按可能外达计数。任何发送都早于
--      当前退件的截止时刻（恢复记录消费该退件的时刻，与退件收件时刻加 3 分钟取早），最早的发送
--      早于第一封已消费退件的截止；确认之后原投递也没有新的 Admin 命令（新 fence）。截止之前的
--      两次发送、同一分钟内的多次发送或时钟偏差下落在截止之前的重发都不能唯一归属，保持原样；
--   6. 依赖履约（卖方货款；isekai 另有装备交付）要么已是 failed，要么是从未准备、没有未过期领取
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
    JOIN public.settlement_recovery_commands AS confirmation
      ON confirmation.recovery_id = recovery.id
     AND confirmation.action = recovery.resolution
     AND confirmation.result_code = 'ok'
    JOIN public.settlement_obligation_fulfillments AS payment
      ON payment.obligation_id = recovery.payment_obligation_id
    JOIN public.deliveries AS delivery ON delivery.id = payment.delivery_id
    JOIN public.inbound_mail AS returned ON returned.id = recovery.return_mail_id
    JOIN public.settlement_recovery_return_consumptions AS consumed
      ON consumed.return_mail_id = returned.id
     AND consumed.recovery_id = recovery.id
    JOIN public.settlement_recovery_actions AS take
      ON take.recovery_id = recovery.id
     AND take.kind = CASE recovery.resolution
         WHEN 'reclaimed' THEN 'reclaim_equipment_take'
         ELSE 'cancel_carrier_take'
     END
    JOIN public.deliveries AS take_delivery ON take_delivery.id = take.delivery_id
    CROSS JOIN LATERAL (
        WITH obligation_deliveries AS (
            SELECT candidate.id,
                   candidate.status,
                   candidate.sending_at,
                   candidate.fencing_version,
                   candidate.external_reference,
                   candidate.result_metadata,
                   current_attempt.sending_fencing_version AS current_fence
            FROM public.deliveries AS candidate
            LEFT JOIN LATERAL public.delivery_current_send_attempt(candidate.id)
                AS current_attempt ON true
            WHERE candidate.origin_kind = 'settlement'
              AND candidate.origin_id = recovery.payment_obligation_id::text
        ),
        proven_unsent AS (
            SELECT unsent.id
            FROM obligation_deliveries AS unsent
            WHERE unsent.status = 'failed'
              AND unsent.external_reference IS NULL
              AND COALESCE(unsent.result_metadata
                      @> '{"write_started": false, "outcome_unknown": false}'::jsonb, false)
              AND NOT EXISTS (
                  SELECT 1
                  FROM public.delivery_sent_bindings AS unsent_binding
                  WHERE unsent_binding.delivery_id = unsent.id
                    AND unsent_binding.fencing_version
                        = COALESCE(unsent.current_fence, unsent.fencing_version)
              )
        ),
        sends AS (
            SELECT attempt.sending_at,
                   COALESCE(attempt.fencing_version = sent.current_fence, false)
                       AND sent.id IN (SELECT id FROM proven_unsent) AS unsent
            FROM obligation_deliveries AS sent
            JOIN public.delivery_send_attempts AS attempt ON attempt.delivery_id = sent.id
            UNION ALL
            SELECT sent.sending_at, sent.id IN (SELECT id FROM proven_unsent)
            FROM obligation_deliveries AS sent
            WHERE sent.sending_at IS NOT NULL AND sent.current_fence IS NULL
            UNION ALL
            SELECT quarantine.sent_at, false
            FROM obligation_deliveries AS sent
            JOIN public.delivery_external_quarantines AS quarantine
              ON quarantine.delivery_id = sent.id
            UNION ALL
            SELECT NULL::timestamptz, false
            FROM obligation_deliveries AS sent
            JOIN public.delivery_sent_bindings AS legacy_binding
              ON legacy_binding.delivery_id = sent.id
            WHERE legacy_binding.expectation_fencing_version IS NULL
              AND NOT (sent.sending_at IS NOT NULL AND sent.current_fence IS NULL
                       AND legacy_binding.fencing_version = sent.fencing_version)
        ),
        returns AS (
            SELECT LEAST(consumption.recorded_at,
                         return_mail.received_at + interval '3 minutes') AS cutoff,
                   return_mail.id = recovery.return_mail_id AS current_return
            FROM public.settlement_recovery_return_consumptions AS consumption
            JOIN public.inbound_mail AS return_mail ON return_mail.id = consumption.return_mail_id
            WHERE consumption.recovery_id = recovery.id
              AND return_mail.decision_metadata ->> 'obligation_id'
                  = recovery.payment_obligation_id::text
              AND COALESCE(
                  return_mail.sender = 'MoogleMail'
                  AND translate(return_mail.subject, 'ABCDEFGHIJKLMNOPQRSTUVWXYZ',
                                'abcdefghijklmnopqrstuvwxyz') COLLATE "C" IN (
                      '[rts] auction equipment delivery ['
                          || (return_mail.decision_metadata ->> 'obligation_id') || ']',
                      '[rts] auction payment request ['
                          || (return_mail.decision_metadata ->> 'obligation_id') || ']',
                      '[rts] auction seller payout ['
                          || (return_mail.decision_metadata ->> 'obligation_id') || ']',
                      '[rts] auction buyer refund ['
                          || (return_mail.decision_metadata ->> 'obligation_id') || ']'),
                  false)
        )
        SELECT COALESCE(
            (SELECT count(*) FROM returns WHERE returns.current_return) = 1
            AND NOT EXISTS (
                SELECT 1
                FROM sends
                CROSS JOIN returns
                WHERE returns.current_return AND sends.sending_at >= returns.cutoff
            )
            AND (SELECT count(*) FROM sends WHERE NOT sends.unsent)
                = (SELECT count(*) FROM returns)
            AND (SELECT min(sends.sending_at) FROM sends)
                < (SELECT min(returns.cutoff) FROM returns),
            false) AS attributable
    ) AS attribution
    WHERE recovery.resolution IN ('reclaimed', 'cancelled')
      -- 2. 原付款已寄出、等待领取证据，原投递是唯一派发且已成功。
      AND payment.execution_state = 'enqueued'
      AND (payment.claim_token IS NULL OR payment.claim_expires_at <= statement_timestamp())
      AND delivery.origin_kind = 'settlement'
      AND delivery.origin_id = recovery.payment_obligation_id::text
      AND delivery.payload_operation = 'send'
      AND delivery.status = 'succeeded'
      AND NOT EXISTS (
          SELECT 1
          FROM public.deliveries AS other
          WHERE other.origin_kind = 'settlement'
            AND other.origin_id = recovery.payment_obligation_id::text
            AND other.id <> delivery.id
      )
      -- 3. 当前退件来源可证明、绑定原付款义务，并已被本恢复记录消费。
      AND returned.mail_kind = 'delivery'
      AND returned.status = 'accepted'
      AND returned.decision_code = 'settlement_recovery_returned'
      AND returned.decision_metadata ->> 'obligation_id' = recovery.payment_obligation_id::text
      AND COALESCE(
          returned.sender = 'MoogleMail'
          AND translate(returned.subject, 'ABCDEFGHIJKLMNOPQRSTUVWXYZ',
                        'abcdefghijklmnopqrstuvwxyz') COLLATE "C" IN (
              '[rts] auction equipment delivery [' || recovery.payment_obligation_id::text || ']',
              '[rts] auction payment request [' || recovery.payment_obligation_id::text || ']'),
          false)
      -- 4. 确认种类对应的 Take 取回了这封退件，证明与 custody 一致。
      AND take.execution_state = 'satisfied'
      AND take.source_mail_id = returned.id
      AND take.proof_kind = 'delivery_recovery_take_succeeded'
      AND take.proof_reference = returned.external_mail_id
      AND take.proof_digest = recovery.custody_proof_digest
      AND recovery.custody_proof_kind = 'delivery_recovery_take_succeeded'
      AND recovery.custody_proof_reference = returned.external_mail_id
      AND take_delivery.origin_kind = 'settlement_recovery_action'
      AND take_delivery.origin_id = 'recovery:' || recovery.id::text
      AND take_delivery.status = 'succeeded'
      AND take_delivery.payload_operation = 'take'
      AND take_delivery.external_reference
          = 'mooglemail:' || returned.world_id || ':take:' || returned.external_mail_id
      -- 5. 被处置的发送能唯一归属，确认之后原投递没有新的 Admin 命令。
      AND delivery.sending_at IS NOT NULL
      AND attribution.attributable
      AND NOT EXISTS (
          SELECT 1
          FROM public.delivery_admin_commands AS admin
          WHERE admin.delivery_id = delivery.id
            AND admin.created_at >= confirmation.created_at
      )
      -- 6. 依赖履约已失败或从未开始。
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
    WHERE eligible.delivery_fence >= 1
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
