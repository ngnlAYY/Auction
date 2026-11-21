-- 隔离事实、恢复 Send/退件归属与退件 action 生命周期必须由数据库事务验证。
-- owner: auction-storage
-- Breaking：新增写入时约束并修复唯一可证明的案件投影；无法证明的历史闭账阻止升级。
LOCK TABLE
    public.delivery_external_quarantines,
    public.delivery_send_attempts,
    public.delivery_sent_bindings,
    public.deliveries,
    public.inbound_mail,
    public.settlement_obligation_fulfillments,
    public.settlement_obligations,
    public.settlement_recoveries,
    public.settlement_recovery_return_consumptions,
    public.settlement_return_actions,
    public.settlement_return_cases,
    public.settlement_return_evidence,
    public.settlement_return_events
IN SHARE ROW EXCLUSIVE MODE;

CREATE FUNCTION public.enforce_settlement_return_action_transition_000123()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $function$
BEGIN
    IF NEW.state_version <> OLD.state_version + 1
       OR NEW.return_mail_id IS DISTINCT FROM OLD.return_mail_id
       OR NEW.world_id IS DISTINCT FROM OLD.world_id
       OR NEW.source_external_mail_id IS DISTINCT FROM OLD.source_external_mail_id
       OR NEW.expected_credits IS DISTINCT FROM OLD.expected_credits
       OR NEW.expected_cod IS DISTINCT FROM OLD.expected_cod
       OR NEW.expected_attachment_ids IS DISTINCT FROM OLD.expected_attachment_ids
       OR NEW.created_at IS DISTINCT FROM OLD.created_at
       OR (OLD.delivery_id IS NOT NULL AND NEW.delivery_id IS DISTINCT FROM OLD.delivery_id)
       OR NOT (
           (OLD.execution_state = 'pending' AND NEW.execution_state = 'enqueued')
           OR (OLD.execution_state = 'enqueued'
               AND NEW.execution_state IN ('satisfied', 'manual_reconcile'))
           OR (OLD.execution_state = 'manual_reconcile' AND NEW.execution_state = 'enqueued')
       ) THEN
        RAISE EXCEPTION 'settlement return action lifecycle transition is invalid'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER settlement_return_action_lifecycle_000123
