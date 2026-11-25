//! 第 6 期夹具：把代管中的 main 件推进到 available，再按真实写序构造购买、派发、退件接管与收款事实。

use sqlx::PgPool;

use super::super::contracts::fixture::{
    CONTRACT, Graph, OBLIGATION_1, OBLIGATION_3, seed_main_custody,
};
use super::super::deposit::support::{
    ACCEPTED, ASSET, ATTEMPT, CREDENTIAL, DECISION, LISTING, POSITION, PROOF, SESSION, USER_2,
    succeeds,
};

pub const AVAILABLE_COMMAND: &str = "c6200000-0000-5000-8000-000000000001";
pub const PURCHASE_COMMAND: &str = "c6100000-0000-4000-8000-000000000001";
pub const HANDOFF_COMMAND: &str = "c6300000-0000-5000-8000-000000000001";
pub const HANDOFF_PROOF: &str = "c6400000-0000-4000-8000-000000000001";
pub const PURCHASED_AT: &str = "2026-01-01T04:00:00Z";
pub const SOLD_AT: &str = "2026-01-01T05:00:00Z";
/// 原付款（原件腿 COD）的 delivery 与 Sent MID。
pub const PAY_DELIVERY: &str = "c6500000-0000-4000-8000-000000000001";
pub const PAY_MID: &str = "9001";
pub const PAY_ROUND: &str = "c6600000-0000-4000-8000-000000000001";
pub const PAY_ATTEMPT: &str = "c6700000-0000-4000-8000-000000000001";
/// 买家退款腿的原 Send（现金退件的原支出）。
pub const REFUND_DELIVERY: &str = "c6510000-0000-4000-8000-000000000001";
pub const REFUND_MID: &str = "9003";
pub const REFUND_ROUND: &str = "c6610000-0000-4000-8000-000000000001";
pub const REFUND_ATTEMPT: &str = "c6710000-0000-4000-8000-000000000001";
pub const PRICE: i64 = 101;

/// 取得证明后的挂单上架：deposit_accepted -> available（版本 5）。
pub fn available_sql() -> String {
    let body = format!(
        "{{\"schema_version\":2,\"operation\":\"apply_mail_proof\",
          \"disposition\":\"take_proven\",\"command_id\":\"{AVAILABLE_COMMAND}\",
          \"decision_id\":\"{DECISION}\",\"proof_id\":\"{PROOF}\",
          \"listing_changes\":[{{\"listing_id\":\"{LISTING}\",\"listing_version\":5,
          \"state\":\"available\"}}]}}"
    );
    // apply_mail_proof 的目标是原 attempt，不是挂单：这里单独写目标列。
    format!(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_id, listing_id, result_status, result_code,
             result_body, created_at)
         VALUES ('{AVAILABLE_COMMAND}', 2, 'worker', 'fake-worker', 'apply_mail_proof',
                 '{ATTEMPT}', decode(repeat('b1', 32), 'hex'),
                 'original_attempt', '{ATTEMPT}', NULL, 200, 'ok',
                 '{body}', '{ACCEPTED}');
         INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{LISTING}', 5, '{AVAILABLE_COMMAND}', 'worker', 'available',
                 'deposit_accepted', 'available', NULL, {PRICE}, '{ACCEPTED}');
         UPDATE public.consignment_listings
         SET state = 'available', state_version = 5, updated_at = '{ACCEPTED}'
         WHERE id = '{LISTING}';"
    )
}

/// 购买回执命令（买家 v4 命令，目标是挂单）。
pub fn purchase_command() -> String {
    let body = format!(
        "{{\"schema_version\":2,\"operation\":\"purchase\",\"disposition\":\"purchased\",
          \"command_id\":\"{PURCHASE_COMMAND}\",\"listing_id\":\"{LISTING}\",
          \"listing_version\":6,\"state\":\"sale_pending\",\"contract_id\":\"{CONTRACT}\",
          \"contract_version\":1,\"equipment_id\":\"{ASSET}\",\"world\":\"main\",
          \"price\":{PRICE},\"order_state\":\"awaiting_payment\"}}"
    );
    format!(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_id, listing_id, result_status, result_code,
             result_body, created_at)
         VALUES ('{PURCHASE_COMMAND}', 2, 'public_user', '{USER_2}', 'purchase', '{LISTING}',
                 decode(repeat('b2', 32), 'hex'), 'listing', '{LISTING}', '{LISTING}', 201,
                 'ok', '{body}', '{PURCHASED_AT}');"
    )
}

/// 合同图：Sale(main)，P=101、F=5、S=96。
pub fn purchase_graph() -> String {
    sale_graph().sql(PURCHASE_COMMAND)
}

