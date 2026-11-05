//! PostgreSQL 18 集成测试入口；由 `test-db` feature 控制。
#![cfg(feature = "test-db")]

mod support;

#[path = "postgres/lot_watch.rs"]
mod lot_watch;
#[path = "postgres/operations_alert.rs"]
mod operations_alert;
#[path = "postgres/snapshot_timezone.rs"]
mod snapshot_timezone;
#[path = "postgres/storage.rs"]
mod storage;