BEFORE UPDATE ON public.settlement_return_actions
FOR EACH ROW EXECUTE FUNCTION public.enforce_settlement_return_action_transition_000123();
-- 000112 的 count equality 不能把两封退件分别归给两次发送；每个退件截止区间须恰好有一条可能外达的 Send。
CREATE FUNCTION public.recovery_payment_return_attributable_000123(target_recovery_id uuid)
RETURNS boolean
LANGUAGE sql
STABLE
SET search_path = pg_catalog, public
AS $function$
WITH recovery AS (
    SELECT id, resolution, payment_obligation_id, return_mail_id
    FROM public.settlement_recoveries
    WHERE id = target_recovery_id
), obligation_deliveries AS (
    SELECT delivery.id, delivery.status, delivery.sending_at,
           delivery.fencing_version, delivery.external_reference,
           delivery.result_metadata,
           current_attempt.sending_fencing_version AS current_fence
    FROM public.deliveries AS delivery
    CROSS JOIN recovery
    LEFT JOIN LATERAL public.delivery_current_send_attempt(delivery.id) AS current_attempt
      ON true
    WHERE delivery.origin_kind = 'settlement'
      AND delivery.origin_id = recovery.payment_obligation_id::text
), proven_unsent AS (
    SELECT delivery.id
    FROM obligation_deliveries AS delivery
    WHERE delivery.status = 'failed'
      AND delivery.external_reference IS NULL
      AND COALESCE(delivery.result_metadata
              @> '{"write_started": false, "outcome_unknown": false}'::jsonb, false)
      AND NOT EXISTS (
          SELECT 1
          FROM public.delivery_sent_bindings AS binding
          WHERE binding.delivery_id = delivery.id
            AND binding.fencing_version = COALESCE(delivery.current_fence, delivery.fencing_version)
      )
), sends AS (
    SELECT attempt.sending_at,
           COALESCE(attempt.fencing_version = delivery.current_fence, false)
               AND delivery.id IN (SELECT id FROM proven_unsent) AS unsent
    FROM obligation_deliveries AS delivery
    JOIN public.delivery_send_attempts AS attempt ON attempt.delivery_id = delivery.id
    UNION ALL
    SELECT delivery.sending_at, delivery.id IN (SELECT id FROM proven_unsent)
    FROM obligation_deliveries AS delivery
    WHERE delivery.sending_at IS NOT NULL AND delivery.current_fence IS NULL
    UNION ALL
    SELECT quarantine.sent_at, false
    FROM obligation_deliveries AS delivery
    JOIN public.delivery_external_quarantines AS quarantine
      ON quarantine.delivery_id = delivery.id
    UNION ALL
    SELECT NULL::timestamptz, false
    FROM obligation_deliveries AS delivery
    JOIN public.delivery_sent_bindings AS binding ON binding.delivery_id = delivery.id
    WHERE binding.expectation_fencing_version IS NULL
      AND NOT (delivery.sending_at IS NOT NULL AND delivery.current_fence IS NULL
               AND binding.fencing_version = delivery.fencing_version)
), returns AS (
    SELECT consumption.return_mail_id,
           LEAST(consumption.recorded_at,
                 inbound.received_at + interval '3 minutes') AS cutoff,
           inbound.id = recovery.return_mail_id AS current_return
    FROM recovery
    JOIN public.settlement_recovery_return_consumptions AS consumption
      ON consumption.recovery_id = recovery.id
    JOIN public.inbound_mail AS inbound ON inbound.id = consumption.return_mail_id
    WHERE inbound.mail_kind = 'delivery'
      AND inbound.status = 'accepted'
      AND inbound.decision_code = 'settlement_recovery_returned'
      AND inbound.decision_metadata ->> 'obligation_id' = recovery.payment_obligation_id::text
      AND COALESCE(
          inbound.sender = 'MoogleMail'
          AND translate(inbound.subject, 'ABCDEFGHIJKLMNOPQRSTUVWXYZ',
                        'abcdefghijklmnopqrstuvwxyz') COLLATE "C" IN (
              '[rts] auction equipment delivery [' || recovery.payment_obligation_id::text || ']',
              '[rts] auction payment request [' || recovery.payment_obligation_id::text || ']',
              '[rts] auction seller payout [' || recovery.payment_obligation_id::text || ']',
              '[rts] auction buyer refund [' || recovery.payment_obligation_id::text || ']'),
          false)
), ordered_returns AS (
    SELECT return_row.*,
           lag(return_row.cutoff) OVER (ORDER BY return_row.cutoff, return_row.return_mail_id)
               AS previous_cutoff
    FROM returns AS return_row
), possible_sends AS (
    SELECT sending_at
    FROM sends
    WHERE NOT unsent
), interval_counts AS (
    SELECT return_row.return_mail_id, count(send_row.sending_at) AS send_count
    FROM ordered_returns AS return_row
    LEFT JOIN possible_sends AS send_row
      ON send_row.sending_at IS NOT NULL
     AND (return_row.previous_cutoff IS NULL OR send_row.sending_at >= return_row.previous_cutoff)
     AND send_row.sending_at < return_row.cutoff
    GROUP BY return_row.return_mail_id
)
SELECT COALESCE(
    (SELECT count(*) FROM returns WHERE current_return) = 1
    AND (SELECT count(*) FROM possible_sends) = (SELECT count(*) FROM returns)
    AND NOT EXISTS (SELECT 1 FROM possible_sends WHERE sending_at IS NULL)
    AND NOT EXISTS (SELECT 1 FROM interval_counts WHERE send_count <> 1)
    AND NOT EXISTS (
        SELECT 1 FROM possible_sends AS send_row
        WHERE NOT EXISTS (
            SELECT 1 FROM ordered_returns AS return_row
            WHERE (return_row.previous_cutoff IS NULL
                   OR send_row.sending_at >= return_row.previous_cutoff)
              AND send_row.sending_at < return_row.cutoff
        )
    )
    AND (SELECT min(sending_at) FROM possible_sends) < (SELECT min(cutoff) FROM returns)
    AND NOT EXISTS (
        SELECT 1 FROM possible_sends AS send_row
        CROSS JOIN ordered_returns AS return_row
        WHERE return_row.current_return AND send_row.sending_at >= return_row.cutoff
    ),
    false)
