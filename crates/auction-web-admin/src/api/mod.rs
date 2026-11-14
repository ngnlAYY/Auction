//! Admin 浏览器端的同源 HTTP 数据访问。
//! 浏览器只接收后端安全投影；改变状态的人工动作携带按输入复用的 `command_id` 与版本
//! fence，最终校验、幂等与审计归服务端。wire DTO 统一由 `auction-contracts` 拥有。
//!
//! 本文件只是门面：按 API 领域声明的客户端模块、共享传输/失败/校验助手与对外重导出都在
//! 目录内的单一职责文件中实现，页面继续经 `crate::api::…` 消费同一组符号。

mod alerts;
mod auctions;
mod audit;
mod credentials;
mod dashboard;
mod delivery;
mod failure;
mod forum;
mod intake;
mod lifecycle;
mod lots;
mod mail;
mod mail_diagnostics;
mod pagination;
mod proxy_events;
mod realtime;
mod recovery;
mod renames;
mod rules;
mod runtime_config;
mod session;
mod settlement;
mod settlement_catalog;
mod setup;
mod status;
mod transport;
mod users;
mod validation;
mod worlds;

pub(crate) use alerts::{
    AdminAlert, AdminAlertPage, AdminPendingCounts, acknowledge_admin_alert, fetch_admin_alerts,
    fetch_admin_pending_counts, fetch_admin_webhooks, silence_admin_alert,
};
pub(crate) use auctions::{
    AdminAuction, AdminAuctionCapabilities, AdminAuctionPage, create_admin_auction,
    fetch_admin_auctions, update_admin_auction_schedule,
};
pub(crate) use audit::{AdminAuditFilter, AdminAuditPage, AdminAuditRecord, fetch_admin_audit};
pub(crate) use credentials::{
    AdminCredential, AdminCredentialCheck, AdminCredentialPage, check_admin_credential,
    create_admin_credential, delete_admin_credential, fetch_admin_credentials,
    mutate_admin_credential,
};
pub(crate) use dashboard::{AdminDashboardSnapshot, AdminExternalAccountsData, fetch_dashboard};
pub(crate) use delivery::{
    DeliveryAction, DeliverySearchFilters, SentProofCommand, acknowledge_delivery,
    confirm_delivery_sent_proof, execute_delivery, fetch_deliveries, fetch_delivery_detail,
    fetch_player_support, preview_delivery_sent_proof, valid_status,
};
pub(crate) use failure::{
    AdminApiFailure, invalid_request_error, invalid_response_error, response_error,
    response_failure,
};
pub(crate) use forum::{AdminForumStatus, fetch_admin_forum_status};
pub(crate) use intake::{AdminIntakeResult, add_admin_intake};
pub(crate) use lifecycle::{AdminAuctionLifecycleAction, execute_admin_auction_lifecycle};
pub(crate) use lots::{
    AdminLot, AdminLotPage, AdminLotUpdateAction, fetch_admin_lots, update_admin_lot,
};
pub(crate) use mail::{
    AdminMailIntake, AdminMailIntakeDelivery, AdminMailIntakeDetail, AdminMailIntakePage,
    AdminMailIntakeQuery, AdminMailPolling, fetch_mail_intake_detail, fetch_mail_intakes,
    fetch_mail_state, set_mail_paused, trigger_mail_poll,
};
pub(crate) use proxy_events::{AdminProxyEvent, AdminProxyEventPage, fetch_admin_proxy_events};
pub(crate) use realtime::{
    RealtimeEventPage, RealtimeFailure, RealtimeFailurePage, RealtimeLagPage,
    fetch_realtime_event_page, fetch_realtime_event_total, fetch_realtime_failure_page,
    fetch_realtime_lag_page, replay_realtime_failure,
};
pub(crate) use recovery::{
    AdminRecovery, AdminRecoveryAction, AdminRecoveryReport, confirm_admin_recovery,
    fetch_admin_recovery,
};
pub(crate) use renames::{
    AdminRename, AdminRenamePage, AdminRenameRevertCommand, fetch_admin_renames,
    prepare_admin_rename_revert, revert_admin_rename,
};
pub(crate) use rules::{
    AdminDocumentKind, AdminRules, AdminRulesCandidate, fetch_admin_rules, publish_admin_rules,
    validate_admin_rules,
};
pub(crate) use runtime_config::{
    fetch_admin_runtime_config, update_admin_runtime_config, validate_admin_runtime_config,
};
pub(crate) use session::{
    AdminSession, AdminSessionCheckTrigger, AdminSessionWritePermission,
    admin_session_writes_allowed, fetch_admin_session, login_admin, logout_admin,
};
pub(crate) use settlement::{
    AdminSettlementLot, AdminSettlementReport, AdminSettlementSummary, fetch_admin_settlement,
};
pub(crate) use settlement_catalog::{
    AdminSettlementCatalogEntry, AdminSettlementCatalogPage, fetch_admin_settlement_catalog,
};
pub(crate) use setup::{submit_first_run, verify_first_run};
pub(crate) use status::{AdminReadinessState, fetch_admin_readiness};
pub(crate) use transport::{fetch_csrf_token, send_get, send_request, send_request_with_timeout};
pub(crate) use users::{
    AdminUser, AdminUserPage, AdminUserStatusFilter, blacklist_admin_user, create_admin_user,
    fetch_admin_users, mutate_admin_user, unblacklist_admin_user,
};
pub(crate) use validation::{
    MAX_JSON_SAFE_INTEGER, OptionalField, RequiredOption, normalize_datetime, parse_admin_time,
    valid_admin_time, valid_safe_text, valid_success_code, valid_trimmed_text,
};
pub(crate) use worlds::{
    AdminBusinessWorld, fetch_admin_business_worlds, set_admin_business_world_enabled,
};

use auction_contracts::ApiResponse;
pub(crate) use auction_contracts::{
    AdminDeliveryAttemptResponse as DeliveryAttempt,
    AdminDeliveryCustodyResponse as DeliveryCustody, AdminDeliveryDetailResponse as DeliveryDetail,
    AdminDeliveryDispositionResponse as DeliveryDisposition,
    AdminDeliveryEvidenceResponse as DeliveryEvidence, AdminDeliveryListResponse as DeliveryPage,
    AdminDeliveryObligationResponse as DeliveryObligation,
    AdminDeliveryRecoveryResponse as DeliveryRecovery, AdminDeliveryResponse as Delivery,
    AdminDeliverySentEvidenceRequest as DeliverySentEvidence,
    AdminDeliverySentProofPreviewResponse as DeliverySentProofPreview,
    AdminDeliverySentProofResponse as DeliverySentProofResult,
    AdminPlayerSupportBidResponse as PlayerSupportBid,
    AdminPlayerSupportBundleResponse as PlayerSupportBundle,
    AdminPlayerSupportDealResponse as PlayerSupportDeal,
    AdminPlayerSupportNotificationResponse as PlayerSupportNotification,
    AdminPlayerSupportObligationResponse as PlayerSupportObligation,
};

pub(crate) use auction_web_shared::valid_uuid;

mod runtime_switches;
pub(crate) use runtime_switches::{
    ExternalAccessPauseAttempt, fetch_runtime_switches, pause_outcome_unknown,
    submit_external_access_pause,
};

mod notification_templates;
pub(crate) use notification_templates::{
    MailTemplateAttempt, MailTemplateBodyProblem, fetch_mail_templates, mail_template_body_problem,
    mail_template_outcome_unknown, submit_mail_template,
};
