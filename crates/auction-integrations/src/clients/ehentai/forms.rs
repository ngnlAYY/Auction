//! E-Hentai 隐藏表单提取、URL/origin 校验与响应协议判断。

use super::*;

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
    matches.next().is_none().then_some(value)
}

pub(super) fn cookie_header(
    cookie: &EhentaiCookie,
    operation: EhentaiOperation,
) -> Result<HeaderValue, EhentaiError> {
    HeaderValue::from_str(cookie.as_str()).map_err(|_| EhentaiError::credential(operation))
}

pub(super) fn valid_cookie(cookie: &EhentaiCookie) -> bool {
    !cookie.0.is_empty() && cookie.0.len() <= MAXIMUM_COOKIE_BYTES
}

pub(super) fn valid_rendered_thread(rendered: &ForumRenderedThread) -> bool {
    let body = rendered.body.trim();
    valid_topic_title(&rendered.title)
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

pub(super) fn valid_form_field(name: &str, value: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && value.len() <= MAXIMUM_FORM_FIELD_BYTES
        && name.is_ascii()
        && !name.chars().any(char::is_control)
        && !value.chars().any(|character| matches!(character, '\0' | '\r' | '\n'))
}

pub(super) fn valid_topic_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value.as_bytes()[0].is_ascii_digit()
        && value.as_bytes()[0] != b'0'
        && value.bytes().all(|byte| byte.is_ascii_digit())
}

pub(super) fn valid_post_id(value: &str) -> bool {
    valid_topic_id(value)
}

pub(super) fn clean_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
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
    if location
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().contains("login"))
    {
        return true;
    }
    let text = String::from_utf8_lossy(body).to_ascii_lowercase();
    let has_username_field = text.contains("name=\"username\"") || text.contains("name='username'");
    let has_password_field = text.contains("name=\"password\"") || text.contains("name='password'");
    // 已登录论坛页的导航仍会包含 `act=Login`，只有登录表单或明确错误文案才能证明会话失效。
    text.contains("you are not logged in")
        || text.contains("this page requires you to log on")
        || (has_username_field && has_password_field)
        || (has_password_field && text.contains("act=login"))
        || text.contains("<title>log in")
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
    matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"))
}

pub(super) fn build_forum_http_client() -> Result<Client, reqwest::Error> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    Client::builder().no_proxy().redirect(Policy::none()).build()
}
