//! 合同图夹具：代管中的 position（main 或 isekai）加可变形状的合同、子项、义务与履约 SQL。

use sqlx::PgPool;

use super::super::deposit::support::{
    ASSET, LISTING, POSITION, USER_1, USER_2, accepted_envelope, attempt_start_proof_and_custody,
    succeeds,
};

pub const CONTRACT: &str = "c5000000-0000-4000-8000-000000000001";
pub const WINDOW: &str = "c5500000-0000-4000-8000-000000000001";
pub const OBLIGATION_1: &str = "c5100000-0000-4000-8000-000000000001";
pub const OBLIGATION_2: &str = "c5100000-0000-4000-8000-000000000002";
pub const OBLIGATION_3: &str = "c5100000-0000-4000-8000-000000000003";
pub const OBLIGATION_4: &str = "c5100000-0000-4000-8000-000000000004";
const CREATED: &str = "2026-01-02T00:00:00Z";
const BUYER: &str = "FakeSellerTwo";
const SELLER: &str = "FakeSellerOne";

/// main 世界的代管中 position：已接受、已取得（处分代 1），季窗为空。
pub async fn seed_main_custody(pool: &PgPool) {
    succeeds(pool, &accepted_envelope()).await;
    succeeds(pool, &attempt_start_proof_and_custody()).await;
}

/// isekai 世界的代管中 position：季窗结束时刻由 `window_end` 的 SQL 表达式给出。
pub async fn seed_isekai_custody(pool: &PgPool, window_end: &str) {
    seed_isekai_awaiting(pool, window_end).await;
    succeeds(pool, &attempt_start_proof_and_custody()).await;
}

/// isekai 世界、绑定季窗、尚未取得（awaiting_take）的 position，取得工作没有起写。
pub async fn seed_isekai_awaiting(pool: &PgPool, window_end: &str) {
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.consignment_season_windows
                (id, season_id, world_id, timezone, start_utc, end_utc, admission_cutoff_utc,
                 calendar_version, preclose_days)
             VALUES ('{WINDOW}', 'fake-season', 'isekai', 'UTC',
                     transaction_timestamp() - interval '20 days', {window_end},
                     transaction_timestamp() - interval '10 days',
                     decode(repeat('79', 32), 'hex'), 3);"
        ),
    )
    .await;
    let accepted =
        accepted_envelope().replace("'main'", "'isekai'").replace("|main|", "|isekai|").replace(
            " 'isekai', '42',\n                 NULL, 'awaiting_take'",
            &format!(" 'isekai', '42',\n                 '{WINDOW}', 'awaiting_take'"),
        );
    assert!(accepted.contains(WINDOW), "isekai 夹具必须绑定季窗");
    succeeds(pool, &accepted).await;
}

/// 合同图形状与可注入的缺陷；缺省是合法图。
pub struct Graph {
    pub kind: &'static str,
    pub world: &'static str,
    pub price: i64,
    pub fee: i64,
    pub seller_amount: i64,
    pub payout_amount: i64,
    pub with_refund: bool,
    pub refund_fulfillment: bool,
    pub payout_identity: &'static str,
    /// 付款义务（COD / 付款载体）金额；缺省等于成交价。
    pub cod_amount: Option<i64>,
    pub refund_state: &'static str,
}

impl Graph {
    pub fn sale_main() -> Self {
        Self {
            kind: "sale",
            world: "main",
            price: 1000,
            fee: 50,
            seller_amount: 950,
            payout_amount: 950,
            with_refund: true,
            refund_fulfillment: true,
            payout_identity: SELLER,
            cod_amount: None,
            refund_state: "contingent",
        }
    }

    pub fn sale_isekai() -> Self {
        Self { world: "isekai", ..Self::sale_main() }
    }

    pub fn withdrawal() -> Self {
        Self { kind: "withdrawal", ..Self::sale_main() }
    }

    /// 完整的合同图：合同头、子项、义务与履约。
    pub fn sql(&self, command: &str) -> String {
        self.head_sql(command) + &self.obligations()
    }

    /// 合同头与子项；`command` 是合同的来源命令。
    pub fn head_sql(&self, command: &str) -> String {
        let season = if self.world == "isekai" { format!("'{WINDOW}'") } else { "NULL".to_owned() };
        let mut sql = format!(
            "INSERT INTO public.consignment_contracts
                (contract_id, kind, origin_command_id, listing_id, position_id, generation,
                 equipment_id, world_id, canonical_eid, season_window_id, seller_user_id,
                 frozen_seller_identity, created_at)
             VALUES ('{CONTRACT}', '{kind}', '{command}', '{LISTING}', '{POSITION}', 1,
                     '{ASSET}', '{world}', '42', {season}, '{USER_1}', '{SELLER}', '{CREATED}');",
            kind = self.kind,
            world = self.world
        );
        if self.kind == "sale" {
            sql.push_str(&format!(
                "INSERT INTO public.settlement_funding_accounts (uid, status)
                 VALUES ('7001', 'ready') ON CONFLICT DO NOTHING;
                 INSERT INTO public.consignment_sales
                    (contract_id, buyer_user_id, frozen_buyer_identity, listing_version, price,
                     fee, seller_amount, pricing_version, fee_bps, payment_deadline_days,
                     payment_deadline_at, reminder_limit, cod_return_limit,
                     isekai_refusal_limit, isekai_refusal_window_days, funding_uid,
                     funding_mode, funding_boundary_at, funding_boundary_source_id,
                     funding_account_version)
                 VALUES ('{CONTRACT}', '{USER_2}', '{BUYER}', 4, {price}, {fee}, {seller_amount},
                         1, 500, 7, '2026-01-09T00:00:00Z', 1, 3, 5, 90, '7001',
                         'strict_cash', '{CREATED}', NULL, 0);",
                price = self.price,
                fee = self.fee,
                seller_amount = self.seller_amount
            ));
        } else {
            sql.push_str(&format!(
                "INSERT INTO public.consignment_withdrawals (contract_id) VALUES ('{CONTRACT}');"
            ));
        }
        sql
    }

