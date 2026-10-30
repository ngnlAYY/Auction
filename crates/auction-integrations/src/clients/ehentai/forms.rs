//! E-Hentai 隐藏表单提取、URL/origin 校验与响应协议判断。

use std::sync::LazyLock;

use super::*;

static FORM_SELECTOR: LazyLock<Result<Selector, ()>> =
    LazyLock::new(|| Selector::parse("form").map_err(|_| ()));
static HIDDEN_INPUT_SELECTOR: LazyLock<Result<Selector, ()>> =
    LazyLock::new(|| Selector::parse("input[type='hidden'][name]").map_err(|_| ()));
static TOPIC_TITLE_SELECTOR: LazyLock<Result<Selector, ()>> =
    LazyLock::new(|| Selector::parse("[name='TopicTitle']").map_err(|_| ()));
static TOPIC_METADATA_SELECTOR: LazyLock<Result<Selector, ()>> =
    LazyLock::new(|| Selector::parse("[name='TopicTitle'], [name='TopicDesc']").map_err(|_| ()));
static POST_BODY_SELECTOR: LazyLock<Result<Selector, ()>> =
    LazyLock::new(|| Selector::parse("[name='Post']").map_err(|_| ()));
static LINK_SELECTOR: LazyLock<Result<Selector, ()>> =
    LazyLock::new(|| Selector::parse("a[href]").map_err(|_| ()));
static LOGIN_FIELD_SELECTOR: LazyLock<Result<Selector, ()>> =
    LazyLock::new(|| Selector::parse("input[name]").map_err(|_| ()));

fn cached_selector(
    cache: &'static LazyLock<Result<Selector, ()>>,
) -> Result<&'static Selector, ()> {
    LazyLock::force(cache).as_ref().map_err(|_| ())
}

pub(super) fn form_selector() -> Result<&'static Selector, ()> {
    cached_selector(&FORM_SELECTOR)
}

pub(super) fn hidden_input_selector() -> Result<&'static Selector, ()> {
    cached_selector(&HIDDEN_INPUT_SELECTOR)
}

pub(super) fn topic_title_selector() -> Result<&'static Selector, ()> {
    cached_selector(&TOPIC_TITLE_SELECTOR)
}

pub(super) fn post_body_selector() -> Result<&'static Selector, ()> {
    cached_selector(&POST_BODY_SELECTOR)
}

pub(super) fn link_selector() -> Result<&'static Selector, ()> {
    cached_selector(&LINK_SELECTOR)
}

pub(super) struct TopicForm {
    pub(super) action: Url,
    pub(super) referer: Url,
    pub(super) values: Vec<(String, String)>,
}

/// 已校验的回复/编辑表单；目标身份来自隐藏字段，不能由响应正文推断覆盖。
pub(super) struct PostForm {
    pub(super) action: Url,
    pub(super) referer: Url,
    pub(super) values: Vec<(String, String)>,
    /// 服务端编辑表单中的原始 BBCode；回复表单通常为空。
    pub(super) body: String,
    /// 只有首帖编辑表单携带主题标题与描述；不从本地期望值补齐。
    pub(super) metadata: Option<ForumTopicMetadata>,
}

pub(super) fn posting_form_values(
    form: scraper::ElementRef<'_>,
    input_selector: &Selector,
    title_selector: &Selector,
    post_selector: &Selector,
    board_id: i32,
) -> Option<Vec<(String, String)>> {
    if form.select(title_selector).next().is_none() || form.select(post_selector).next().is_none() {
        return None;
    }
    let mut values = Vec::new();
    for input in form.select(input_selector) {
        let name = input.value().attr("name")?.to_owned();
        let value = input.value().attr("value").unwrap_or_default().to_owned();
        if !valid_form_field(&name, &value) || values.len() >= MAXIMUM_FORM_FIELDS {
            return None;
        }
        values.push((name, value));
    }
    let board = board_id.to_string();
    if single_value_for(&values, "act") != Some("Post")
        || single_value_for(&values, "CODE") != Some("01")
        || single_value_for(&values, "f") != Some(board.as_str())
        || single_value_for(&values, "auth_key").is_none_or(|value| value.is_empty())
    {
        return None;
    }
    Some(values)
}

