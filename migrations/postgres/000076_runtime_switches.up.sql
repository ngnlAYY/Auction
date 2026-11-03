-- platform/storage owner：运行时行为选择与命令结果只追加，不参与业务金额、资格、幂等或锁。
-- compatible：纯新增表及其保护，旧程序不读取；不修改既有审计闭集、业务表或历史事实。
CREATE TABLE public.runtime_switch_events (
    command_id uuid PRIMARY KEY CHECK (command_id <> '00000000-0000-0000-0000-000000000000'),
    digest_version smallint NOT NULL CHECK (digest_version = 1),
    request_digest bytea NOT NULL CHECK (octet_length(request_digest) = 32),
    switch_name text NOT NULL CHECK (octet_length(switch_name) BETWEEN 1 AND 64
        AND switch_name ~ '^[a-z][a-z0-9_]*$'),
    revision bigint NOT NULL CHECK (revision > 0),
    expected_revision bigint NOT NULL CHECK (expected_revision >= 0 AND revision = expected_revision + 1),
    operation text NOT NULL CHECK (operation IN ('set', 'clear', 'expire')),
    value jsonb,
    expires_at timestamptz,
    expired_command_id uuid REFERENCES public.runtime_switch_events(command_id) ON DELETE RESTRICT,
    actor_id text NOT NULL CHECK (octet_length(actor_id) BETWEEN 1 AND 256 AND actor_id = btrim(actor_id)),
    entrypoint text NOT NULL CHECK (entrypoint IN ('admin', 'cli', 'system')),
    reason text NOT NULL CHECK (octet_length(reason) BETWEEN 1 AND 1024 AND reason = btrim(reason)),
    audit_id uuid NOT NULL UNIQUE REFERENCES public.audit_records(id) ON DELETE RESTRICT,
    result_payload jsonb NOT NULL CHECK (jsonb_typeof(result_payload) = 'object'),
    recorded_at timestamptz NOT NULL,
    CHECK ((operation = 'set' AND value IS NOT NULL
            AND jsonb_typeof(value) IN ('boolean', 'string') AND expired_command_id IS NULL
            AND (expires_at IS NULL OR expires_at > recorded_at))
        OR (operation = 'clear' AND value IS NULL AND expires_at IS NULL AND expired_command_id IS NULL)
        OR (operation = 'expire' AND value IS NULL AND expires_at IS NULL AND expired_command_id IS NOT NULL)),
    UNIQUE (switch_name, revision)
);

CREATE UNIQUE INDEX runtime_switch_events_expiry_idx
    ON public.runtime_switch_events (expired_command_id) WHERE operation = 'expire';
CREATE INDEX runtime_switch_events_latest_idx
    ON public.runtime_switch_events (switch_name, revision DESC);

CREATE FUNCTION public.reject_runtime_switch_mutation()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'runtime switch events are append-only' USING ERRCODE = '55000';
END;
$$;
CREATE TRIGGER runtime_switch_events_append_only
BEFORE UPDATE OR DELETE ON public.runtime_switch_events
FOR EACH ROW EXECUTE FUNCTION public.reject_runtime_switch_mutation();
CREATE TRIGGER runtime_switch_events_no_truncate
BEFORE TRUNCATE ON public.runtime_switch_events
FOR EACH STATEMENT EXECUTE FUNCTION public.reject_runtime_switch_mutation();

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (76, 'compatible');