pub fn sale_graph() -> Graph {
    Graph { price: PRICE, fee: 5, seller_amount: 96, payout_amount: 96, ..Graph::sale_main() }
}

/// position in_custody -> committed（版本 3）加事件。
pub fn purchase_position() -> String {
    format!(
        "UPDATE public.consignment_custody_positions
         SET state = 'committed', version = 3, updated_at = '{PURCHASED_AT}'
         WHERE id = '{POSITION}';
         INSERT INTO public.consignment_custody_events
            (position_id, version, command_id, event_kind, from_state, to_state, proof_id,
             listing_id, listing_version, occurred_at)
         VALUES ('{POSITION}', 3, '{PURCHASE_COMMAND}', 'committed', 'in_custody', 'committed',
                 NULL, '{LISTING}', 6, '{PURCHASED_AT}');"
    )
}

/// 挂单 available -> sale_pending（版本 6）加 purchased 事件。
pub fn purchase_listing() -> String {
    format!(
        "INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{LISTING}', 6, '{PURCHASE_COMMAND}', 'public_user', 'purchased', 'available',
                 'sale_pending', NULL, {PRICE}, '{PURCHASED_AT}');
         UPDATE public.consignment_listings
         SET state = 'sale_pending', state_version = 6, updated_at = '{PURCHASED_AT}'
         WHERE id = '{LISTING}';"
    )
}

/// 完整购买：回执、合同图、position、挂单在同一事务（顺序即真实写序）。
pub fn purchase_sql() -> String {
    purchase_command() + &purchase_graph() + &purchase_position() + &purchase_listing()
}

/// 代管中 -> available -> 已购（sale_pending / committed）。
pub async fn seed_purchased(pool: &PgPool) {
    seed_main_custody(pool).await;
    succeeds(pool, &available_sql()).await;
    succeeds(pool, &purchase_sql()).await;
}

/// 某条义务的一次真实 Send：delivery、Send 尝试与跨来源唯一的 Sent MID 绑定。
pub fn send_sql(delivery: &str, obligation: &str, mid: &str, cod: i64) -> String {
    let command = format!("c71{}", &delivery[3..]);
    format!(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, fencing_version, attempt_count, created_at, updated_at)
         VALUES ('{delivery}', '{command}', decode(repeat('c1', 32), 'hex'), 'settlement',
                 '{obligation}', 'FakeSellerTwo', '{{}}'::jsonb, 'prepared', 0, 0,
                 '{PURCHASED_AT}', '{PURCHASED_AT}');
         INSERT INTO public.delivery_send_attempts
            (delivery_id, fencing_version, command_id, request_digest, world_id, sending_at,
             expectation)
         VALUES ('{delivery}', 1, '{command}', decode(repeat('c2', 32), 'hex'), 'main',
                 '{PURCHASED_AT}',
                 jsonb_build_object('world_id', 'main', 'recipient', 'FakeSellerTwo',
                     'subject', 'fake', 'body_digest',
                     (SELECT jsonb_agg(0) FROM generate_series(1, 32)), 'credits', 0,
                     'cod', {cod}, 'equipment_ids', '[]'::jsonb, 'credential', 'null'::jsonb));
         INSERT INTO public.delivery_sent_bindings
            (world_id, external_mail_id, delivery_id, fencing_version,
             expectation_fencing_version)
         VALUES ('main', '{mid}', '{delivery}', 1, 1);"
    )
}

/// 首轮派发与其冻结身份、该轮的 Send 尝试（leg 是资产腿或现金腿）。
pub fn round_sql(
    round: &str,
    delivery: &str,
    obligation: &str,
    attempt: &str,
    leg: &str,
) -> String {
    format!(
        "INSERT INTO public.consignment_dispatch_rounds
            (contract_id, obligation_id, round, round_id, delivery_id, rights_holder_identity,
             bot_uid, world_id, subject, template, body_digest, round_ref, d0, credits, cod,
             prior_round, authorized_by_intake, authorized_by_consumption)
         VALUES ('{CONTRACT}', '{obligation}', 1, '{round}', '{delivery}', 'FakeSellerTwo',
                 '12345', 'main', 'fake subject', 'consignment_cod',
                 decode(repeat('d1', 32), 'hex'), '{round}', '{PURCHASED_AT}', 0, {PRICE},
                 NULL, NULL, NULL);
         INSERT INTO public.consignment_dispatch_attempts
            (delivery_id, fence, attempt_id, contract_id, obligation_id, round, leg, started_at)
         VALUES ('{delivery}', 1, '{attempt}', '{CONTRACT}', '{obligation}', 1, '{leg}',
                 '{PURCHASED_AT}');"
    )
}