$function$;

DO $block$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM public.settlement_recoveries AS recovery
        JOIN public.settlement_obligation_fulfillments AS payment
          ON payment.obligation_id = recovery.payment_obligation_id
        WHERE recovery.resolution IN ('reclaimed', 'cancelled')
          AND payment.execution_state = 'failed'
          AND payment.last_error_code IN ('recovery_reclaimed', 'recovery_cancelled')
          AND NOT public.recovery_payment_return_attributable_000123(recovery.id)
    ) THEN
        RAISE EXCEPTION '000112 闭账记录的 Send/退件不能一对一归属；须人工核对，未改写任何事实'
            USING ERRCODE = '23514';
    END IF;
END
$block$;

CREATE FUNCTION public.validate_recovery_return_attribution_000123()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $function$
BEGIN
    IF NEW.resolution IN ('reclaimed', 'cancelled')
       AND EXISTS (
           SELECT 1 FROM public.settlement_obligation_fulfillments AS payment
           WHERE payment.obligation_id = NEW.payment_obligation_id
             AND payment.execution_state = 'failed'
             AND payment.last_error_code IN ('recovery_reclaimed', 'recovery_cancelled')
       )
       AND NOT public.recovery_payment_return_attributable_000123(NEW.id) THEN
        RAISE EXCEPTION 'recovery Send/退件无法唯一归属'
            USING ERRCODE = '23514';
    END IF;
    RETURN NULL;
END
$function$;
CREATE CONSTRAINT TRIGGER settlement_recovery_return_attribution_000123
AFTER INSERT OR UPDATE ON public.settlement_recoveries
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.validate_recovery_return_attribution_000123();