pub(super) fn fixed_post_form_values(
    form: scraper::ElementRef<'_>,
    input_selector: &Selector,
    post_selector: &Selector,
    expected_code: &str,
    board_id: i32,
    thread_id: &str,
    post_id: Option<&str>,
) -> Option<Vec<(String, String)>> {
    form.select(post_selector).next()?;
    let mut values = Vec::new();
    for input in form.select(input_selector) {
        let name = input.value().attr("name")?.to_owned();
        let value = input.value().attr("value").unwrap_or_default().to_owned();
        if !valid_form_field(&name, &value) || values.len() >= MAXIMUM_FORM_FIELDS {
            return None;
        }
        values.push((name, value));
    }
    let board = board_id.to_string();
    let post_target_matches = match post_id {
        Some(wanted) => single_value_for(&values, "p") == Some(wanted),
        None => single_value_for(&values, "p").is_none(),
    };
    if single_value_for(&values, "act") != Some("Post")
        || single_value_for(&values, "CODE") != Some(expected_code)
        || single_value_for(&values, "f") != Some(board.as_str())
        || single_value_for(&values, "t") != Some(thread_id)
        || single_value_for(&values, "auth_key").is_none_or(|value| value.is_empty())
        || !post_target_matches
        || post_id.is_some() && single_value_for(&values, "st") != Some("0")
    {
        return None;
    }
    Some(values)
}

pub(super) fn fixed_post_metadata(
    form: scraper::ElementRef<'_>,
) -> Result<Option<ForumTopicMetadata>, ()> {
    let selector = cached_selector(&TOPIC_METADATA_SELECTOR)?;
    let mut title = None;
    let mut description = None;
    for field in form.select(selector) {
        if field.value().name() != "input" {
            return Err(());
        }
        let value = field.value().attr("value").ok_or(())?;
        let destination = match field.value().attr("name") {
            Some("TopicTitle") if valid_topic_title(value) => &mut title,
            Some("TopicDesc") if valid_topic_description(value) => &mut description,
            _ => return Err(()),
        };
        if destination.replace(value.to_owned()).is_some() {
            return Err(());
        }
    }
    match (title, description) {
        (None, None) => Ok(None),
        (Some(title), Some(description)) => {
            Ok(Some(ForumTopicMetadata { title: Some(title), description: Some(description) }))
        }
        _ => Err(()),
    }
}

pub(super) fn posting_action_matches_board(action: &Url, board_id: i32) -> bool {
    let wanted = board_id.to_string();
    optional_query_identity(action, &["f", "showforum"])
        .is_ok_and(|value| value.is_none_or(|value| value == wanted))
}

pub(super) fn posting_action_matches_target(
    action: &Url,
    board_id: i32,
    thread_id: &str,
    post_id: Option<&str>,
    expected_code: &str,
) -> bool {
    if action.path() != "/index.php"
        || !posting_action_matches_board(action, board_id)
        || !optional_query_identity(action, &["act"])
            .is_ok_and(|value| value.is_none_or(|value| value == "Post"))
        || !optional_query_identity(action, &["CODE"])
            .is_ok_and(|value| value.is_none_or(|value| value == expected_code))
        || !optional_query_identity(action, &["t", "showtopic"])
            .is_ok_and(|value| value.is_none_or(|value| value == thread_id))
    {
        return false;
    }
    match (post_id, optional_query_identity(action, &["p", "pid", "gopid"])) {
        (_, Err(())) | (None, Ok(Some(_))) => false,
        (None, Ok(None)) => true,
        (Some(wanted), Ok(observed)) => observed.is_none_or(|value| value == wanted),
    }
}

pub(super) fn optional_query_identity(url: &Url, aliases: &[&str]) -> Result<Option<String>, ()> {
    let mut matches =
        url.query_pairs().filter(|(name, _)| aliases.iter().any(|alias| name.as_ref() == *alias));
    let value = matches.next().map(|(_, value)| value.into_owned());
    if matches.next().is_some() { Err(()) } else { Ok(value) }
}

pub(super) fn set_form_value(values: &mut Vec<(String, String)>, name: &str, value: String) {
    values.retain(|(current, _)| current != name);
    values.push((name.to_owned(), value));
}

fn single_value_for<'a>(values: &'a [(String, String)], name: &str) -> Option<&'a str> {
    let mut matches = values.iter().filter(|(current, _)| current == name);
    let value = matches.next()?.1.as_str();
    // 真实论坛编辑表单会重复生成相同的 `st=0`；相同重复不改变编码语义，冲突值仍失败关闭。
    matches.all(|(_, current)| current == value).then_some(value)
}

