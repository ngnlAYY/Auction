//! PostgreSQL 18 集成测试入口；由 `test-db` feature 控制。
//! 这里的 crate 级 `cfg` 已覆盖全部子模块，子模块不再重复声明。
#![cfg(feature = "test-db")]

mod perf;
mod public_lots;
mod support;

mod admin_auctions;
mod admin_runtime_commands;
mod asset_reconciliation;
mod auction_write_queue;
mod bid_cross_lot;
mod bid_log_proxies;
mod delivery;
mod disposition_reads;
mod forum_reply_targets;
mod key_rotation;
mod lot_watch;
mod mail_intake_resale;
mod mail_templates;
mod mailbox_sweep;
mod maintenance_evidence;
mod migration_compatibility;
mod operations_alert;
mod realtime_relay;
mod realtime_sequence;
mod recovery_confirm;
mod recovery_returns;
mod retention_dry_run;
mod retention_purge;
mod runtime_privileges;
mod settlement;
mod settlement_returns;
mod shutdown_window;
mod site_documents;
mod snapshot_timezone;
mod storage;