/// 原件腿（OBLIGATION_1）的首派：Send、轮次与资产腿尝试。
pub fn original_dispatch_sql() -> String {
    send_sql(PAY_DELIVERY, OBLIGATION_1, PAY_MID, PRICE)
        + &round_sql(PAY_ROUND, PAY_DELIVERY, OBLIGATION_1, PAY_ATTEMPT, "original")
}

pub fn refund_dispatch_sql() -> String {
    send_sql(REFUND_DELIVERY, OBLIGATION_3, REFUND_MID, 0)
        + &round_sql(REFUND_ROUND, REFUND_DELIVERY, OBLIGATION_3, REFUND_ATTEMPT, "cash_refund")
}

/// 同一 K 的退件接管：n 区分不同来信（MID 556 + n），legs 取现金退款 / 付款腿。
pub struct Intake {
    pub n: u32,
    pub leg: &'static str,
}

impl Intake {
    pub fn id(&self, prefix: &str) -> String {
        format!("{prefix}{:02}000-0000-4000-8000-000000000001", self.n)
    }

    pub fn mid(&self) -> String {
        (556 + self.n).to_string()
    }

    pub fn intake(&self) -> String {
        self.id("c68")
    }

    pub fn decision(&self) -> String {
        self.id("c69")
    }

    pub fn work(&self) -> String {
        self.id("c6a")
    }

    pub fn take_delivery(&self) -> String {
        self.id("c6b")
    }

    pub fn attempt(&self) -> String {
        self.id("c6c")
    }

    pub fn proof(&self) -> String {
        self.id("c6d")
    }

    pub fn observation(&self) -> String {
        self.id("c6f")
    }

    /// acquire_return 是系统命令，v5。
    pub fn acquire_command(&self) -> String {
        format!("c67{:02}000-0000-5000-8000-000000000001", self.n)
    }

    pub fn start_command(&self) -> String {
        format!("c66{:02}000-0000-5000-8000-000000000001", self.n)
    }