    fn obligations(&self) -> String {
        let row = |id: &str,
                   kind: &str,
                   state: &str,
                   buyer: bool,
                   amount: i64,
                   credit: i32,
                   prerequisite: Option<&str>| {
            let recipient = if buyer {
                format!("'{USER_2}', NULL")
            } else {
                format!("NULL, '{}'", self.payout_identity)
            };
            let prerequisite =
                prerequisite.map_or_else(|| "NULL".to_owned(), |value| format!("'{value}'"));
            format!(
                "('{id}', '{world}', 1, '{kind}', '{state}', {recipient}, {amount}, {credit},
                  {prerequisite}, '{CONTRACT}', '{contract_kind}', '{ASSET}',
                  '{CREATED}', '{CREATED}')",
                world = self.world,
                contract_kind = self.kind
            )
        };
        let (rows, fulfillments): (Vec<String>, Vec<(&str, &str, bool)>) =
            match (self.kind, self.world) {
                ("withdrawal", _) => (
                    vec![row(OBLIGATION_1, "withdrawn_return", "ready", false, 0, 0, None)],
                    vec![(OBLIGATION_1, self.world, false)],
                ),
                (_, "main") => {
                    let mut rows = vec![
                        row(
                            OBLIGATION_1,
                            "winner_equipment_cod",
                            "ready",
                            true,
                            self.cod_amount.unwrap_or(self.price),
                            0,
                            None,
                        ),
                        row(
                            OBLIGATION_2,
                            "seller_payout",
                            "blocked",
                            false,
                            self.payout_amount,
                            0,
                            Some(OBLIGATION_1),
                        ),
                    ];
                    let mut fulfillments =
                        vec![(OBLIGATION_1, "main", false), (OBLIGATION_2, "main", false)];
                    if self.with_refund {
                        rows.push(row(
                            OBLIGATION_3,
                            "buyer_refund",
                            self.refund_state,
                            true,
                            self.price,
                            0,
                            Some(OBLIGATION_1),
                        ));
                        if self.refund_fulfillment {
                            fulfillments.push((OBLIGATION_3, "main", true));
                        }
                    }
                    (rows, fulfillments)
                }
                _ => {
                    let mut rows = vec![
                        row(
                            OBLIGATION_1,
                            "winner_payment_carrier_cod",
                            "ready",
                            true,
                            self.cod_amount.unwrap_or(self.price),
                            1,
                            None,
                        ),
                        row(
                            OBLIGATION_2,
                            "winner_equipment_handoff",
                            "blocked",
                            true,
                            0,
                            0,
                            Some(OBLIGATION_1),
                        ),
                        row(
                            OBLIGATION_3,
                            "seller_payout",
                            "blocked",
                            false,
                            self.payout_amount,
                            0,
                            Some(OBLIGATION_2),
                        ),
                    ];
                    let mut fulfillments = vec![
                        (OBLIGATION_1, "main", false),
                        (OBLIGATION_2, "isekai", false),
                        (OBLIGATION_3, "main", false),
                    ];
                    if self.with_refund {
                        rows.push(row(
                            OBLIGATION_4,
                            "buyer_refund",
                            self.refund_state,
                            true,
                            self.price,
                            0,
                            Some(OBLIGATION_1),
                        ));
                        if self.refund_fulfillment {
                            fulfillments.push((OBLIGATION_4, "main", true));
                        }
                    }
                    (rows, fulfillments)
                }
            };
        let mut sql = format!(
            "INSERT INTO public.settlement_obligations
                (id, world_id, policy_version, obligation_kind, business_state,
                 recipient_user_id, recipient_game_identity, amount, carrier_credit,
                 prerequisite_obligation_id, contract_id, contract_kind, custody_equipment_id,
                 created_at, updated_at)
             VALUES {};",
            rows.join(",")
        );
        for (obligation, dispatch_world, dormant) in fulfillments {
            sql.push_str(&format!(
                "INSERT INTO public.settlement_obligation_fulfillments
                    (obligation_id, execution_state, dispatch_world, delivery_operation, dormant,
                     created_at, updated_at)
                 VALUES ('{obligation}', 'pending', '{dispatch_world}', 'send', {dormant},
                         '{CREATED}', '{CREATED}');"
            ));
        }
        sql
    }
}