pub(super) fn cookie_header(
    cookie: &EhentaiCookie,
    operation: EhentaiOperation,
) -> Result<HeaderValue, EhentaiError> {
    if !valid_cookie(cookie) {
        return Err(EhentaiError::credential(operation));
    }
    let mut header =
        HeaderValue::from_str(cookie.as_str()).map_err(|_| EhentaiError::credential(operation))?;
    header.set_sensitive(true);
    Ok(header)
}

pub(super) fn valid_cookie(cookie: &EhentaiCookie) -> bool {
    cookie.is_active()
        && !cookie.as_str().is_empty()
        && cookie.as_str().len() <= MAXIMUM_COOKIE_BYTES
}

pub(super) fn valid_rendered_thread(rendered: &ForumRenderedThread) -> bool {
    let body = rendered.body.trim();
    valid_topic_titles(&rendered.title, &rendered.reconciliation_titles)
        && valid_topic_description(&rendered.description)
        && rendered.catalog_continuation.len() < auction_application::MAX_FORUM_CATALOG_PAGES
        && rendered.catalog_continuation.iter().all(valid_rendered_post)
        && !body.is_empty()
        && rendered.body.len() <= MAXIMUM_FORUM_BODY_BYTES
        && rendered
            .body
            .chars()
            .all(|character| !character.is_control() || matches!(character, '\n' | '\r' | '\t'))
}

pub(super) fn valid_rendered_post(rendered: &ForumRenderedPost) -> bool {
    let body = rendered.body.trim();
    !body.is_empty()
        && rendered.body.len() <= MAXIMUM_FORUM_BODY_BYTES
        && rendered
            .body
            .chars()
            .all(|character| !character.is_control() || matches!(character, '\n' | '\r' | '\t'))
}

pub(super) fn valid_topic_title(title: &str) -> bool {
    !title.is_empty()
        && title == title.trim()
        && title.chars().count() <= MAXIMUM_FORUM_TITLE_RUNES
        && title.chars().all(|character| !character.is_control())
}

pub(super) fn valid_topic_titles(title: &str, reconciliation_titles: &[String]) -> bool {
    valid_topic_title(title)
        && reconciliation_titles.len() <= MAX_FORUM_RECONCILIATION_TITLES
        && reconciliation_titles.iter().enumerate().all(|(index, candidate)| {
            valid_topic_title(candidate)
                && candidate != title
                && !reconciliation_titles[..index].contains(candidate)
        })
}

pub(super) fn valid_topic_description(description: &str) -> bool {
    description == description.trim()
        && description.len() <= MAXIMUM_FORM_FIELD_BYTES
        && !description
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\t'))
}

pub(super) fn valid_form_field(name: &str, value: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && value.len() <= MAXIMUM_FORM_FIELD_BYTES
        && name.is_ascii()
        && !name.chars().any(char::is_control)
        && !value.chars().any(|character| matches!(character, '\0' | '\r' | '\n'))
}

pub(super) fn valid_topic_id(value: &str) -> bool {
    auction_domain::valid_positive_decimal(value, 32)
}

pub(super) fn valid_post_id(value: &str) -> bool {
    valid_topic_id(value)
}

pub(super) fn clean_line(value: &str) -> String {
    let mut cleaned = String::with_capacity(value.len());
    for word in value.split_whitespace() {
        if !cleaned.is_empty() {
            cleaned.push(' ');
        }
        cleaned.push_str(word);
    }
    cleaned
}

pub(super) fn body_looks_like_html(body: &[u8]) -> bool {
    let text = String::from_utf8_lossy(body).to_ascii_lowercase();
    text.contains("<html") || text.contains("<form") || text.contains("<body")
}

