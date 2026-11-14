//! 公开端浏览器界面（Dioxus CSR）。
//! 路由集合保留旧版 Auction-BF 的 `manifests/public.routes.json` 路径，并显式登记经评审的 Public
//! 扩展页面，保持 URL 契约可审计。
//! 界面文案全部经由 `dioxus-i18n` 取自 `i18n/*.ftl`，默认英文；受支持语言与持久化行为
//! 由 `auction-web-shared` 统一提供。
//! 本 crate 不得导入存储、配置或凭据访问路径。HTTP/流 DTO 由 `auction-contracts` 拥有，
//! 本 crate 只保留浏览器展示模型、查询状态和交互编排。

mod account;
mod blacklist;
mod commands;
mod compensation;
mod forum;
mod loading;
mod proxy;
mod reads;
mod realtime;
mod rules;
mod session;
mod session_preference;
mod watch_preference;
mod watches;

pub(crate) use loading::{PublicContentLoading, use_content_loading};
use watches::{LotWatchButton, LotWatchScope, use_lot_watched};

mod board;
mod catalog;
mod pages;
mod pagination;
mod presentation;
mod query;
mod router;
mod shell;

pub(crate) use board::*;
pub(crate) use catalog::*;
pub(crate) use pages::*;
pub(crate) use pagination::*;
pub(crate) use query::*;
pub(crate) use router::Route;
pub use router::run;
pub(crate) use session::{initial_login_return_location, login_href_for_current_location};

use chrono::{DateTime, Utc};
use dioxus::prelude::*;
use dioxus_i18n::prelude::*;
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
use wasm_bindgen::{JsCast, JsValue};

use crate::api::{
    AuctionListPage, CatalogRow, CurrentAuction, HistoricalAuction, LotEnvelopeRows,
    ProxyCommandAuthorization, PublicSession, RealtimeCache, RealtimeEvent, SessionRefreshReason,
    WorldStatus, Worlds, estimated_now, fetch_auction_list, fetch_current_auction,
    fetch_current_lots, fetch_historical_auction, fetch_my_proxy, fetch_public_readiness,
    fetch_public_session, fetch_worlds, subscribe_auction_realtime, subscribe_realtime,
};
use auction_web_shared::{
    AsyncState, AsyncStateKind, BaseErrorKind, BaseStylesheet, ConsoleStylesheet, LanguagePicker,
    QueryState, QueryWriteMode, StaleBuildBanner, ThemePicker, Time, TruncatedText,
    base_api_error_message, format_credit_amount,
    i18n::{
        DEFAULT_PUBLIC_LOCALE, LocaleCatalog, LocaleSession, LocaleSource, build_bundle,
        current_language, format_localized_time_for_language,
    },
    load_sidebar_collapsed, save_sidebar_collapsed, t, use_app_boot_ready, use_dialog_focus_trap,
    use_locale_effects, use_query_state, use_route_focus, use_second_tick, use_second_ticker,
    use_theme_effects, write_query_pairs,
};

/// Public 外壳唯一拥有的会话快照；页面只消费该资源，避免同一渲染树重复请求 session。
pub(crate) type PublicSessionResource = Resource<Result<Option<PublicSession>, String>>;

/// 命令宿主的已确认身份；暂时会话读取失败不清理，不能用于授权新写入。
#[derive(Clone, Copy)]
pub(crate) struct PublicCommandOwner(pub Signal<Option<String>>);

/// Public 外壳唯一拥有的 readiness 快照；默认首页用它协调主内容与首屏门禁。
pub(crate) type PublicReadinessResource = Resource<Result<bool, String>>;

/// Public 外壳按世界读取的当前拍卖快照；结果携带请求世界，切换期间不得消费旧世界缓存。
pub(crate) type CurrentAuctionResource = Resource<(String, Result<Option<CurrentAuction>, String>)>;

/// 会话资源快照的统一展示投影，看板、lot 详情和外壳头部共用。
/// 认证、就绪、错误与查看者身份按同一规则派生，匿名/未就绪/错误一律失败关闭。
#[derive(Clone)]
pub(crate) struct PublicSessionView {
    pub(crate) pending: bool,
    pub(crate) ready: bool,
    pub(crate) error: bool,
    pub(crate) authenticated: bool,
    pub(crate) viewer_player_id: Option<String>,
}

impl PublicSessionView {
    /// 从会话资源当前快照派生展示投影；调用方仍需原始快照时可保留本地副本。
    pub(crate) fn from_snapshot(snapshot: Option<&Result<Option<PublicSession>, String>>) -> Self {
        Self {
            pending: snapshot.is_none(),
            ready: matches!(snapshot, Some(Ok(_))),
            error: matches!(snapshot, Some(Err(_))),
            authenticated: matches!(snapshot, Some(Ok(Some(identity))) if identity.authenticated),
            viewer_player_id: match snapshot {
                Some(Ok(Some(identity))) if identity.authenticated => {
                    Some(identity.username.clone())
                }
                _ => None,
            },
        }
    }
}
