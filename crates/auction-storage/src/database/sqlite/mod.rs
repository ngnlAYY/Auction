//! SQLite Lite 档的事务适配器。

#[path = "competition/transaction.rs"]
mod competition;

pub use competition::SqliteCompetitionTransaction;