pub(super) fn is_session_expired(
    status: StatusCode,
    location: Option<&HeaderValue>,
    body: &[u8],
) -> bool {
    // 论坛前置 Cloudflare 也会返回 403；只有 401 或正文中的明确登录证据才能作废 Cookie。
    if status == StatusCode::UNAUTHORIZED {
        return true;
    }
    if location.and_then(|value| value.to_str().ok()).is_some_and(login_location) {
        return true;
    }
    let document = Html::parse_document(&String::from_utf8_lossy(body));
    // 用户帖子、主题标题和签名中的登录文案不代表会话失效；只接受正文区外的登录表单。
    let (Ok(forms), Ok(fields)) = (form_selector(), cached_selector(&LOGIN_FIELD_SELECTOR)) else {
        return false;
    };
    document.select(forms).any(|form| {
        if form.ancestors().filter_map(scraper::ElementRef::wrap).any(|element| {
            element.value().classes().any(|class| matches!(class, "postcolor" | "signature"))
        }) {
            return false;
        }
        let mut username = false;
        let mut password = false;
        for field in form.select(fields) {
            let name = field.attr("name").unwrap_or_default();
            username |= name.eq_ignore_ascii_case("username");
            password |=
                name.eq_ignore_ascii_case("password") || name.eq_ignore_ascii_case("userpass");
        }
        password && (username || form.attr("action").is_some_and(login_location))
    })
}

fn login_location(reference: &str) -> bool {
    let Ok(url) = Url::parse("https://forums.e-hentai.org/").and_then(|base| base.join(reference))
    else {
        return false;
    };
    url.query_pairs()
        .any(|(key, value)| key.eq_ignore_ascii_case("act") && value.eq_ignore_ascii_case("login"))
        || matches!(url.path().trim_end_matches('/'), "/login" | "/login.php")
}

pub(super) fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

pub(super) fn origin_of(url: &Url) -> Option<String> {
    let host = url.host_str()?;
    let mut origin = format!("{}://{}", url.scheme(), host);
    if let Some(port) = url.port() {
        origin.push(':');
        origin.push_str(&port.to_string());
    }
    Some(origin)
}

pub(super) fn is_loopback_host(url: &Url) -> bool {
    // host_str 的 IPv6 文本含方括号；使用解析后的地址且保留论坛原有 IPv4 单地址边界。
    match url.host() {
        Some(url::Host::Domain("localhost")) => true,
        Some(url::Host::Ipv4(address)) => address == std::net::Ipv4Addr::LOCALHOST,
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        _ => false,
    }
}

pub(super) fn build_forum_http_client() -> Result<Client, reqwest::Error> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    // 单次论坛写入不能由传输层隐式重放；重试决定只能来自持久任务及远端证明。
    Client::builder().no_proxy().redirect(Policy::none()).retry(reqwest::retry::never()).build()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 论坛编辑表单的最小真实结构；只替换第二个 `st` 字段值。
    fn edit_form_with_second_offset(offset: &str) -> String {
        format!(
            r#"
            <form action="index.php" method="post">
              <input type="hidden" name="st" value="0">
              <input type="hidden" name="act" value="Post">
              <input type="hidden" name="f" value="4">
              <input type="hidden" name="auth_key" value="0123456789abcdef0123456789abcdef">
              <input type="hidden" name="CODE" value="09">
              <input type="hidden" name="t" value="300433">
              <input type="hidden" name="p" value="6956800">
              <input type="hidden" name="st" value="{offset}">
              <textarea name="Post">System log</textarea>
            </form>
        "#
        )
    }

    fn parse_edit_form(html: &str) -> Option<Vec<(String, String)>> {
        let document = Html::parse_document(html);
        let form = document
            .select(
                form_selector().unwrap_or_else(|error| panic!("表单 selector 应合法: {error:?}")),
            )
            .next()
            .unwrap_or_else(|| panic!("表单应存在"));
        fixed_post_form_values(
            form,
            hidden_input_selector()
                .unwrap_or_else(|error| panic!("隐藏字段 selector 应合法: {error:?}")),
            post_body_selector().unwrap_or_else(|error| panic!("正文 selector 应合法: {error:?}")),
            "09",
            4,
            "300433",
            Some("6956800"),
        )
    }

    #[test]
    fn fixed_post_form_accepts_identical_duplicate_offset_fields() {
        let values = parse_edit_form(&edit_form_with_second_offset("0"))
            .unwrap_or_else(|| panic!("论坛生成的相同重复 st 应可接受"));

        assert_eq!(single_value_for(&values, "st"), Some("0"));
    }

    #[test]
    fn fixed_post_form_rejects_conflicting_duplicate_offset_fields() {
        assert!(parse_edit_form(&edit_form_with_second_offset("20")).is_none());
    }
}
