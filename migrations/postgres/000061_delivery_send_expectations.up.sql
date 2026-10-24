-- 升级必须先停止旧 Worker；旧进程不会写入冻结身份，不得与新版并行授权 Send。
-- 不回填历史 sending/manual_reconcile；没有真实渲染证据的记录仍按保守旧证明处理。
CREATE TABLE public.delivery_send_attempts (
    delivery_id uuid NOT NULL REFERENCES public.deliveries(id) ON DELETE RESTRICT,
    fencing_version bigint NOT NULL CHECK (fencing_version > 0),
    command_id uuid NOT NULL CHECK (command_id <> '00000000-0000-0000-0000-000000000000'),
    request_digest bytea NOT NULL CHECK (octet_length(request_digest) = 32),
    world_id text NOT NULL CHECK (world_id IN ('main', 'isekai')),
    sending_at timestamptz NOT NULL CHECK (isfinite(sending_at)),
    expectation jsonb NOT NULL CHECK (
        jsonb_typeof(expectation) = 'object'
        AND expectation ?& ARRAY['world_id', 'recipient', 'subject', 'body_digest',
            'credits', 'cod', 'equipment_ids', 'credential']
        AND expectation - ARRAY['world_id', 'recipient', 'subject', 'body_digest',
            'credits', 'cod', 'equipment_ids', 'credential']::text[] = '{}'::jsonb
        AND expectation->>'world_id' = world_id
        AND jsonb_typeof(expectation->'recipient') = 'string'
        AND jsonb_typeof(expectation->'subject') = 'string'
        AND jsonb_typeof(expectation->'body_digest') = 'array'
        AND jsonb_array_length(expectation->'body_digest') = 32
        AND jsonb_typeof(expectation->'credits') = 'number'
        AND jsonb_typeof(expectation->'cod') = 'number'
        AND jsonb_typeof(expectation->'equipment_ids') = 'array'
        AND (
            expectation->'credential' = 'null'::jsonb
            OR (
                jsonb_typeof(expectation->'credential') = 'object'
                AND expectation->'credential' ?& ARRAY['credential_id', 'credential_version', 'runtime_generation']
                AND (expectation->'credential') - ARRAY['credential_id', 'credential_version', 'runtime_generation']::text[] = '{}'::jsonb
            )
        )
    ),
    PRIMARY KEY (delivery_id, fencing_version)
);

CREATE INDEX delivery_send_attempts_content_window
    ON public.delivery_send_attempts
        (world_id, (expectation->>'recipient'), (expectation->'body_digest'), sending_at);

-- world/MID 全局唯一比按可轮换的凭据 ID 分区更保守，不能把同一 Sent 证明给两次发送。
-- legacy 没有冻结 attempt 时保持空引用；这不是对旧渲染身份的猜测或回填。
CREATE TABLE public.delivery_sent_bindings (
    world_id text NOT NULL CHECK (world_id IN ('main', 'isekai')),
    external_mail_id text NOT NULL CHECK (
        length(external_mail_id) BETWEEN 1 AND 64 AND external_mail_id ~ '^[1-9][0-9]*$'
    ),
    delivery_id uuid NOT NULL REFERENCES public.deliveries(id) ON DELETE RESTRICT,
    fencing_version bigint NOT NULL CHECK (fencing_version > 0),
    expectation_fencing_version bigint CHECK (expectation_fencing_version = fencing_version),
    proven_at timestamptz NOT NULL DEFAULT clock_timestamp() CHECK (isfinite(proven_at)),
    PRIMARY KEY (world_id, external_mail_id),
    UNIQUE (delivery_id, fencing_version),
    FOREIGN KEY (delivery_id, expectation_fencing_version)
        REFERENCES public.delivery_send_attempts(delivery_id, fencing_version) ON DELETE RESTRICT
);

-- 两类事实只追加；管理员重排队可以更换 delivery 当前 fence，不能改写或删除旧证据。
CREATE FUNCTION public.reject_delivery_send_evidence_mutation() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'delivery send evidence is append-only' USING ERRCODE = '55000';
END;
$$;

