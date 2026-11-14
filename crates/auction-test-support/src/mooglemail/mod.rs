//! 邮件语料的有状态 HTTP 回放；故障标记是人工注入，不冒充真实页面。

use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{Method, StatusCode, Uri},
    response::{Html, IntoResponse, Response},
    routing::any,
};
use scraper::{Html as Document, Selector};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{net::TcpListener, task::JoinHandle};
use url::Url;

mod fixtures;

/// 真实发送成功页；仅替换玩家/世界/收件人显示名并移除扩展 DOM。
const WRITE_SUCCESS: &[u8] =
    include_bytes!("../../../auction-integrations/tests/fixtures/w6_6/compose-success.html");

/// 缺失真实语料的服务类别；后续扩展必须先补充获准语料。
#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("mooglemail fixture unavailable: {0:?}")]
pub struct UnsupportedFixture(pub UnsupportedService);

/// 当前不能回放的类别。
#[derive(Debug, Clone, Copy)]
pub enum UnsupportedService {
    Forum,
    Battle,
    Login,
}

/// 人工故障注入，独立于真实语料来源。
#[derive(Debug, Clone, Copy)]
pub enum Fault {
    /// 最小 battle_main DOM 标记，不是战斗页 fixture。
    AccountBusy,
    /// 登录跳转响应，不提供虚构登录页。
    CookieInvalid,
    /// 在响应前延迟，供客户端自己的超时预算裁决。
    Timeout(Duration),
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
}

#[derive(Default)]
struct MailState {
    fault: Option<Fault>,
    unread: BTreeSet<String>,
    removed_sent: BTreeSet<String>,
    reads: usize,
    writes: usize,
    requests: Vec<String>,
    captured_pages_complete: bool,
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

    /// 为未来论坛支持保留显式失败入口，不构造缺失页面。
    pub fn require_fixture(&self, service: UnsupportedService) -> Result<(), UnsupportedFixture> {
        Err(UnsupportedFixture(service))
    }
}

impl Drop for MailDouble {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn handle(
    State(state): State<Arc<Mutex<MailState>>>,
    method: Method,
    uri: Uri,
    _body: Bytes,
) -> Response {
    // 不支持的路由也记录，便于测试指出扫描请求了哪些未采集页面。
    state.lock().expect("test state poisoned").requests.push(format!("{method} {uri}"));
    let query: BTreeMap<_, _> =
        url::form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes())
            .into_owned()
            .collect();
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
    match fault {
        Some(Fault::AccountBusy) => return Html("<div id=\"battle_main\"></div>").into_response(),
        Some(Fault::CookieInvalid) => {
            return (
                StatusCode::FOUND,
                [("location", "https://hentaiverse.org/bounce_login.php")],
                "",
            )
                .into_response();
        }
        Some(Fault::Timeout(delay)) => tokio::time::sleep(delay).await,
        Some(Fault::SlowWriteSucceeded(delay)) if method == Method::POST => {
            tokio::time::sleep(delay).await;
        }
        Some(Fault::ProtocolDrift) => {
            return Html("<div>injected protocol drift</div>").into_response();
        }
        _ => {}
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
