//! Public 与 Admin 的 HTTP 表面适配器。
//!
//! 这里只做 HTTP 输入到应用命令、应用输出到稳定契约的映射；竞价、结算和状态机规则
//! 留在领域与应用层。Public 与 Admin 是不同 surface，各自拥有独立的路由与前端产物。

#[path = "../admin/auctions/mod.rs"]
mod admin_auctions;
#[path = "../admin/audit.rs"]
mod admin_audit;
#[path = "../admin/credentials/mod.rs"]
mod admin_credentials;
#[path = "../admin/dashboard/mod.rs"]
mod admin_dashboard;
#[path = "../admin/forum.rs"]
mod admin_forum;
#[path = "../admin/intake.rs"]
mod admin_intake;
#[path = "../admin/lot_update/mod.rs"]
mod admin_lot_update;
#[path = "../admin/lots.rs"]
mod admin_lots;
#[path = "../admin/mail/mod.rs"]
mod admin_mail;
#[path = "../admin/proxy_events.rs"]
mod admin_proxy_events;
#[path = "../admin/renames.rs"]
mod admin_renames;
#[path = "../admin/rules.rs"]
mod admin_rules;
#[path = "../admin/runtime_config.rs"]
mod admin_runtime_config;
#[path = "../admin/users/mod.rs"]
mod admin_users;
#[path = "../public/blacklist.rs"]
mod blacklist;
#[path = "../shared/boundary.rs"]
mod boundary;
#[path = "../public/compensation.rs"]
mod compensation;
#[path = "../admin/delivery/mod.rs"]
mod delivery_admin;
#[path = "../public/forum.rs"]
mod forum;
#[path = "../metrics/mod.rs"]
mod metrics;
#[path = "../public/realtime/mod.rs"]
mod realtime;
#[path = "../admin/realtime/mod.rs"]
mod realtime_admin;
#[path = "../shared/realtime_hub/mod.rs"]
mod realtime_hub;
#[path = "../shared/response_cache.rs"]
mod response_cache;
#[path = "../public/rules.rs"]
mod rules;
#[path = "../security/mod.rs"]
mod security;
#[path = "../shared/server.rs"]
mod server;
#[path = "../shared/state/mod.rs"]
mod state;
#[cfg(target_os = "linux")]
#[path = "../shared/unix_listener.rs"]
mod unix_listener;

mod admin;
mod health;
mod mapping;
mod public;
mod reads;
mod session;
mod surfaces;

pub(crate) use admin::*;
pub(crate) use mapping::*;
pub(crate) use public::*;
pub(crate) use reads::*;
pub(crate) use session::*;

use std::{collections::BTreeMap, net::SocketAddr, time::Instant};

use crate::assets as web_assets;

