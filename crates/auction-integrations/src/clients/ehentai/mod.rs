//! E-Hentai 论坛固定 origin 网关。
//!
//! 该适配器只实现旧论坛协议所需的两类操作：按标题查找或创建主题，以及读取已发布主题。
//! 所有响应都有硬字节上限；创建主题在 POST 发出后只接受可证明的远端主题身份，否则返回
//! `write_unknown`，上层必须先对账，不能盲目重发。

use std::{
    collections::HashSet,
    fmt,
    sync::{Arc, Mutex as StdMutex},
    time::{Duration, Instant},
};

use auction_application::{
    AdminCredentialRuntimeWriter, AdminForumPreviewResult, ApplicationError, ApplicationFuture,
    ForumPublishedThread, ForumPublisher, ForumRemoteError, ForumRenderedThread, ForumTopicReader,
};
use auction_domain::SettlementWorld;
use chrono::Utc;
use reqwest::{
    Client, Response, StatusCode,
    header::{self, HeaderValue},
    redirect::Policy,
};
use scraper::{Html, Selector};
use tokio::sync::Mutex;
use url::{Url, form_urlencoded};
use uuid::Uuid;

const DEFAULT_MAXIMUM_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const HARD_MAXIMUM_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAXIMUM_FORUM_BODY_BYTES: usize = 65_531;
const MAXIMUM_FORUM_TITLE_RUNES: usize = 50;
const MAXIMUM_FORM_FIELDS: usize = 128;
const MAXIMUM_FORM_FIELD_BYTES: usize = 4 * 1024;
const MAXIMUM_COOKIE_BYTES: usize = 16 * 1024;
const MAXIMUM_CREDENTIAL_CANDIDATES: usize = 32;
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_MINIMUM_REQUEST_INTERVAL: Duration = Duration::from_millis(750);
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0";

/// 旧论坛生产入口。
pub const PRODUCTION_ENDPOINT: &str = "https://forums.e-hentai.org";
/// 旧版测试论坛板块。
pub const TEST_BOARD_ID: i32 = 4;
/// 旧版主世界拍卖板块。
pub const MAIN_BOARD_ID: i32 = 77;
/// 旧版异世界拍卖板块。
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
