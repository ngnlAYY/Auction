//! Auction-BF PostgreSQL v28 的显式接管边界；旧版 golang-migrate 版本号与 SQLx 迁移树无对应含义。
//! 本模块只接受精确、干净且已停外部连接的旧 v28，先验证关键事实可无损回填，再在同一事务内补齐
//! Rust v5 结构并写入受审计的 SQLx 基线。

use crate::database::audit_health::AuditWriteResult as _;

use super::PostgresStore;

mod adoption;
mod mapping;
mod model;
mod preflight;
mod source_checks;

pub use model::*;

pub const LEGACY_V28_MIGRATION_VERSION: i64 = 28;

/// 旧版 v28 接管预检的机器可读格式。
pub const POSTGRES_LEGACY_V28_PREFLIGHT_FORMAT: &str = "auction-postgres-legacy-v28-preflight.v1";

/// 旧 v28 已经等价覆盖的 Rust SQLx 迁移。
/// 1..=5 是压缩后的旧版最终 schema；v7 的 replay 唯一性在旧版 v25 已用另一索引名实现，bridge 会
/// 补建当前索引，不能再执行 v7 中针对 fresh schema 临时约束的 DROP。
const BASELINED_SQLX_VERSIONS: &[i64] = &[1, 2, 3, 4, 5, 7];

const EXPECTED_LEGACY_TABLES: &[&str] = &[
    "admin_sessions",
    "auction_commands",
    "auction_forum_bid_commands",
    "auction_forum_reply_cursors",
    "auction_forum_sync",
    "auction_intake_targets",
    "auction_preview_items",
    "auction_schema_identity",
    "auction_settlements",
    "auction_submissions",
    "auctions",
    "audit_records",
    "bid_proxy_authorization_transitions",
    "bid_proxy_authorizations",
    "bids",
    "compensation_ledger",
    "credential_admin_commands",
    "credential_keys",
    "deliveries",
    "delivery_admin_commands",
    "equipment",
    "equipment_secrets",
    "forum_bidder_identities",
    "inbound_mail",
    "lot_settlements",
    "lots",
    "mail_admin_commands",
    "mail_commands",
    "mail_poll_state",
    "operations_alert_batch_items",
    "operations_alert_batches",
    "operations_alert_signals",
    "public_sessions",
    "realtime_consumer_offsets",
    "realtime_events",
    "realtime_failures",
    "realtime_streams",
    "rules_admin_commands",
    "rules_documents",
    "runtime_config_admin_commands",
    "scheduled_rounds",
    "schema_migrations",
    "service_heartbeats",
    "settlement_obligation_fulfillments",
    "settlement_obligations",
    "settlement_pools",
    "settlement_recoveries",
    "settlement_recovery_actions",
    "settlement_recovery_commands",
    "settlement_recovery_health",
    "user_admin_commands",
    "user_blacklist",
    "user_blacklist_history",
    "user_game_identity_history",
    "user_login_key_envelopes",
    "user_mail_commands",
    "user_renames",
    "users",
    "worlds",
];
