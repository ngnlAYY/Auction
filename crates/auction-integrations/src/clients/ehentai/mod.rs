//! E-Hentai 论坛固定 origin 网关；所有响应有硬字节上限，POST
//! 发出后只接受可证明的远端身份，否则返回 `write_unknown`，上层必须先对账。

use std::{
    collections::HashSet,
    fmt,
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};

use auction_application::{
    AdminCredentialRuntimeOwnership, AdminCredentialRuntimeWriter, AdminForumPreviewResult,
    ApplicationError, ApplicationFuture, ForumCredentialAvailability, ForumPostWriter,
    ForumPublishedPost, ForumPublishedThread, ForumPublisher, ForumReadPost, ForumRemoteError,
    ForumRenderedPost, ForumRenderedThread, ForumTopicCursor, ForumTopicMetadata, ForumTopicReader,
    ForumTopicScan, ForumTopicScanMode, MAX_FORUM_RECONCILIATION_TITLES,
};
use auction_domain::SettlementWorld;
use chrono::Utc;
use reqwest::{
    Client, Response, StatusCode,
    header::{self, HeaderValue},
    redirect::Policy,
};
use scraper::{Html, Selector};
use url::{Url, form_urlencoded};
use uuid::Uuid;

const DEFAULT_MAXIMUM_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const HARD_MAXIMUM_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAXIMUM_FORUM_BODY_BYTES: usize = 65_531;
/// 本地主题标题预算，覆盖完整前缀与最大场次号；远端写入仍遵守既有证明门禁。
pub(crate) const MAXIMUM_FORUM_TITLE_RUNES: usize = 80;
const MAXIMUM_FORM_FIELDS: usize = 128;
const MAXIMUM_FORM_FIELD_BYTES: usize = 4 * 1024;
const MAXIMUM_COOKIE_BYTES: usize = 16 * 1024;
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_MINIMUM_REQUEST_INTERVAL: Duration = Duration::from_millis(750);
const DEFAULT_MAXIMUM_TOPIC_PAGES: usize = 10;
const HARD_MAXIMUM_TOPIC_PAGES: usize = 20;
const HARD_MAXIMUM_TOPIC_BYTES: usize = 32 * 1024 * 1024;
const MAXIMUM_CONCURRENT_TOPIC_READS: usize = 4;
/// 分页容量与应用层楼层换算共用同一个来源，避免两处常量漂移。
const FORUM_POSTS_PER_PAGE: u64 = auction_application::FORUM_POSTS_PER_PAGE as u64;
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0";

pub const PRODUCTION_ENDPOINT: &str = "https://forums.e-hentai.org";
pub const TEST_BOARD_ID: i32 = 4;
pub const MAIN_BOARD_ID: i32 = 77;
pub const ISEKAI_BOARD_ID: i32 = 90;

mod client;
mod credentials;
mod error;
mod forms;
mod gateway;

pub use client::{EhentaiClient, EhentaiClientConfig};
pub use credentials::{EhentaiCookie, EhentaiCredentialSet};
pub use error::{EhentaiError, EhentaiErrorCategory, EhentaiOperation};
pub use gateway::EhentaiGateway;
