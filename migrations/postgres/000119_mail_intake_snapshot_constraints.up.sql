-- 收紧入站邮件展示快照的 JSON 类型、整数范围与载荷大小边界。
-- owner: auction-storage
-- Breaking：重建既有 CHECK；非法历史快照阻止升级，旧写入不兼容。
LOCK TABLE public.inbound_mail IN SHARE ROW EXCLUSIVE MODE;

-- 000035 检查 JSON number 后再窄化为 int/bigint；直接 cast 会令 CHECK 抛出范围错误。
ALTER TABLE public.inbound_mail
    DROP CONSTRAINT inbound_mail_message_display_check,
    ADD CONSTRAINT inbound_mail_message_display_check CHECK (
        message_display IS NULL
        OR (
            sender IS NOT NULL
            AND subject IS NOT NULL
            AND jsonb_typeof(message_display) = 'object'
            AND message_display ?& ARRAY[
                'schema_version', 'redaction_version', 'body', 'attachment_ids', 'credits', 'cod'
            ]
            AND CASE WHEN jsonb_typeof(message_display -> 'schema_version') = 'number'
                THEN (message_display ->> 'schema_version')::numeric = 1
                     AND trunc((message_display ->> 'schema_version')::numeric)
                         = (message_display ->> 'schema_version')::numeric
                ELSE false END
            AND CASE WHEN jsonb_typeof(message_display -> 'redaction_version') = 'number'
                THEN (message_display ->> 'redaction_version')::numeric = 1
                     AND trunc((message_display ->> 'redaction_version')::numeric)
                         = (message_display ->> 'redaction_version')::numeric
                ELSE false END
            AND jsonb_typeof(message_display -> 'body') = 'string'
            AND octet_length(message_display ->> 'body') <= 65536
            AND jsonb_typeof(message_display -> 'attachment_ids') = 'array'
            AND jsonb_array_length(message_display -> 'attachment_ids') <= 1000
            AND CASE WHEN jsonb_typeof(message_display -> 'credits') = 'number'
                THEN (message_display ->> 'credits')::numeric >= 0
                     AND (message_display ->> 'credits')::numeric <= 9223372036854775807::numeric
                     AND trunc((message_display ->> 'credits')::numeric)
                         = (message_display ->> 'credits')::numeric
                ELSE false END
            AND CASE WHEN jsonb_typeof(message_display -> 'cod') = 'number'
                THEN (message_display ->> 'cod')::numeric >= 0
                     AND (message_display ->> 'cod')::numeric <= 9223372036854775807::numeric
                     AND trunc((message_display ->> 'cod')::numeric)
                         = (message_display ->> 'cod')::numeric
                ELSE false END
            AND octet_length(message_display::text) <= 131072
        )
    ) NOT VALID;
ALTER TABLE public.inbound_mail VALIDATE CONSTRAINT inbound_mail_message_display_check;
INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (119, 'breaking');
