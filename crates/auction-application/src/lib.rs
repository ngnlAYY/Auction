//! 用例、窄端口和事务边界。

mod errors;
mod operator_audit;
mod ports;
mod services;

pub mod admin;
pub mod auction;
pub mod core;
pub mod delivery;
pub mod external_accounts;
pub mod external_metrics;
pub mod forum;
pub mod lot_watch_pm;
pub mod lot_watch_reminders;
pub mod mail;
pub mod notification_metrics;
pub mod operations;
pub mod player_notifications;
pub mod public;
pub mod realtime;
pub mod reconciliation;
pub mod recovery;
pub mod runtime_switches;
pub mod scheduling;
pub mod sent_baseline;
pub mod sessions;
pub mod settlement;
pub mod shadow_run;
pub mod sli;
