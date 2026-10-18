#[path = "admin/auction.rs"]
mod admin_auction;
#[path = "admin/auction_schedule.rs"]
mod admin_auction_schedule;
#[path = "admin/credentials/mod.rs"]
mod admin_credentials;
#[path = "admin/intake.rs"]
mod admin_intake;
#[path = "admin/lot.rs"]
mod admin_lot;
#[path = "admin/mail.rs"]
mod admin_mail;
#[path = "admin/rules.rs"]
mod admin_rules;
#[path = "admin/runtime_config.rs"]
mod admin_runtime_config;
#[path = "admin/users/mod.rs"]
mod admin_users;
#[path = "admin/worlds.rs"]
mod admin_worlds;
#[path = "auction/commands.rs"]
mod auction;
#[path = "auction/competition.rs"]
mod competition;
#[path = "settlement/funding/mod.rs"]
mod funding;
pub use funding::FundingReconciliationService;
#[path = "settlement/fulfillment/mod.rs"]
mod fulfillment;
#[path = "auction/lifecycle/mod.rs"]
mod lifecycle;
#[path = "mail/maintenance.rs"]
mod mail_maintenance;
#[path = "mail/processing.rs"]
mod mail_processing;
#[path = "public/rules_cache.rs"]
mod public_rules_cache;
#[path = "realtime/relay.rs"]
mod realtime_relay;
#[path = "recovery/commands.rs"]
mod recovery;
#[path = "recovery/health.rs"]
mod recovery_health;
mod sessions;
#[path = "settlement/commands/mod.rs"]
mod settlement;
#[path = "mail/user.rs"]
mod user_mail;

pub use admin_auction::AdminAuctionCreateService;
pub use admin_auction_schedule::AdminAuctionScheduleService;
pub use admin_credentials::{AdminCredentialCheckService, AdminCredentialService};
pub use admin_intake::AdminIntakeService;
pub use admin_lot::AdminLotUpdateService;
pub use admin_mail::AdminMailService;
pub use admin_rules::AdminRulesService;
pub use admin_runtime_config::AdminRuntimeConfigService;
pub use admin_users::AdminUserService;
pub use admin_worlds::AdminWorldService;
pub use auction::{AuctionService, CreateAuctionCommand};
pub use competition::{
    CREATE_PROXY_COMMAND_KIND, CompetitionService, CreateProxyCommand, LOWER_PROXY_COMMAND_KIND,
    LowerProxyCommand, PLACE_BID_COMMAND_KIND, PlaceBidCommand,
};
pub use fulfillment::SettlementFulfillmentService;
pub use lifecycle::AuctionLifecycleService;
pub use mail_maintenance::{MailboxMaintenanceReport, MailboxMaintenanceService};
pub use mail_processing::{MailAuctionIntakeService, MailProcessorConfig, MailProcessorService};
pub use public_rules_cache::{CachedPublicRulesReader, public_rules_cache_key};
pub use realtime_relay::{RealtimeRelay, RealtimeRelayBatch, RealtimeRelayConfig};
pub use recovery::RecoveryCommandService;
pub use recovery_health::RecoveryHealthService;
pub use sessions::{
    AdminSessionService, PublicSessionService, validate_admin_actor, validate_admin_secret,
};
pub use settlement::{
    SettleAuctionCommand, SettlementCommandPort, SettlementService, verify_legacy_settlement_replay,
};
pub use user_mail::MailUserService;

#[path = "public/watch.rs"]
mod lot_watch;
pub use lot_watch::{lot_watch_notification, set_lot_watch};
#[path = "public/preferences/mod.rs"]
mod watch_preference;
pub use watch_preference::set_lot_watch_preference;
#[path = "public/watch_notification.rs"]
mod watch_notification;
pub use watch_notification::{LotWatchPmNotification, lot_watch_pm_notification};
