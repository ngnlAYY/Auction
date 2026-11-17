//! 邮件语料的有状态 HTTP 回放；故障标记是人工注入，不冒充真实页面。测试另可登记一封由调用方
//! 提供页面（由真实语料派生）的待领取邮件，替身只按领取前/后两份页面有状态回放，并可让一次
//! 领取 POST 迟到完成，用于复现外部 Take 的时序。

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri, header},
    response::{Html, IntoResponse, Response},
    routing::any,
};
use scraper::{Html as Document, Selector};
use tokio::{net::TcpListener, sync::watch, task::JoinHandle};
use url::Url;

mod fixtures;
mod scripted;

/// 真实发送成功页；仅替换玩家/世界/收件人显示名并移除扩展 DOM。
const WRITE_SUCCESS: &[u8] =
    include_bytes!("../../../auction-integrations/tests/fixtures/mooglemail/compose-success.html");

/// 人工故障注入，独立于真实语料来源。
#[derive(Debug, Clone, Copy)]
pub enum Fault {
    /// 最小 battle_main DOM 标记，不是战斗页 fixture。
    AccountBusy,
    /// 先延迟再返回同一最小 battle_main 标记；模拟账号忙窗口在慢请求在途期间才打开。
    SlowAccountBusy(Duration),
    /// 登录跳转响应，不提供虚构登录页。
    CookieInvalid,
    /// 在响应前延迟，供客户端自己的超时预算裁决。
    Timeout(Duration),
    /// 远端收到请求后独立处理到延迟结束再按原路由回应：客户端断开或取消不会提前结束处理，在途
    /// 计数一直保持到延迟结束。模拟不随连接断开停止处理的真实远端，用来证明调用方取消后、延迟短于
    /// 客户端超时时，紧接的请求不会与远端仍在处理的请求重叠。
    DetachedDelay(Duration),
    /// HTML 类型正确但邮件结构缺失。
    ProtocolDrift,
    /// 收到 POST 后返回无法证明成功或失败的内容；不自动重发。
    WriteResultUnknown,
    /// 仅注入客户端已识别的拒绝提示。
    InsufficientPostage,
    /// 仅注入客户端已识别的收件人不存在提示。
    MissingRecipient,
    /// 真实发送成功页（已脱敏），覆盖发送成功证明解析。
    WriteSucceeded,
    /// 写入 POST 先延迟再返回同一真实成功页；读取不受影响。模拟停机时仍在途的发送。
    SlowWriteSucceeded(Duration),
    /// 仅注入客户端已识别的 HV 状态锁限流正文；读取与写入都返回该正文。
    StateLockLimited,
    /// 同一限流正文配 429 状态码；覆盖状态码与正文同时指示限流的响应。
    StateLockLimitedTooManyRequests,
}

#[derive(Default)]
struct MailState {
    fault: Option<Fault>,
    scripted: Option<scripted::ScriptedTake>,
    unread: BTreeSet<String>,
    removed_sent: BTreeSet<String>,
    reads: usize,
    writes: usize,
    requests: Vec<String>,
    /// 每个请求携带的 Cookie 头，按到达顺序；只保存测试夹具值。
    cookies: Vec<Option<String>>,
    captured_pages_complete: bool,
    active: usize,
    max_active: usize,
    /// 响应闸门：发送端（[`ResponseHold`]）存活期间，新到达的请求登记后挂起。
    hold: Option<watch::Receiver<()>>,
}

/// 服务与状态的 owner；Drop 中止监听，测试不会遗留后台服务。
pub struct MailDouble {
    endpoint: Url,
    state: Arc<Mutex<MailState>>,
    task: JoinHandle<()>,
}

