//! PostgreSQL 18 集成测试入口；由 `test-db` feature 控制。
#![cfg(feature = "test-db")]

mod support;

#[path = "postgres/auction_write_queue.rs"]
mod auction_write_queue;
#[path = "postgres/bid_cross_lot.rs"]
mod bid_cross_lot;
#[path = "postgres/delivery_payload_columns.rs"]
mod delivery_payload_columns;
#[path = "postgres/lot_watch.rs"]
mod lot_watch;
#[path = "postgres/migration_compatibility.rs"]
mod migration_compatibility;
#[path = "postgres/operations_alert.rs"]
mod operations_alert;
#[path = "postgres/realtime_sequence.rs"]
mod realtime_sequence;
#[path = "postgres/retention_dry_run.rs"]
mod retention_dry_run;
#[path = "postgres/retention_purge.rs"]
mod retention_purge;
#[path = "postgres/snapshot_timezone.rs"]
mod snapshot_timezone;
#[path = "postgres/storage.rs"]
mod storage;
