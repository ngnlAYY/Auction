//! PostgreSQL 18 集成测试入口；由 `test-db` feature 控制。
#![cfg(feature = "test-db")]

mod perf;
mod public_lots;
mod support;

mod admin_auctions;
mod admin_runtime_commands;
mod auction_write_queue;
mod bid_cross_lot;
mod delivery;
mod disposition_reads;
mod key_rotation;
mod lot_watch;
mod mail_intake_resale;
mod mail_templates;
mod maintenance_evidence;
mod migration_compatibility;
mod operations_alert;
mod realtime_sequence;
mod retention_dry_run;
mod retention_purge;
mod runtime_privileges;
mod settlement;
mod shutdown_window;
mod site_documents;
mod snapshot_timezone;
mod storage;
