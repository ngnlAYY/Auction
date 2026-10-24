-- 升级必须停止旧 Worker；旧进程不能冻结 Take 原凭据，不得与新版并行授权动作。
-- 不回填历史 Take 身份；当前账号不能代替原发送账号，旧未知结果继续人工核对。
CREATE TABLE public.delivery_take_attempts (
    delivery_id uuid NOT NULL REFERENCES public.deliveries(id) ON DELETE RESTRICT,
    fencing_version bigint NOT NULL CHECK (fencing_version > 0),
    command_id uuid NOT NULL CHECK (command_id <> '00000000-0000-0000-0000-000000000000'),
    request_digest bytea NOT NULL CHECK (octet_length(request_digest) = 32),
    world_id text NOT NULL CHECK (world_id IN ('main', 'isekai')),
    sending_at timestamptz NOT NULL CHECK (isfinite(sending_at)),
    expectation jsonb NOT NULL CHECK (
        jsonb_typeof(expectation) = 'object'
        AND expectation ?& ARRAY['world_id', 'external_mail_id', 'credential']
        AND expectation - ARRAY['world_id', 'external_mail_id', 'credential']::text[] = '{}'::jsonb
        AND jsonb_typeof(expectation->'world_id') = 'string'
        AND expectation->>'world_id' = world_id
        AND jsonb_typeof(expectation->'external_mail_id') = 'string'
        AND length(expectation->>'external_mail_id') BETWEEN 1 AND 64
        AND expectation->>'external_mail_id' ~ '^[1-9][0-9]*$'
        AND (
            expectation->'credential' = 'null'::jsonb
            OR (
                jsonb_typeof(expectation->'credential') = 'object'
                AND expectation->'credential' ?& ARRAY['credential_id', 'credential_version', 'runtime_generation']
                AND (expectation->'credential') - ARRAY['credential_id', 'credential_version', 'runtime_generation']::text[] = '{}'::jsonb
                AND jsonb_typeof(expectation->'credential'->'credential_id') = 'string'
                AND expectation->'credential'->>'credential_id'
                    ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
                AND expectation->'credential'->>'credential_id' <> '00000000-0000-0000-0000-000000000000'
                AND jsonb_typeof(expectation->'credential'->'credential_version') = 'number'
                AND expectation->'credential'->>'credential_version' ~ '^[1-9][0-9]*$'
                AND (expectation->'credential'->'credential_version') <= '9223372036854775807'::jsonb
                AND jsonb_typeof(expectation->'credential'->'runtime_generation') = 'number'
                AND expectation->'credential'->>'runtime_generation' ~ '^[1-9][0-9]*$'
                AND (expectation->'credential'->'runtime_generation') <= '9223372036854775807'::jsonb
            )
        )
    ),
    PRIMARY KEY (delivery_id, fencing_version)
);

-- 复用同一追加事实保护；重排可推进 delivery 当前 fence，不能改写旧账号或尝试。
CREATE TRIGGER delivery_take_attempts_append_only
BEFORE UPDATE OR DELETE ON public.delivery_take_attempts
FOR EACH ROW EXECUTE FUNCTION public.reject_delivery_send_evidence_mutation();

-- 过期 Sending 会推进 CAS fence；按原发送签名取回 Take 尝试，而非误读当前 fence。
CREATE FUNCTION public.delivery_current_take_attempt(delivery_id uuid)
RETURNS TABLE (taking_fencing_version bigint, expectation jsonb)
LANGUAGE sql STABLE AS $$
    SELECT attempt.fencing_version, attempt.expectation
    FROM public.deliveries delivery
    JOIN public.delivery_take_attempts attempt
        ON attempt.delivery_id=delivery.id
       AND attempt.command_id=delivery.command_id
       AND attempt.request_digest=delivery.request_digest
       AND attempt.sending_at=delivery.sending_at
       AND attempt.world_id=delivery.payload->>'world'
       AND attempt.expectation->>'external_mail_id'=delivery.payload->>'external_mail_id'
       AND attempt.fencing_version<=delivery.fencing_version
    WHERE delivery.id=$1 AND delivery.payload->>'operation'='take'
    ORDER BY attempt.fencing_version DESC
    LIMIT 1;
$$;
