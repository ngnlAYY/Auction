//! 公开端浏览器界面（Dioxus CSR）。
//!
//! 路由集合保留旧版 Auction-BF 的 `manifests/public.routes.json` 路径，并显式登记经评审的 Public
//! 扩展页面，保持 URL 契约可审计。
//! 界面文案全部经由 `dioxus-i18n` 取自 `i18n/*.ftl`，默认英文；受支持语言与持久化行为
//! 由 `auction-web-shared` 统一提供。
//!
//! 本 crate 不得导入存储、配置或凭据访问路径。HTTP/流 DTO 由 `auction-contracts` 拥有，
//! 本 crate 只保留浏览器展示模型、查询状态和交互编排。

#[path = "../identity/account.rs"]
mod account;
#[path = "../content/blacklist.rs"]
mod blacklist;
#[path = "../auction/commands.rs"]
mod commands;
#[path = "../content/compensation/mod.rs"]
mod compensation;
#[path = "../content/forum.rs"]
mod forum;
#[path = "../auction/proxy.rs"]
mod proxy;
#[path = "../auction/reads/mod.rs"]
mod reads;
#[path = "../auction/realtime.rs"]
mod realtime;
#[path = "../content/rules.rs"]
mod rules;
#[path = "../identity/session.rs"]
mod session;

mod board;
mod catalog;
mod pages;
mod query;
mod router;

pub(crate) use board::*;
pub(crate) use catalog::*;
pub(crate) use pages::*;
pub(crate) use query::*;
pub(crate) use router::Route;
pub use router::run;
pub(crate) use session::{initial_login_return_location, login_href_for_current_location};

use chrono::{DateTime, Utc};
use dioxus::prelude::*;
use dioxus_i18n::prelude::*;
use gloo_timers::future::TimeoutFuture;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
};
use unic_langid::langid;
use url::form_urlencoded;
#[cfg(target_arch = "wasm32")]
use uuid::Uuid;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, JsValue, closure::Closure};

use crate::api::{
    AuctionListPage, CatalogRow, CurrentAuction, HistoricalAuction, LotDetailFetchError,
    LotDetailItem, LotEnvelopeRows, ProxyCommandAuthorization, PublicSession, RealtimeCache,
    RealtimeEvent, WorldStatus, Worlds, estimated_now, fetch_auction_list, fetch_auction_page,
    fetch_current_auction, fetch_current_lots, fetch_historical_auction, fetch_lot_bid_log,
    fetch_lot_detail, fetch_my_proxy, fetch_public_readiness, fetch_public_session, fetch_worlds,
    lot_code_for, subscribe_auction_realtime, subscribe_realtime,
};
use auction_web_shared::{
    AsyncState, AsyncStateKind, BaseStylesheet, LanguagePicker, Pager, Time, TruncatedText,
    format_credit_amount as format_amount, format_localized_time,
    i18n::{DEFAULT_PUBLIC_LOCALE, initial_language},
    legacy_t as t, use_app_boot_ready, use_dialog_focus_trap, use_locale_effects, use_route_focus,
    use_second_tick, use_second_ticker,
};

/// Public 外壳唯一拥有的会话快照；页面只消费该资源，避免同一渲染树重复请求 session。
pub(crate) type PublicSessionResource = Resource<Result<Option<PublicSession>, String>>;

/// Public 外壳唯一拥有的 readiness 快照；默认首页用它协调主内容与首屏门禁。
pub(crate) type PublicReadinessResource = Resource<Result<bool, String>>;

/// Public 外壳按世界读取的当前拍卖快照；结果携带请求世界，切换期间不得消费旧世界缓存。
pub(crate) type CurrentAuctionResource = Resource<(String, Result<Option<CurrentAuction>, String>)>;
