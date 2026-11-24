//! 000134 改造既有表时新增的命名约束违反行。

use super::support::Case;

pub const CASES: &[Case] = &[
    Case {
        table: "consignment_commands",
        constraint: "c4_command_schema_version_ck",
        state: "23514",
        sql: "UPDATE public.consignment_commands
              SET schema_version=3
              WHERE command_id='41000000-0000-5000-8000-000000000002';",
    },
    Case {
        table: "consignment_commands",
        constraint: "c4_command_typed_target_ck",
        state: "23514",
        sql: "UPDATE public.consignment_commands
              SET target_bot_uid=NULL
              WHERE command_id='41000000-0000-5000-8000-000000000002';",
    },
    Case {
        table: "deliveries",
        constraint: "c4_delivery_consignment_payload_ck",
        state: "23514",
        sql: "UPDATE public.deliveries
              SET payload='{}'::jsonb
              WHERE id='8a000000-0000-0000-0000-000000000001';",
    },
    Case {
        table: "deliveries",
        constraint: "c4_delivery_consignment_notice_payload_ck",
        state: "23514",
        sql: "UPDATE public.deliveries
              SET origin_kind='consignment_notice'
              WHERE id='8a000000-0000-0000-0000-000000000001';",
    },
];

pub const EXCEPTIONS: &[(&str, &str)] = &[
    ("c4_listing_identity_uq", "元组含主键 id；PK 先拦截"),
    ("c4_event_command_listing_version_uq", "元组含 (listing_id,state_version) 主键；PK 先拦截"),
    ("c4_inbound_mail_identity_uq", "元组含主键 id；PK 先拦截"),
];
