//! 论坛与战斗页回放；覆盖层显式区分测试载荷和真实页面结构。

use axum::{
    Router,
    extract::State,
    http::{Method, StatusCode, Uri},
    response::{Html, IntoResponse, Response},
    routing::any,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{net::TcpListener, task::JoinHandle};
use url::Url;

const TOPIC: &str =
    include_str!("../../../auction-integrations/tests/fixtures/w6_3/forum/topic-302133.html");
const LOGIN: &str =
    include_str!("../../../auction-integrations/tests/fixtures/w6_3/forum/login.html");
const SENT: &str =
    include_str!("../../../auction-integrations/tests/fixtures/w6_3/forum/pm-sent-list.html");
const DETAIL: &str =
    include_str!("../../../auction-integrations/tests/fixtures/w6_3/forum/pm-detail-1558026.html");
const BATTLE: &str =
    include_str!("../../../auction-integrations/tests/fixtures/w6_3/forum/battle-main.html");
const SUBJECT: &str = "请求增加H@H服务器";

/// 人工故障持续生效，直到测试显式清除。
#[derive(Clone, Copy, Debug)]
pub enum Fault {
    /// 返回真实登录页，模拟原 Cookie 已失效。
    CookieInvalid,
    /// 正确 HTML 类型，但缺少协议结构。
    ProtocolDrift,
    /// 响应延迟由真实客户端的预算裁决。
    Timeout(Duration),
    /// 移除唯一帖子正文，不伪造远端删除成功证明。
    DeletedPost,
    /// 仅在已知回帖/PM POST 路径收到请求时注入未知结果。
    WriteResultUnknown,
    /// 返回真实主世界战斗页。
    Battle,
}

#[derive(Default)]
struct ForumState {
    fault: Option<Fault>,
    pages: BTreeMap<u64, String>,
    pm_payload: Option<(String, String)>,
    writes: usize,
    requests: Vec<String>,
}

/// 仅拥有 IPv4 回环监听；Drop 终止服务，不连接真实账号。
pub struct ForumDouble {
    endpoint: Url,
    state: Arc<Mutex<ForumState>>,
    task: JoinHandle<()>,
}

impl ForumDouble {
    /// 启动独立测试服务，默认仅挂载实际采集的第零页。
    pub async fn start() -> Result<Self, std::io::Error> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let endpoint = Url::parse(&format!("http://{}/", listener.local_addr()?))
            .map_err(std::io::Error::other)?;
        let state = Arc::new(Mutex::new(ForumState::default()));
        let task_state = Arc::clone(&state);
        let task = tokio::spawn(async move {
            let _ =
                axum::serve(listener, Router::new().fallback(any(handle)).with_state(task_state))
                    .await;
        });
        Ok(Self { endpoint, state, task })
    }

    /// 交给真实客户端的固定 origin。
    pub fn endpoint(&self) -> Url {
        self.endpoint.clone()
    }

    /// 注入持续故障。
    pub fn set_fault(&self, fault: Option<Fault>) {
        self.state.lock().expect("test state poisoned").fault = fault;
    }

    /// 挂载明确提供的主题分页；不会为缺失 st= 页面复制或编造帖子。
    pub fn set_topic_page(&self, offset: u64, html: String) {
        self.state.lock().expect("test state poisoned").pages.insert(offset, html);
    }

    /// 在真实 PM 结构中替换标题与正文，测试冻结通知匹配；并非原始采集内容。
    pub fn set_pm_payload(&self, title: &str, body: &str) {
        self.state.lock().expect("test state poisoned").pm_payload =
            Some((escape(title), escape(body)));
    }

    /// 已抵达替身的 POST 次数；未知结果不能变成成功或自动重试。
    pub fn write_requests(&self) -> usize {
        self.state.lock().expect("test state poisoned").writes
    }

    /// 请求路径用于证明分页与详情实际经过真实客户端。
    pub fn requests(&self) -> Vec<String> {
        self.state.lock().expect("test state poisoned").requests.clone()
    }
}

