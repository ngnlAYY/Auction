//! MoogleMail HTTP 客户端的安全响应判定、限速器和敏感数据边界。

use scraper::{ElementRef, Html, Node};

use super::*;

/// 构造不跟随重定向、关闭环境代理及自动重试的 MoogleMail HTTP client。
pub(super) fn build_mooglemail_http_client() -> Result<Client, reqwest::Error> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()
}

pub(super) fn is_html_content_type(headers: &header::HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| matches!(value.trim(), "text/html" | "application/xhtml+xml"))
}

pub(crate) fn is_hentaiverse_login_response(
    status: StatusCode,
    location: Option<&header::HeaderValue>,
    body: &[u8],
) -> bool {
    if status.is_redirection()
        && location.and_then(|value| value.to_str().ok()).is_some_and(is_login_reference)
    {
        return true;
    }
    let document = Html::parse_document(&String::from_utf8_lossy(body));
    let root = document.root_element();
    // 正常邮件、装备和余额页面存在时，正文中的短语或链接不能作废凭据；矛盾响应交给协议校验。
    if root.descendants().filter_map(ElementRef::wrap).any(is_authenticated_content) {
        return false;
    }
    let mut text = String::new();
    let mut ancestors = Vec::new();
    // 沿深度维护隐藏状态，避免嵌套页面的每个节点都重扫完整祖先链。
    for node in root.descendants() {
        let parent = node.parent().map(|parent| parent.id());
        while ancestors.last().is_some_and(|(id, _)| Some(*id) != parent) {
            ancestors.pop();
        }
        let hidden = ancestors.last().is_some_and(|(_, hidden)| *hidden)
            || matches!(node.value(), Node::Element(element)
                if matches!(element.name(), "textarea" | "script" | "style" | "noscript"));
        if matches!(node.value(), Node::Element(_)) {
            ancestors.push((node.id(), hidden));
        }
        if hidden {
            continue;
        }
        match node.value() {
            Node::Text(value) => {
                for word in value.split_whitespace() {
                    if !text.is_empty() {
                        text.push(' ');
                    }
                    text.push_str(word);
                }
            }
            Node::Element(element) => {
                let reference = match element.name() {
                    "a" => element.attr("href"),
                    "form" => element.attr("action"),
                    _ => None,
                };
                if reference.is_some_and(is_login_reference) {
                    return true;
                }
            }
            _ => {}
        }
    }
    text.make_ascii_lowercase();
    text.contains("you are not logged in")
        || text.contains("this page requires you to log on")
        || text.contains("you must be logged on to visit the hentaiverse")
}

fn is_authenticated_content(element: ElementRef<'_>) -> bool {
    matches!(
        element.value().attr("id"),
        Some("mmail_outer" | "mmail_list" | "networth" | "battle_main")
    ) || element.value().classes().any(|class| class == "showequip")
}

fn login_reference_url(reference: &str) -> Option<Url> {
    let base = Url::parse("https://hentaiverse.org/").ok()?;
    let url = base.join(reference).ok()?;
    (matches!(url.scheme(), "http" | "https")
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none())
    .then_some(url)
}

fn is_login_reference(reference: &str) -> bool {
    login_reference_url(reference).is_some_and(|url| {
        matches!(url.host_str(), Some("hentaiverse.org" | "e-hentai.org"))
            && url.path() == "/bounce_login.php"
    })
}

pub(super) fn is_session_expired(
    status: StatusCode,
    location: Option<&header::HeaderValue>,
    body: &[u8],
) -> bool {
    if is_hentaiverse_login_response(status, location, body) {
        return true;
    }
    status.is_redirection()
        && location.and_then(|value| value.to_str().ok()).and_then(login_reference_url).is_some_and(
            |url| matches!(url.host_str(), Some("e-hentai.org" | "forums.e-hentai.org")),
        )
}

/// HV 会在角色处于战斗时用战斗页替代 Item Shop/MoogleMail 页面。
/// 该状态不表示 Cookie 失效或页面协议漂移；调用方应保留当前凭据并在战斗结束后重试。
pub(super) fn is_hentaiverse_battle_response(body: &[u8]) -> bool {
    // 正常响应不复制整页；仅命中候选标记时才解析 DOM，正文或脚本里的文字不算战斗页。
    if !body.windows(b"battle_main".len()).any(|value| value.eq_ignore_ascii_case(b"battle_main")) {
        return false;
    }
    let document = Html::parse_document(&String::from_utf8_lossy(body));
    document.root_element().descendants().filter_map(ElementRef::wrap).any(|element| {
        element.attr("id").is_some_and(|value| value.eq_ignore_ascii_case("battle_main"))
    })
}

pub(super) fn post_allowed(
    target_operation: MoogleMailOperation,
    form_operation: MoogleMailOperation,
) -> bool {
    match target_operation {
        MoogleMailOperation::Compose => matches!(
            form_operation,
            MoogleMailOperation::AttachAdd
                | MoogleMailOperation::AttachCod
                | MoogleMailOperation::Send
                | MoogleMailOperation::Discard
        ),
        MoogleMailOperation::Detail => {
            matches!(form_operation, MoogleMailOperation::ReturnMessage | MoogleMailOperation::Take)
        }
        MoogleMailOperation::InventoryOrganize => matches!(
            form_operation,
            MoogleMailOperation::EquipmentUnstore | MoogleMailOperation::EquipmentUnlock
        ),
        _ => false,
    }
}

pub(super) struct IntervalGate {
    interval: Duration,
    next: Mutex<Instant>,
}

impl IntervalGate {
    pub(super) fn new(interval: Duration) -> Self {
        Self { interval, next: Mutex::new(Instant::now()) }
    }

    pub(super) async fn wait(&self) -> Result<(), ()> {
        // 只有实际获准的请求推进时间槽；预算取消不会留下未发送请求的限速债务。
        let mut next = self.next.lock().await;
        let delay = next.saturating_duration_since(Instant::now());
        if !delay.is_zero() {
            time::sleep(delay).await;
        }
        *next = Instant::now().checked_add(self.interval).ok_or(())?;
        Ok(())
    }
}