impl MailDouble {
    /// 只绑定 IPv4 回环临时端口。
    pub async fn start() -> Result<Self, std::io::Error> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;
        let endpoint = Url::parse(&format!("http://{address}/")).map_err(std::io::Error::other)?;
        let state = Arc::new(Mutex::new(MailState::default()));
        let router = Router::new().fallback(any(handle)).with_state(Arc::clone(&state));
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Ok(Self { endpoint, state, task })
    }

    /// 交给真实客户端配置；路径和查询仍由客户端的 typed URL builder 生成。
    pub fn endpoint(&self) -> Url {
        self.endpoint.clone()
    }

    /// 持续故障，直到显式清除；不会偷偷消费为成功。
    pub fn set_fault(&self, fault: Option<Fault>) {
        self.state.lock().expect("test state poisoned").fault = fault;
    }

    /// 唯一有 Inbox/Read 配对详情的 MID，可在两集合之间移动。
    pub fn set_return_receipt_unread(&self, unread: bool) {
        let mut state = self.state.lock().expect("test state poisoned");
        if unread {
            state.unread.insert("3125357".into());
        } else {
            state.unread.remove("3125357");
        }
    }

    /// 从 Sent 列表及详情共同移除一封邮件，模拟远端状态变化。
    pub fn remove_sent(&self, mid: &str) {
        self.state.lock().expect("test state poisoned").removed_sent.insert(mid.into());
    }

    /// 撤销 `remove_sent`：先隐藏最新一封、发送后再恢复，模拟新邮件在 POST 之后才出现在 Sent。
    pub fn restore_sent(&self, mid: &str) {
        self.state.lock().expect("test state poisoned").removed_sent.remove(mid);
    }

    /// 已收到的写入请求数；未知结果仍计一次，不伪造成功邮件。
    pub fn write_requests(&self) -> usize {
        self.state.lock().expect("test state poisoned").writes
    }

    /// 已收到的读取请求数；熔断拒绝必须不增加该计数。
    pub fn read_requests(&self) -> usize {
        self.state.lock().expect("test state poisoned").reads
    }

    /// 把每个列表已采集的最后一页按真实末页结构（`<span>Next &gt;</span>`，同 isekai
    /// 采集页）回放，使完整枚举在已采集页内结束。远端真实页数更多；只供需要完整枚举的
    /// 调度与预算测试显式开启，默认关闭时未采集页仍以 501 失败关闭。
    pub fn set_captured_pages_complete(&self, complete: bool) {
        self.state.lock().expect("test state poisoned").captured_pages_complete = complete;
    }

    /// 已抵达替身的请求（方法与路径查询），按到达顺序；用于证明扫描实际经过的页面。
    pub fn requests(&self) -> Vec<String> {
        self.state.lock().expect("test state poisoned").requests.clone()
    }

    /// 同一时刻在处理中的最大请求数；用于证明客户端没有让同一账号的物理请求重叠。
    pub fn max_concurrent_requests(&self) -> usize {
        self.state.lock().expect("test state poisoned").max_active
    }

    /// 已抵达替身的请求携带的 Cookie 头，按到达顺序；用于证明请求用的是哪一份凭据。
    pub fn request_cookies(&self) -> Vec<Option<String>> {
        self.state.lock().expect("test state poisoned").cookies.clone()
    }

    /// 此后到达的请求照常登记（计入 `requests`、`request_cookies` 与在途数）后挂起，直到返回的闸门
    /// 释放；用来确定性地让一个物理请求停在远端处理中，不依赖延迟长短。
    pub fn hold_responses(&self) -> ResponseHold {
        let (release, held) = watch::channel(());
        self.state.lock().expect("test state poisoned").hold = Some(held);
        ResponseHold { _release: release }
    }
}

/// [`MailDouble::hold_responses`] 的闸门；`release` 或 Drop 后，挂起中与之后到达的请求都照常回应。
pub struct ResponseHold {
    /// 只靠存活期表示挂起：Drop 即关闭通道、放行全部等待者。
    _release: watch::Sender<()>,
}

impl ResponseHold {
    /// 放行全部挂起的请求。
    pub fn release(self) {}
}