REVOKE ALL ON FUNCTION public.recovery_payment_return_attributable_000123(uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.recovery_payment_return_attributable_000123(uuid)
    TO auction_runtime;


-- 000114 的 earliest 判定和回填使用同一组具有 inbound_mail 的证据；孤儿证据不抢占可回填记录。
-- 只重绑唯一可证明的最早案件并追加事件，不改写原证据。
WITH repairable AS (
    SELECT case_row.return_mail_id, evidence.delivery_id, evidence.reason_code
    FROM public.settlement_return_cases AS case_row
    JOIN public.settlement_return_evidence AS evidence
      ON evidence.return_mail_id = case_row.return_mail_id
    JOIN public.inbound_mail AS inbound ON inbound.id = evidence.return_mail_id
    WHERE case_row.delivery_id IS NULL
      AND case_row.dispatch_no IS NULL
      AND case_row.status = 'quarantined'
      AND case_row.reason_code = 'dispatch_already_returned'
      AND evidence.delivery_id IS NOT NULL
      AND NOT EXISTS (
          SELECT 1
          FROM public.settlement_return_evidence AS earlier
          JOIN public.inbound_mail AS earlier_inbound
            ON earlier_inbound.id = earlier.return_mail_id
          WHERE earlier.delivery_id = evidence.delivery_id
            AND (earlier.recorded_at, earlier.return_mail_id)
                < (evidence.recorded_at, evidence.return_mail_id)
      )
      AND NOT EXISTS (
          SELECT 1 FROM public.settlement_return_cases AS existing
          WHERE existing.delivery_id = evidence.delivery_id
            AND existing.return_mail_id <> evidence.return_mail_id
      )
), repaired AS (
    UPDATE public.settlement_return_cases AS case_row
    SET delivery_id = repairable.delivery_id,
        dispatch_no = 1,
        status = 'awaiting_decision',
        reason_code = repairable.reason_code,
        case_version = case_row.case_version + 1,
        updated_at = GREATEST(case_row.updated_at, clock_timestamp())
    FROM repairable
    WHERE case_row.return_mail_id = repairable.return_mail_id
    RETURNING case_row.return_mail_id, case_row.case_version, case_row.status,
              case_row.updated_at
)
INSERT INTO public.settlement_return_events
    (return_mail_id, case_version, event_kind, status, details, occurred_at)
SELECT repaired.return_mail_id, repaired.case_version, 'recheck_rebound', repaired.status,
       jsonb_build_object('migration', 123, 'backfill_orphan_ordering_repaired', true),
       repaired.updated_at
FROM repaired;

-- 000114 的 pending action 是初始投影；任何越过 pending 的状态都必须已有状态推进版本。
ALTER TABLE public.settlement_return_actions
    ADD CONSTRAINT settlement_return_actions_pending_version_000123
        CHECK (execution_state <> 'pending' OR state_version = 1) NOT VALID,
    ADD CONSTRAINT settlement_return_actions_progressed_version_000123
        CHECK (execution_state = 'pending' OR state_version > 1) NOT VALID;
ALTER TABLE public.settlement_return_actions
    VALIDATE CONSTRAINT settlement_return_actions_pending_version_000123,
    VALIDATE CONSTRAINT settlement_return_actions_progressed_version_000123;
-- 隔离事实必须与当前 pending/claimed → manual_reconcile fence 推进在同一事务内绑定。
CREATE FUNCTION public.validate_external_quarantine_before_insert_000123()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $function$
DECLARE
    delivery_status text;
    current_fence bigint;
BEGIN
    SELECT delivery.status, delivery.fencing_version
    INTO delivery_status, current_fence
    FROM public.deliveries AS delivery
    WHERE delivery.id = NEW.delivery_id
    FOR UPDATE;
    IF NOT FOUND OR delivery_status NOT IN ('pending', 'claimed')
       OR current_fence IS DISTINCT FROM NEW.fencing_version THEN
        RAISE EXCEPTION 'quarantine must bind the current pending/claimed delivery fence'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END
$function$;

CREATE FUNCTION public.validate_external_quarantine_transition_000123()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $function$
DECLARE
    delivery_row record;
    evidence jsonb;
BEGIN
    SELECT delivery.status, delivery.fencing_version, delivery.failure_code,
           delivery.external_reference, delivery.result_metadata
    INTO delivery_row
    FROM public.deliveries AS delivery
    WHERE delivery.id = NEW.delivery_id;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'quarantine delivery disappeared before commit'
            USING ERRCODE = '23514';
    END IF;
    evidence := delivery_row.result_metadata -> 'external_recovery_quarantine';
    IF delivery_row.status IS DISTINCT FROM 'manual_reconcile'
       OR delivery_row.fencing_version IS DISTINCT FROM NEW.fencing_version + 1
       OR delivery_row.failure_code IS NULL
       OR delivery_row.failure_code NOT IN (
           'external_recovery_sent_match', 'external_recovery_payload_match'
       )
       OR delivery_row.external_reference IS NOT NULL
       OR delivery_row.result_metadata -> 'outcome_unknown' IS DISTINCT FROM 'true'::jsonb
       OR delivery_row.result_metadata ->> 'reason_code' IS DISTINCT FROM delivery_row.failure_code
       OR jsonb_typeof(evidence) IS DISTINCT FROM 'object'
       OR evidence ->> 'scan_id' IS DISTINCT FROM NEW.scan_id::text
       OR evidence ->> 'external_mail_id' IS DISTINCT FROM NEW.external_mail_id
       OR evidence ->> 'match_evidence' IS NULL
       OR evidence ->> 'match_evidence' NOT IN ('frozen_expectation', 'restored_payload')
       OR evidence -> 'sent_at' IS DISTINCT FROM to_jsonb(NEW.sent_at)
       OR evidence -> 'observed_at' IS DISTINCT FROM to_jsonb(NEW.observed_at) THEN
        RAISE EXCEPTION 'quarantine fact is not bound to its delivery fence transition'
            USING ERRCODE = '23514';
    END IF;
    RETURN NULL;
END
$function$;

CREATE TRIGGER delivery_external_quarantine_current_fence_000123
BEFORE INSERT ON public.delivery_external_quarantines
FOR EACH ROW EXECUTE FUNCTION public.validate_external_quarantine_before_insert_000123();
CREATE CONSTRAINT TRIGGER delivery_external_quarantine_transition_000123
AFTER INSERT ON public.delivery_external_quarantines
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.validate_external_quarantine_transition_000123();

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (123, 'breaking');
