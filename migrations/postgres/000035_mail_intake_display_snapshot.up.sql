ALTER TABLE public.inbound_mail
    ADD COLUMN sender text,
    ADD COLUMN subject text,
    ADD COLUMN message_display jsonb;

ALTER TABLE public.inbound_mail
    ADD CONSTRAINT inbound_mail_sender_subject_pair_check
        CHECK ((sender IS NULL) = (subject IS NULL)),
    ADD CONSTRAINT inbound_mail_sender_size_check
        CHECK (sender IS NULL OR (octet_length(sender) <= 256 AND btrim(sender) <> '')),
    ADD CONSTRAINT inbound_mail_subject_size_check
        CHECK (subject IS NULL OR (octet_length(subject) <= 512 AND btrim(subject) <> '')),
    ADD CONSTRAINT inbound_mail_message_display_check
        CHECK (
            message_display IS NULL
            OR (
                sender IS NOT NULL
                AND subject IS NOT NULL
                AND jsonb_typeof(message_display) = 'object'
                AND message_display ?& ARRAY[
                    'schema_version', 'redaction_version', 'body', 'attachment_ids', 'credits', 'cod'
                ]
                AND jsonb_typeof(message_display -> 'schema_version') = 'number'
                AND (message_display ->> 'schema_version')::int = 1
                AND jsonb_typeof(message_display -> 'redaction_version') = 'number'
                AND (message_display ->> 'redaction_version')::int = 1
                AND jsonb_typeof(message_display -> 'body') = 'string'
                AND octet_length(message_display ->> 'body') <= 65536
                AND jsonb_typeof(message_display -> 'attachment_ids') = 'array'
                AND jsonb_array_length(message_display -> 'attachment_ids') <= 1000
                AND jsonb_typeof(message_display -> 'credits') = 'number'
                AND (message_display ->> 'credits')::bigint >= 0
                AND jsonb_typeof(message_display -> 'cod') = 'number'
                AND (message_display ->> 'cod')::bigint >= 0
                AND octet_length(message_display::text) <= 131072
            )
        );