CREATE TRIGGER delivery_send_attempts_append_only
BEFORE UPDATE OR DELETE ON public.delivery_send_attempts
FOR EACH ROW EXECUTE FUNCTION public.reject_delivery_send_evidence_mutation();

CREATE TRIGGER delivery_sent_bindings_append_only
BEFORE UPDATE OR DELETE ON public.delivery_sent_bindings
FOR EACH ROW EXECUTE FUNCTION public.reject_delivery_send_evidence_mutation();

-- 过期隔离会推进当前 CAS fence，但不会改变原 sending 身份；按实际发送签名找回原尝试。
-- 管理员重试清空 sending_at，下一次 Send 重新冻结；不能把旧 attempt 关联到新发送窗口。
CREATE FUNCTION public.delivery_current_send_attempt(delivery_id uuid)
RETURNS TABLE (sending_fencing_version bigint, expectation jsonb)
LANGUAGE sql STABLE AS $$
    SELECT attempt.fencing_version, attempt.expectation
    FROM public.deliveries delivery
    JOIN public.delivery_send_attempts attempt
        ON attempt.delivery_id=delivery.id
       AND attempt.command_id=delivery.command_id
       AND attempt.request_digest=delivery.request_digest
       AND attempt.sending_at=delivery.sending_at
       AND attempt.world_id=COALESCE(delivery.payload->>'world','main')
       AND attempt.expectation->>'recipient'=delivery.recipient
       AND attempt.fencing_version<=delivery.fencing_version
    WHERE delivery.id=$1
    ORDER BY attempt.fencing_version DESC
    LIMIT 1;
$$;

-- 下游资金/履约/退货只消费这一个强谓词；新 proof 标签不能仅凭字符串白名单获得旧证明能力。
CREATE FUNCTION public.delivery_has_frozen_sent_proof(
    delivery_id uuid, result_metadata jsonb, external_reference text
) RETURNS boolean
LANGUAGE sql STABLE AS $$
    SELECT COALESCE(
        $2->>'operation' = 'send'
        AND $2->'external_reference_proven' = 'true'::jsonb
        AND (
            ($2->>'proof' = 'sent_exact_payload_v1' AND $2->'sent_verified' = 'true'::jsonb)
            OR ($2->>'proof' = 'sent_recovered_frozen_v1' AND $2->'sent_recovered' = 'true'::jsonb)
        )
        AND EXISTS (
            SELECT 1 FROM public.delivery_send_attempts attempt
            JOIN public.delivery_sent_bindings binding
                ON binding.delivery_id=attempt.delivery_id
               AND binding.fencing_version=attempt.fencing_version
               AND binding.expectation_fencing_version=attempt.fencing_version
               AND binding.world_id=attempt.world_id
            JOIN public.deliveries delivery
                ON delivery.id=attempt.delivery_id
               AND attempt.fencing_version=(
                   SELECT sending_fencing_version
                   FROM public.delivery_current_send_attempt(delivery.id)
               )
            WHERE attempt.delivery_id=$1 AND delivery.status='succeeded'
              AND delivery.external_reference=$3 AND delivery.result_metadata=$2
              AND $2->'send_expectation'=attempt.expectation
              AND $2->'sending_fencing_version'=to_jsonb(attempt.fencing_version)
              AND $3='mooglemail:' || binding.world_id || ':send:' || binding.external_mail_id
        ),
        false
    );
$$;

DROP INDEX public.deliveries_mailbox_sweep_order;
CREATE INDEX deliveries_mailbox_sweep_order
    ON public.deliveries ((COALESCE(payload->>'world', 'main')), created_at, id)
    WHERE status = 'manual_reconcile'
      AND payload->>'operation' IN ('send', 'take')
      AND external_reference IS NULL
      AND sending_at IS NOT NULL
      AND completed_at IS NOT NULL;