use auction_application::{
    AdminLotUpdateError, AdminSessionIdentity, ApplicationError, AuctionLifecycleAction,
    AuctionLifecycleActor, AuctionLifecycleActorType, AuctionLifecycleCommand, CommandLedgerError,
    CompetitionTransactionError, CreateProxyCommand, CurrentAuctionSnapshot,
    DEFAULT_PUBLIC_ACCOUNT_PAGE_SIZE, LowerProxyCommand, MAX_PUBLIC_ACCOUNT_OFFSET,
    MAX_PUBLIC_ACCOUNT_PAGE_SIZE, MAX_PUBLIC_BID_LOG_OFFSET, MAX_PUBLIC_BID_LOG_PAGE_SIZE,
    MAX_PUBLIC_HISTORY_NAME_TERMS, MAX_PUBLIC_LOT_OFFSET, MAX_PUBLIC_LOT_PAGE_SIZE,
    MAX_PUBLIC_PRICE_HISTORY_OFFSET, MAX_PUBLIC_PRICE_HISTORY_PAGE_SIZE, PublicAccountPageRequest,
    PublicAuctionCursor, PublicAuctionListRequest, PublicAuctionPage, PublicAuctionProjection,
    PublicAuctionSummary, PublicBidLogCursor, PublicBidLogEntry, PublicBidLogListRequest,
    PublicBidLogPage, PublicEquipment, PublicFinalRestriction, PublicLiveLotSummary,
    PublicLotDetail, PublicLotListRequest, PublicLotPage, PublicMyBid, PublicMyBidsPage,
    PublicMyBidsRequest, PublicOwnedProxy, PublicPreviewLotDetail, PublicPreviewLotSummary,
    PublicPriceHistoryCursor, PublicPriceHistoryEntry, PublicPriceHistoryListRequest,
    PublicPriceHistoryPage, PublicPriceHistoryStats, PublicProxyBid, PublicProxyTransition,
    PublicSessionIdentity, RecoveryCommandError, RecoveryConfirmationAction,
    RecoveryConfirmationCommand, RecoveryConfirmationResult, RecoveryReport, RequestHash,
    SessionError, SettlementReport, WorldSnapshot, allowed_recovery_commands,
};
use auction_contracts::{
    AdminAuctionLifecycleRequest, AdminAuctionLifecycleResponse, AdminRecoveryActionResponse,
    AdminRecoveryConfirmationRequest, AdminRecoveryConfirmationResponse,
    AdminRecoveryConfirmationResultResponse, AdminRecoveryReportResponse, AdminRecoveryResponse,
    AdminSessionResponse, AdminSettlementAdjustmentResponse, AdminSettlementAuctionResponse,
    AdminSettlementLotResponse, AdminSettlementReportResponse, AdminSettlementSummaryResponse,
    ApiErrorResponse, ApiErrorResponseWithData, ApiResponse, AuctionListResponse,
    AuctionSummaryResponse, CreateProxyAuthorizationRequest, CurrentAuctionItemResponse,
    CurrentAuctionResponse, HealthLiveData, HealthProbeResponse, HealthReadyData, HealthResponse,
    LowerProxyAuthorizationRequest, PlaceBidRequest, PlaceBidResponse, PlaceBidResultResponse,
    PublicAccountBlockedResponse, PublicBidLogEntryResponse, PublicBidLogListResponse,
    PublicEquipmentResponse, PublicEquipmentStatResponse, PublicFinalRestrictionResponse,
    PublicLiveLotResponse, PublicLoginRequest, PublicLoginResponse, PublicLotDetailItemResponse,
    PublicLotEnvelopeResponse, PublicLotListItemsResponse, PublicMyBidsResponse,
    PublicMyProxyResponse, PublicOwnedBidResponse, PublicOwnedEquipmentResponse,
    PublicOwnedProxyCapabilitiesResponse, PublicOwnedProxyQuotaResponse, PublicOwnedProxyResponse,
    PublicOwnedProxyTransitionResponse, PublicPreviewLotDetailResponse, PublicPreviewLotResponse,
    PublicPriceHistoryEntryResponse, PublicPriceHistoryListResponse,
    PublicPriceHistoryStatsResponse, PublicProxyCommandAuthorizationResponse,
    PublicProxyCommandBidResponse, PublicProxyCommandResponse, PublicSessionCurrentResponse,
    WorldStatusResponse, WorldsResponse,
};
use auction_domain::{
    BidAmount, CommandId, CompetitionPlan, DomainError, LotId, ProxyAuthorization,
    ProxyAuthorizationId, ProxyMode, ProxyStatus, SettlementId, UserId,
};
use auction_observability::{BusinessEvent, BusinessOutcome};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{ConnectInfo, DefaultBodyLimit, Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header},
    middleware,
    response::{IntoResponse, Response},
    routing::{get, patch, post, put},
};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use chrono::{DateTime, Duration, NaiveDate, SecondsFormat, TimeZone, Utc};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use url::form_urlencoded;
use uuid::Uuid;

pub use boundary::HttpBoundary;
pub use realtime_hub::{RealtimeHub, RealtimeHubHealth, RealtimeHubSink, RealtimeSubscription};
pub use security::{AdminHttpSecurity, HttpSecurityOptions, PublicHttpSecurity};
pub use server::serve_http;
pub use state::{AdminApiState, PublicApiState};
pub use surfaces::{admin_router, public_router};
#[cfg(target_os = "linux")]
pub use unix_listener::{UnixHttpListener, UnixListenerOptions};
