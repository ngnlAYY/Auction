//! PM 固定目标、身份与纯文本证明；任何未知操作字段均失败关闭。

use std::collections::BTreeMap;
use std::collections::HashSet;

use auction_application::core::diagnostics::DiagnosticContext;
use auction_application::forum::{ForumPmError, ForumPmMessage};
use scraper::{ElementRef, Node};
use scraper::{Html, Selector};
use url::Url;

use crate::clients::ehentai::forms::{same_origin, valid_topic_id};
use crate::safe_diagnostics;

pub(super) fn selector(value: &str) -> Result<Selector, ForumPmError> {
    Selector::parse(value).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "forum_pm",
            "build_selector",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        ForumPmError::Protocol
    })
}

pub(super) fn validate_message(message: &ForumPmMessage) -> Result<(), ForumPmError> {
    if !valid_topic_id(&message.recipient_forum_account_id) {
        return Err(ForumPmError::InvalidRecipient);
    }
    if message.notification_id.is_nil()
        || message.title != format!("Outbid {}", message.notification_id.simple())
        || !(2..=2048).contains(&message.body.len())
        || !message.body.contains(&message.notification_id.to_string())
        || message.body.bytes().any(|b| !(0x20..=0x7e).contains(&b) || b"[]<>".contains(&b))
        || message.body != message.body.trim()
    {
        return Err(ForumPmError::Protocol);
    }
    Ok(())
}

/// 仅接受同源 index.php；session 参数只在内存解析并立即去除，绝不作为下一请求目标。
pub(super) fn read_link(
    base: &Url,
    reference: &str,
    allowed: &[&str],
) -> Result<(Url, BTreeMap<String, String>), ForumPmError> {
    let mut url = base.join(reference).map_err(|diagnostic_cause| {
        safe_diagnostics::warn(
            "forum_pm",
            "resolve_read_link",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        ForumPmError::Protocol
    })?;
    if !same_origin(base, &url)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/index.php"
        || url.fragment().is_some()
    {
        return Err(ForumPmError::Protocol);
    }
    let mut pairs = BTreeMap::new();
    for (key, value) in url.query_pairs() {
        if pairs.len() >= 8
            || value.len() > 1024
            || (key != "s" && !allowed.contains(&key.as_ref()))
            || pairs.insert(key.into_owned(), value.into_owned()).is_some()
        {
            return Err(ForumPmError::Protocol);
        }
    }
    pairs.remove("s");
    url.set_query(None);
    url.query_pairs_mut().extend_pairs(pairs.iter());
    Ok((url, pairs))
}

pub(super) fn profile_id(base: &Url, reference: &str) -> Result<String, ForumPmError> {
    let (_, pairs) = read_link(base, reference, &["showuser"])?;
    let value =
        pairs.get("showuser").filter(|v| valid_topic_id(v)).ok_or(ForumPmError::Protocol)?;
    Ok(value.clone())
}

pub(super) fn has_query(base: &Url, reference: &str, key: &str) -> bool {
    base.join(reference).is_ok_and(|url| url.query_pairs().any(|(k, _)| k == key))
}

pub(super) fn sender_identity(document: &Html, base: &Url) -> Result<String, ForumPmError> {
    let navigation_selector = selector("#userlinks")?;
    let mut navigation = document.select(&navigation_selector);
    let navigation_node = navigation.next().ok_or(ForumPmError::Protocol)?;
    if navigation.next().is_some() {
        return Err(ForumPmError::Protocol);
    }
    let mut identities = HashSet::new();
    for link in navigation_node.select(&selector("a[href]")?) {
        let reference = link.attr("href").ok_or(ForumPmError::Protocol)?;
        if has_query(base, reference, "showuser") {
            identities.insert(profile_id(base, reference)?);
        }
    }
    if identities.len() != 1 {
        return Err(ForumPmError::Protocol);
    }
    identities.into_iter().next().ok_or(ForumPmError::Protocol)
}

/// 只接受文本与 br；不把图片、隐藏控件、引用或脚本当作完整冻结正文。
pub(super) fn plain_body(element: ElementRef<'_>) -> Result<String, ForumPmError> {
    let mut result = String::new();
    for node in element.descendants().skip(1) {
        match node.value() {
            Node::Text(text) => result.push_str(text),
            Node::Element(value) if value.name() == "br" => result.push('\n'),
            _ => return Err(ForumPmError::Protocol),
        }
        if result.len() > 8192 {
            return Err(ForumPmError::Protocol);
        }
    }
    // 冻结正文是单行 ASCII；仅消除 HTML 布局产生的 ASCII 空白，不折叠非 ASCII 字符。
    if !result.is_ascii() {
        return Err(ForumPmError::Protocol);
    }
    Ok(normalize(&result))
}

pub(super) fn normalize(value: &str) -> String {
    let value = value.trim_ascii();
    let mut normalized = String::with_capacity(value.len());
    for word in value.split_ascii_whitespace() {
        if !normalized.is_empty() {
            normalized.push(' ');
        }
        normalized.push_str(word);
    }
    normalized
}
