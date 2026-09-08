ALTER TABLE public.inbound_mail
    ADD COLUMN equipment_diagnostic jsonb;

ALTER TABLE public.inbound_mail
    ADD CONSTRAINT inbound_mail_equipment_diagnostic_check
        CHECK (
            equipment_diagnostic IS NULL
            OR (
                mail_kind = 'equipment'
                AND status = 'rejected'
                AND jsonb_typeof(equipment_diagnostic) = 'object'
                AND octet_length(equipment_diagnostic::text) <= 4096
                AND equipment_diagnostic ?& ARRAY[
                    'schema_version', 'equipment_id', 'attachment_position', 'stage',
                    'category', 'detail_code', 'http_status', 'elapsed_millis',
                    'evidence_id', 'evidence_status', 'retention_error'
                ]
                AND jsonb_typeof(equipment_diagnostic -> 'schema_version') = 'number'
                AND (equipment_diagnostic ->> 'schema_version') ~ '^[0-9]+$'
                AND (equipment_diagnostic ->> 'schema_version')::int = 1
                AND jsonb_typeof(equipment_diagnostic -> 'equipment_id') = 'string'
                AND equipment_diagnostic ->> 'equipment_id' ~ '^[0-9]{1,64}$'
                AND jsonb_typeof(equipment_diagnostic -> 'attachment_position') = 'number'
                AND (equipment_diagnostic ->> 'attachment_position') ~ '^[0-9]+$'
                AND (equipment_diagnostic ->> 'attachment_position')::int BETWEEN 1 AND 1000
                AND jsonb_typeof(equipment_diagnostic -> 'stage') = 'string'
                AND equipment_diagnostic ->> 'stage' IN ('request', 'response', 'parse', 'validate')
                AND jsonb_typeof(equipment_diagnostic -> 'category') = 'string'
                AND equipment_diagnostic ->> 'category' IN (
                    'configuration', 'credential_unavailable', 'session_expired', 'timeout',
                    'network', 'redirect', 'response_too_large', 'content_encoding',
                    'rate_limited', 'server_error', 'http_status', 'content_type',
                    'protocol_drift', 'identity_mismatch', 'snapshot_invalid', 'filter_rejected'
                )
                AND (jsonb_typeof(equipment_diagnostic -> 'detail_code') = 'null'
                    OR jsonb_typeof(equipment_diagnostic -> 'detail_code') = 'string')
                AND octet_length(COALESCE(equipment_diagnostic ->> 'detail_code', '')) <= 64
                AND (
                    (equipment_diagnostic ->> 'category' = 'snapshot_invalid'
                        AND equipment_diagnostic ->> 'detail_code' IN (
                            'adapter_invalid', 'fingerprint_invalid', 'equipment_id_invalid',
                            'name_invalid', 'category_invalid', 'level_invalid',
                            'collected_at_invalid', 'collected_at_future', 'attributes_invalid'
                        ))
                    OR (equipment_diagnostic ->> 'category' = 'filter_rejected'
                        AND equipment_diagnostic ->> 'detail_code' IN (
                            'quality_not_allowed', 'prefix_not_allowed', 'suffix_not_allowed',
                            'category_not_allowed', 'equipment_type_not_allowed',
                            'level_not_allowed', 'attributes_invalid', 'tradeable_required'
                        ))
                    OR (equipment_diagnostic ->> 'category' NOT IN ('snapshot_invalid', 'filter_rejected')
                        AND jsonb_typeof(equipment_diagnostic -> 'detail_code') = 'null')
                )
                AND (jsonb_typeof(equipment_diagnostic -> 'http_status') = 'null'
                    OR (
                        jsonb_typeof(equipment_diagnostic -> 'http_status') = 'number'
                        AND (equipment_diagnostic ->> 'http_status') ~ '^[0-9]+$'
                        AND (equipment_diagnostic ->> 'http_status')::int BETWEEN 100 AND 599
                    ))
                AND (jsonb_typeof(equipment_diagnostic -> 'elapsed_millis') = 'null'
                    OR (
                        jsonb_typeof(equipment_diagnostic -> 'elapsed_millis') = 'number'
                        AND (equipment_diagnostic ->> 'elapsed_millis') ~ '^[0-9]+$'
                        AND (equipment_diagnostic ->> 'elapsed_millis')::bigint BETWEEN 0 AND 300000
                    ))
                AND (jsonb_typeof(equipment_diagnostic -> 'evidence_id') = 'null'
                    OR (
                        jsonb_typeof(equipment_diagnostic -> 'evidence_id') = 'string'
                        AND equipment_diagnostic ->> 'evidence_id'
                            ~ '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$'
                        AND equipment_diagnostic ->> 'evidence_id'
                            <> '00000000-0000-0000-0000-000000000000'
                    ))
                AND jsonb_typeof(equipment_diagnostic -> 'evidence_status') = 'string'
                AND equipment_diagnostic ->> 'evidence_status' IN (
                    'retained', 'body_unavailable', 'save_failed',
                    'completion_unconfirmed', 'prepare_failed', 'disabled'
                )
                AND (
                    (equipment_diagnostic ->> 'evidence_status' IN ('retained', 'body_unavailable')
                        AND jsonb_typeof(equipment_diagnostic -> 'evidence_id') = 'string'
                        AND jsonb_typeof(equipment_diagnostic -> 'retention_error') = 'null')
                    OR (equipment_diagnostic ->> 'evidence_status' IN ('save_failed', 'completion_unconfirmed')
                        AND jsonb_typeof(equipment_diagnostic -> 'evidence_id') = 'string'
                        AND jsonb_typeof(equipment_diagnostic -> 'retention_error') = 'string')
                    OR (equipment_diagnostic ->> 'evidence_status' = 'prepare_failed'
                        AND jsonb_typeof(equipment_diagnostic -> 'evidence_id') = 'null'
                        AND jsonb_typeof(equipment_diagnostic -> 'retention_error') = 'string')
                    OR (equipment_diagnostic ->> 'evidence_status' = 'disabled'
                        AND jsonb_typeof(equipment_diagnostic -> 'evidence_id') = 'null'
                        AND jsonb_typeof(equipment_diagnostic -> 'retention_error') = 'null')
                )
                AND (jsonb_typeof(equipment_diagnostic -> 'retention_error') = 'null'
                    OR (
                        jsonb_typeof(equipment_diagnostic -> 'retention_error') = 'string'
                        AND equipment_diagnostic ->> 'retention_error' IN (
                            'invalid_context', 'invalid_outcome', 'invalid_message',
                            'payload_too_large', 'body_unavailable', 'encryption',
                            'serialization', 'io', 'capacity', 'not_found',
                            'authentication', 'unsupported_platform', 'retention_timeout',
                            'retention_cancelled', 'recorder_unavailable'
                        )
                    ))
                AND octet_length(COALESCE(equipment_diagnostic ->> 'retention_error', '')) <= 64
            )
        );