impl Drop for ForumDouble {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn handle(State(state): State<Arc<Mutex<ForumState>>>, method: Method, uri: Uri) -> Response {
    let mut query = BTreeMap::new();
    for (key, value) in url::form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes()) {
        if query.insert(key.into_owned(), value.into_owned()).is_some() {
            return StatusCode::BAD_REQUEST.into_response();
        }
    }
    let value = |key: &str| query.get(key).map(String::as_str);
    let topic = uri.path() == "/index.php"
        && value("showtopic") == Some("302133")
        && query.keys().all(|key| matches!(key.as_str(), "showtopic" | "st" | "s"));
    let pm = uri.path() == "/index.php"
        && value("act") == Some("Msg")
        && value("VID") == Some("sent")
        && query.keys().all(|key| {
            matches!(key.as_str(), "act" | "CODE" | "VID" | "MSID" | "sort" | "st" | "s")
        });
    let login = uri.path() == "/index.php"
        && value("act") == Some("Login")
        && value("CODE") == Some("00")
        && query.keys().all(|key| matches!(key.as_str(), "act" | "CODE" | "s"));
    let battle = uri.path() == "/"
        && value("s") == Some("Battle")
        && value("ss") == Some("rb")
        && query.len() == 2;
    let reply_write = uri.path() == "/index.php"
        && value("act") == Some("Post")
        && value("CODE") == Some("03")
        && value("t") == Some("302133")
        && value("f").is_none_or(|value| value == "4")
        && query.keys().all(|key| matches!(key.as_str(), "act" | "CODE" | "t" | "f"));
    let pm_write = uri.path() == "/index.php" && value("act") == Some("msg") && query.len() == 1;
    let fault = {
        let mut state = state.lock().expect("test state poisoned");
        state.requests.push(uri.to_string());
        if method == Method::POST {
            state.writes += 1;
        }
        state.fault
    };
    // 邮件请求仅支持真实战斗页故障；正常邮件内容归 MailDouble 所有。
    if method == Method::GET
        && matches!(fault, Some(Fault::Battle))
        && uri.path() == "/"
        && value("s") == Some("Bazaar")
        && value("ss") == Some("mm")
    {
        return Html(BATTLE).into_response();
    }
    if method == Method::POST {
        if (reply_write || pm_write) && matches!(fault, Some(Fault::WriteResultUnknown)) {
            return Html("<div>injected unknown write outcome</div>").into_response();
        }
        return unsupported("write/form success fixture unavailable");
    }
    if !topic && !pm && !login && !battle {
        return unsupported("route fixture unavailable");
    }
    if method != Method::GET {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    match fault {
        Some(Fault::CookieInvalid) => return Html(LOGIN).into_response(),
        Some(Fault::Battle) => return Html(BATTLE).into_response(),
        Some(Fault::ProtocolDrift) => {
            return Html("<div>injected protocol drift</div>").into_response();
        }
        Some(Fault::Timeout(delay)) => tokio::time::sleep(delay).await,
        _ => {}
    }
    let state = state.lock().expect("test state poisoned");
    let source = if login {
        LOGIN.to_owned()
    } else if battle {
        BATTLE.to_owned()
    } else if topic {
        let offset = match value("st").unwrap_or("0").parse::<u64>() {
            Ok(offset) => offset,
            Err(_) => return StatusCode::BAD_REQUEST.into_response(),
        };
        let source = match state.pages.get(&offset) {
            Some(page) => page.as_str(),
            None if offset == 0 => TOPIC,
            None => return unsupported("topic pagination fixture unavailable"),
        };
        if matches!(fault, Some(Fault::DeletedPost)) {
            remove_post(source)
        } else {
            source.to_owned()
        }
    } else {
        match value("CODE") {
            Some("01")
                if value("MSID").is_none()
                    && value("st").is_none_or(|v| v.is_empty() || v == "0") =>
            {
                SENT.to_owned()
            }
            Some("03")
                if value("MSID") == Some("1558026")
                    && value("st").is_none()
                    && value("sort").is_none() =>
            {
                DETAIL.to_owned()
            }
            _ => return unsupported("PM page/detail fixture unavailable"),
        }
    };
    let mut source = source.replace("https://forums.e-hentai.org/", "/");
    if pm && let Some((title, body)) = &state.pm_payload {
        source = source.replace(SUBJECT, title);
        if value("CODE") == Some("03") {
            let document = scraper::Html::parse_document(&source);
            let selector = scraper::Selector::parse(".postcolor").expect("constant selector");
            let node = document.select(&selector).next().expect("captured PM body");
            source = source.replace(&node.inner_html(), body);
        }
    }
    Html(source).into_response()
}

fn unsupported(reason: &'static str) -> Response {
    (StatusCode::NOT_IMPLEMENTED, reason).into_response()
}

fn remove_post(source: &str) -> String {
    let document = scraper::Html::parse_document(source);
    let selector = scraper::Selector::parse(".postcolor").expect("constant selector");
    let mut result = document.root_element().html();
    for post in document.select(&selector) {
        result = result.replace(&post.html(), "");
    }
    result
}

fn escape(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}