    /// 原信、观察、首次决定与终局处置、Take 工作与 delivery、intake：与 acquire_return 同一事务。
    /// 需要先有退款腿的首派（refund_dispatch_sql）。
    pub fn sql(&self) -> String {
        let (mid, obs, decision, work, take, intake, command) = (
            self.mid(),
            self.observation(),
            self.decision(),
            self.work(),
            self.take_delivery(),
            self.intake(),
            self.acquire_command(),
        );
        let leg = self.leg;
        let body = format!(
            "{{\"schema_version\":2,\"operation\":\"acquire_return\",
              \"disposition\":\"return_acquired\",\"command_id\":\"{command}\",
              \"return_intake_id\":\"{intake}\",\"original_round\":1,
              \"take_work_id\":\"{work}\",\"take_delivery_id\":\"{take}\"}}"
        );
        format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_bot_uid, target_world_id,
                 target_external_mid, listing_id, result_status, result_code, result_body,
                 created_at)
             VALUES ('{command}', 2, 'system', 'fake-system', 'acquire_return',
                     '12345|main|{mid}', decode(repeat('e2', 32), 'hex'), 'mail_key', '12345',
                     'main', '{mid}', NULL, 200, 'ok', '{body}', '{PURCHASED_AT}');
             INSERT INTO public.mail_originals
                (bot_uid, world_id, external_mid, first_session_id, received_minute_raw,
                 received_timezone, first_seen_at, season_window_id)
             VALUES ('12345', 'main', '{mid}', '{SESSION}', '2026-01-01T04:00', 'UTC',
                     '{PURCHASED_AT}', NULL);
             INSERT INTO public.mail_envelope_observations
                (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
                 mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
                 evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
                 returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
                 season_window_id)
             VALUES ('{obs}', '12345', 'main', '{mid}', 'obs-{mid}', '{SESSION}',
                     'fake-mailbox', NULL, NULL, '{PURCHASED_AT}', 'parser-v1', NULL,
                     decode(repeat('66', 32), 'hex'), decode(repeat('67', 32), 'hex'),
                     decode(repeat('68', 32), 'hex'), 'FakeSellerOne', 'FakeBot',
                     true, true, 0, 0, 1,
                     '[{{\"ordinal\":1,\"kind\":\"equipment\",\"name\":\"Fake Gear\",
                        \"raw_eid\":\"42\",\"canonical_eid\":\"42\",
                        \"protected_ref\":\"83000000-0000-0000-0000-000000000001\"}}]'::jsonb,
                     '{{\"subject_is_consign\":true,\"sender_identity\":\"verified\",
                       \"list_detail_consistent\":true,\"duplicate_raw_eid\":false,
                       \"canonical_alias_ambiguous\":false,\"unknown_attachment\":false,
                       \"system_rts\":false,\"legacy_auction_owner\":null,
                       \"received_minute_raw\":\"2026-01-01T04:00\",
                       \"items\":[{{\"ordinal\":1,\"world\":\"main\",\"tradeable\":true,
                                   \"history_ok\":true}}]}}'::jsonb,
                     NULL);
             INSERT INTO public.mail_initial_decisions
                (bot_uid, world_id, external_mid, id, observation_id, command_id, kind,
                 quarantine_id, decided_at)
             VALUES ('12345', 'main', '{mid}', '{initial}', '{obs}', '{command}',
                     'acquire_return', NULL, '{PURCHASED_AT}');
             INSERT INTO public.mail_final_dispositions
                (bot_uid, world_id, external_mid, id, kind, source_observation_id, command_id,
                 decided_at)
             VALUES ('12345', 'main', '{mid}', '{decision}', 'acquire_return', '{obs}',
                     '{command}', '{PURCHASED_AT}');
             INSERT INTO public.deliveries
                (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
                 status, fencing_version, attempt_count, created_at, updated_at)
             VALUES ('{take}', '{take_command}', decode(repeat('69', 32), 'hex'),
                     'consignment_disposition', 'consignment:{decision}:{work}', 'FakeBot',
                     '{{\"schema_version\":2,\"operation\":\"take\",\"world\":\"main\",
                        \"external_mail_id\":\"{mid}\",
                        \"mail_key\":{{\"bot_uid\":\"12345\",\"world_id\":\"main\",
                        \"external_mid\":\"{mid}\"}},
                        \"decision_id\":\"{decision}\",\"work_id\":\"{work}\"}}'::jsonb,
                     'prepared', 0, 0, '{PURCHASED_AT}', '{PURCHASED_AT}');
             INSERT INTO public.mail_disposition_work
                (id, bot_uid, world_id, external_mid, decision_id, notice_id, action,
                 delivery_id, expected_observation_id, manifest_digest, created_at)
             VALUES ('{work}', '12345', 'main', '{mid}', '{decision}', NULL, 'take',
                     '{take}', '{obs}', decode(repeat('68', 32), 'hex'), '{PURCHASED_AT}');
             INSERT INTO public.consignment_return_intakes
                (id, bot_uid, world_id, external_mid, decision_id, decision_kind, contract_id,
                 obligation_id, original_round, original_delivery_id, original_fence, sent_mid,
                 leg, rts_observation_id, rts_returned_at, manifest_digest, session_id,
                 take_work_id, take_delivery_id)
             VALUES ('{intake}', '12345', 'main', '{mid}', '{decision}', 'acquire_return',
                     '{CONTRACT}', '{OBLIGATION_3}', 1, '{REFUND_DELIVERY}', 1, '{REFUND_MID}',
                     '{leg}', '{obs}', '{PURCHASED_AT}', decode(repeat('e1', 32), 'hex'),
                     '{SESSION}', '{work}', '{take}');",
            initial = self.id("c6e"),
            take_command = self.id("c70")
        )
    }

    /// 起写：start_return_take 命令、Take 尝试与起写事实（同一 intake 的唯一 Take 工作）。
    pub fn start_sql(&self) -> String {
        let (intake, work, attempt, take, command, decision) = (
            self.intake(),
            self.work(),
            self.attempt(),
            self.take_delivery(),
            self.start_command(),
            self.decision(),
        );
        let body = format!(
            "{{\"schema_version\":2,\"operation\":\"start_return_take\",
              \"disposition\":\"write_started\",\"command_id\":\"{command}\",
              \"return_intake_id\":\"{intake}\",\"work_id\":\"{work}\",
              \"attempt_id\":\"{attempt}\",\"fence\":1}}"
        );
        format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_id, listing_id, result_status, result_code,
                 result_body, created_at)
             VALUES ('{command}', 2, 'worker', 'fake-worker', 'start_return_take', '{intake}',
                     decode(repeat('e3', 32), 'hex'), 'return_intake', '{intake}', NULL, 200,
                     'ok', '{body}', '{PURCHASED_AT}');
             INSERT INTO public.mail_disposition_attempts
                (id, work_id, delivery_id, fence, credential_id, credential_version,
                 runtime_generation, session_id, expected_digest, prepared_at)
             VALUES ('{attempt}', '{work}', '{take}', 1, '{CREDENTIAL}', 1, 1, '{SESSION}',
                     decode(repeat('a6', 32), 'hex'), '{PURCHASED_AT}');
             INSERT INTO public.mail_disposition_write_starts
                (work_id, decision_id, delivery_id, attempt_id, fence, started_at, command_id)
             VALUES ('{work}', '{decision}', '{take}', '{attempt}', 1, '{PURCHASED_AT}',
                     '{command}');"
        )
    }
}
