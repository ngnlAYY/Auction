//! PostgreSQL 18 集成测试入口；由 `test-db` feature 控制。
#![cfg(feature = "test-db")]

mod perf;
mod support;

mod auction_write_queue;
mod bid_cross_lot;
mod delivery_payload_columns;
mod lot_watch;
mod migration_compatibility;
mod operations_alert;
mod realtime_sequence;
mod retention_dry_run;
mod retention_purge;
mod snapshot_timezone;
mod storage;