impl Drop for MailDouble {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// 请求处理期间计入在途数；Drop 覆盖提前返回与客户端断开时的取消（`DetachedDelay` 除外，
/// 它把在途计数交给独立任务持有到延迟结束）。
struct InFlight(Arc<Mutex<MailState>>);

impl InFlight {
    fn enter(state: &Arc<Mutex<MailState>>) -> Self {
        let mut current = state.lock().expect("test state poisoned");
        current.active += 1;
        current.max_active = current.max_active.max(current.active);
        drop(current);
        Self(Arc::clone(state))
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        // Drop 中不能再 panic；锁中毒时沿用残留计数，测试断言会暴露异常。
        let mut current = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        current.active = current.active.saturating_sub(1);
    }
}

async fn handle(
    State(state): State<Arc<Mutex<MailState>>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    _body: Bytes,
) -> Response {
    let in_flight = InFlight::enter(&state);
    // 不支持的路由也记录，便于测试指出扫描请求了哪些未采集页面。
    let (detached, hold) = {
        let mut current = state.lock().expect("test state poisoned");
        current.requests.push(format!("{method} {uri}"));
        current.cookies.push(
            headers.get(header::COOKIE).and_then(|value| value.to_str().ok()).map(str::to_owned),
        );
        let detached = match current.fault {
            Some(Fault::DetachedDelay(delay)) => Some(delay),
            _ => None,
        };
        (detached, current.hold.clone())
    };
    // 闸门从不发送新值：`changed` 只在发送端释放时返回错误，此前一直挂起；释放后到达的请求立即通过。
    if let Some(mut held) = hold {
        while held.changed().await.is_ok() {}
    }
    // 不随断开停止的远端：在途计数交给独立任务持有到延迟结束；客户端断开只丢弃本 handler，
    // 不提前结束远端处理。处理结束后按原路由回应。
    let _in_flight = match detached {
        Some(delay) => {
            let processing = tokio::spawn(async move {
                tokio::time::sleep(delay).await;
                drop(in_flight);
            });
            let _ = processing.await;
            None
        }
        None => Some(in_flight),
    };
    if uri.path().starts_with("/equip/") || uri.path().starts_with("/isekai/equip/") {
        return equipment(&state, &method).await;
    }
    let query: BTreeMap<_, _> =
        url::form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes())
            .into_owned()
            .collect();
    if matches!(uri.path(), "/" | "/isekai/")
        && query.get("s").map(String::as_str) == Some("Bazaar")
        && query.get("ss").map(String::as_str) == Some("is")
    {
        return item_shop(&state, &method).await;
    }
    if !matches!(uri.path(), "/" | "/isekai/")
        || query.get("s").map(String::as_str) != Some("Bazaar")
        || query.get("ss").map(String::as_str) != Some("mm")
    {
        return (StatusCode::NOT_IMPLEMENTED, "unsupported route: fixture unavailable")
            .into_response();
    }
    let fault = {
        let mut state = state.lock().expect("test state poisoned");
        if method == Method::POST {
            state.writes += 1;
        } else if method == Method::GET {
            state.reads += 1;
        }
        state.fault
    };
    if let Some(response) = account_state_fault(fault).await {
        return response;
    }
    match fault {
        Some(Fault::Timeout(delay)) => tokio::time::sleep(delay).await,
        Some(Fault::SlowWriteSucceeded(delay)) if method == Method::POST => {
            tokio::time::sleep(delay).await;
        }
        Some(Fault::ProtocolDrift) => {
            return Html("<div>injected protocol drift</div>").into_response();
        }
        _ => {}
    }
    if let Some(response) =
        scripted::respond(&state, &method, query.get("mid").map(String::as_str)).await
    {
        return response;
    }
    if method == Method::POST {
        return match fault {
            Some(Fault::WriteResultUnknown) => {
                Html("<div>injected unknown write outcome</div>").into_response()
            }
            Some(Fault::InsufficientPostage) => {
                Html("<div id=\"messagebox_inner\">You cannot afford the postage</div>")
                    .into_response()
            }
            Some(Fault::MissingRecipient) => {
                Html("<div id=\"messagebox_inner\">Recipient not found</div>").into_response()
            }
            Some(Fault::WriteSucceeded | Fault::SlowWriteSucceeded(_)) => {
                Html(String::from_utf8_lossy(WRITE_SUCCESS).into_owned()).into_response()
            }
            _ => (StatusCode::NOT_IMPLEMENTED, "write success fixture unavailable").into_response(),
        };
    }
    if method != Method::GET {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let world = if uri.path() == "/isekai/" { "isekai" } else { "main" };
    let filter = query.get("filter").map_or("inbox", String::as_str);
    let page = match query.get("page").map(|value| value.parse::<usize>()).transpose() {
        Ok(page) => page.unwrap_or(0),
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let mid = query.get("mid").map(String::as_str);
    let mut state = state.lock().expect("test state poisoned");
    if filter == "sent" && mid.is_some_and(|mid| state.removed_sent.contains(mid)) {
        return Html(fixtures::MISSING_SENT).into_response();
    }
    let Some(source) = fixtures::page(world, filter, page, mid) else {
        return (StatusCode::NOT_IMPLEMENTED, "fixture unavailable for world/filter/page/MID")
            .into_response();
    };
    if world == "main" && filter == "inbox" && mid == Some("3125357") {
        state.unread.remove("3125357");
    }
    let html = if mid.is_none() && page == 0 && world == "main" {
        match filter {
            "inbox" if !state.unread.is_empty() => {
                list_overlay(fixtures::READ_MAIN, |mid| state.unread.contains(mid))
                    .replace("filter=read", "filter=inbox")
            }
            "read" => list_overlay(source, |mid| !state.unread.contains(mid)),
            "sent" => list_overlay(source, |mid| !state.removed_sent.contains(mid)),
            _ => source.to_owned(),
        }
    } else {
        source.to_owned()
    };
    let html = if mid.is_none()
        && state.captured_pages_complete
        && fixtures::page(world, filter, page + 1, None).is_none()
    {
        end_pagination(html)
    } else {
        html
    };
    Html(html).into_response()
}

/// 装备页没有获准的真实语料：只回放人工注入的账号状态故障，其余一律 501，不编造装备页。
/// GET 与邮件读取一样计入 `read_requests`，熔断拒绝必须不增加该计数。
async fn equipment(state: &Arc<Mutex<MailState>>, method: &Method) -> Response {
    if *method != Method::GET {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let fault = {
        let mut state = state.lock().expect("test state poisoned");
        state.reads += 1;
        state.fault
    };
    if let Some(response) = account_state_fault(fault).await {
        return response;
    }
    match fault {
        Some(Fault::Timeout(delay)) => {
            tokio::time::sleep(delay).await;
            (StatusCode::NOT_IMPLEMENTED, "equipment fixture unavailable").into_response()
        }
        _ => (StatusCode::NOT_IMPLEMENTED, "equipment fixture unavailable").into_response(),
    }
}

/// Item Shop 余额页同样没有获准的真实语料：与装备页同口径，只回放人工注入的账号状态故障
/// （登录失效、战斗页、限流）与延迟，其余一律 501，不编造余额页。GET 计入 `read_requests`。
async fn item_shop(state: &Arc<Mutex<MailState>>, method: &Method) -> Response {
    if *method != Method::GET {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let fault = {
        let mut state = state.lock().expect("test state poisoned");
        state.reads += 1;
        state.fault
    };
    if let Some(response) = account_state_fault(fault).await {
        return response;
    }
    if let Some(Fault::Timeout(delay)) = fault {
        tokio::time::sleep(delay).await;
    }
    (StatusCode::NOT_IMPLEMENTED, "item shop fixture unavailable").into_response()
}

/// 账号状态故障的回放正文：战斗页、状态锁限流与登录失效。邮件路由、装备页与 Item Shop 共用同一份，
/// 各路由的语料口径不会各自漂移；其他故障返回 `None`，由调用方按路由处理。
async fn account_state_fault(fault: Option<Fault>) -> Option<Response> {
    match fault {
        Some(Fault::AccountBusy) => Some(Html("<div id=\"battle_main\"></div>").into_response()),
        Some(Fault::SlowAccountBusy(delay)) => {
            tokio::time::sleep(delay).await;
            Some(Html("<div id=\"battle_main\"></div>").into_response())
        }
        Some(Fault::StateLockLimited) => Some(Html("state lock limiter in effect").into_response()),
        Some(Fault::StateLockLimitedTooManyRequests) => Some(
            (StatusCode::TOO_MANY_REQUESTS, Html("state lock limiter in effect")).into_response(),
        ),
        Some(Fault::CookieInvalid) => Some(
            (StatusCode::FOUND, [("location", "https://hentaiverse.org/bounce_login.php")], "")
                .into_response(),
        ),
        _ => None,
    }
}

/// 把指向未采集页的 Next 链接替换为真实末页的禁用结构；没有链接时原样返回。
fn end_pagination(html: String) -> String {
    const NEXT: &str = ">Next &gt;</a>";
    let Some(end) = html.find(NEXT) else { return html };
    let Some(start) = html[..end].rfind("<a ") else { return html };
    format!("{}<span>Next &gt;</span>{}", &html[..start], &html[end + NEXT.len()..])
}

fn list_overlay(source: &str, keep: impl Fn(&str) -> bool) -> String {
    let document = Document::parse_document(source);
    let selector = Selector::parse("table#mmail_list tr[onclick]").expect("constant selector");
    let mut html = document.root_element().html();
    let mut remaining = 0;
    for row in document.select(&selector) {
        let onclick = row.attr("onclick").unwrap_or_default();
        let mid = onclick
            .split("mid=")
            .nth(1)
            .unwrap_or_default()
            .split(|ch: char| !ch.is_ascii_digit())
            .next()
            .unwrap_or_default();
        if keep(mid) {
            remaining += 1;
        } else {
            html = html.replace(&row.html(), "");
        }
    }
    if remaining == 0 {
        // 空集合仍使用真实 Inbox 的空行结构，避免把无行表格误当协议漂移。
        let empty = Document::parse_document(
            fixtures::page("main", "inbox", 0, None).expect("captured inbox"),
        );
        let empty_selector = Selector::parse("#mmail_nnm").expect("constant selector");
        let empty_row = empty
            .select(&empty_selector)
            .next()
            .and_then(|cell| cell.parent())
            .and_then(scraper::ElementRef::wrap)
            .expect("captured empty row");
        let table_selector = Selector::parse("table#mmail_list").expect("constant selector");
        let rendered_table = Document::parse_fragment(&html);
        let current =
            rendered_table.select(&table_selector).next().expect("rendered mail table").html();
        html = html.replace(
            &current,
            &current.replace("</tbody>", &format!("{}</tbody>", empty_row.html())),
        );
    }
    html
}
