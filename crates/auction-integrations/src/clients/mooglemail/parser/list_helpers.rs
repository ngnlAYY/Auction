//! 列表页表头与翻页链接的校验。

use auction_application::core::diagnostics::DiagnosticContext;
use scraper::{ElementRef, Html};
use url::Url;

use super::{
    decode_css_glyph_label, direct_children, direct_element_children, normalize_text, protocol,
    selector, unique_element,
};
use crate::clients::mooglemail::{MoogleMailError, MoogleMailOperation, MoogleMailWorld};
use crate::safe_diagnostics;

pub(crate) fn valid_inbox_header(row: ElementRef<'_>) -> bool {
    valid_list_header(row, ["From", "Subject", "Sent", "Read"])
}

pub(crate) fn valid_sent_header(row: ElementRef<'_>) -> bool {
    valid_list_header(row, ["To", "Subject", "Sent", "Read"])
}

fn valid_list_header(row: ElementRef<'_>, expected: [&str; 4]) -> bool {
    // 只接收表头行的直接 th，避免嵌套 table 提供一组伪造列名。
    let headers = direct_children(row, "th");
    headers.len() == expected.len()
        && headers
            .iter()
            .zip(expected)
            .all(|(header, expected)| header_label(*header).as_deref() == Some(expected))
}

fn header_label(header: ElementRef<'_>) -> Option<String> {
    let visible = normalize_text(header.text().collect::<String>());
    if !visible.is_empty() {
        return Some(visible);
    }

    // 部分账号把固定英文表头渲染成 CSS 字形。兼容路径只接受真实页面已观察到的
    // `div.fl.f2b > div.c3x` 直接子结构，并在调用方继续逐字核对固定列名。
    let wrapper_children = direct_element_children(header);
    if wrapper_children.len() != 1 {
        return None;
    }
    let wrapper = wrapper_children[0];
    decode_css_glyph_label(wrapper, "fl f2b", false)
}

pub(crate) fn parse_next_page(
    document: &Html,
    world: MoogleMailWorld,
    current_page: usize,
    filter: &str,
    operation: MoogleMailOperation,
) -> Result<bool, MoogleMailError> {
    let pager_selector = selector("#mmail_pager", operation, world)?;
    let pager = unique_element(document.select(&pager_selector), operation, world)?;
    let controls = direct_children(pager, "div");
    if controls.len() != 2 {
        return Err(protocol(operation, world));
    }
    let next = controls[1];
    let links = direct_children(next, "a");
    let spans = direct_children(next, "span");
    if links.is_empty() && spans.len() == 1 && direct_element_children(next).len() == 1 {
        if normalize_text(spans[0].text().collect::<String>()) != "Next >" {
            return Err(protocol(operation, world));
        }
        return Ok(false);
    }
    if links.len() != 1
        || !spans.is_empty()
        || direct_element_children(next).len() != 1
        || normalize_text(links[0].text().collect::<String>()) != "Next >"
    {
        return Err(protocol(operation, world));
    }
    let href = links[0].value().attr("href").ok_or_else(|| protocol(operation, world))?;
    if !valid_next_page_href(href, world, current_page.saturating_add(1), filter) {
        return Err(protocol(operation, world));
    }
    Ok(true)
}

pub(crate) fn valid_next_page_href(
    raw: &str,
    world: MoogleMailWorld,
    expected_page: usize,
    filter: &str,
) -> bool {
    if raw.is_empty() || raw != raw.trim() || raw.chars().any(char::is_control) {
        return false;
    }
    let base = match Url::parse("https://hentaiverse.org/") {
        Ok(base) => base,
        Err(diagnostic_cause) => {
            safe_diagnostics::error(
                "mooglemail_read",
                "build_pagination_origin",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            return false;
        }
    };
    let parsed = if raw.starts_with('/') {
        if raw.starts_with("//") {
            return false;
        }
        match base.join(raw) {
            Ok(parsed) => parsed,
            Err(diagnostic_cause) => {
                safe_diagnostics::debug(
                    "mooglemail_read",
                    "resolve_relative_page_link",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                return false;
            }
        }
    } else {
        // 当前 MoogleMail 会输出规范同源绝对分页 URL；只兼容精确 HTTPS origin，
        // 不接受协议相对、用户信息、显式端口或其它 host。
        if !raw.starts_with("https://hentaiverse.org/") {
            return false;
        }
        match Url::parse(raw) {
            Ok(parsed) => parsed,
            Err(diagnostic_cause) => {
                safe_diagnostics::debug(
                    "mooglemail_read",
                    "parse_absolute_page_link",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                return false;
            }
        }
    };
    if parsed.scheme() != "https"
        || parsed.host_str() != Some("hentaiverse.org")
        || parsed.port().is_some()
        || parsed.username() != ""
        || parsed.password().is_some()
        || parsed.fragment().is_some()
        || parsed.path() != world.path()
    {
        return false;
    }
    // 闭集字段按槽位借用，重复或未知字段立即拒绝，不复制整份 query 到集合。
    let mut values = [const { None }; 4];
    for (key, value) in parsed.query_pairs() {
        let index = match key.as_ref() {
            "s" => 0,
            "ss" => 1,
            "filter" => 2,
            "page" => 3,
            _ => return false,
        };
        if values[index].replace(value).is_some() {
            return false;
        }
    }
    values[0].as_deref() == Some("Bazaar")
        && values[1].as_deref() == Some("mm")
        && values[2].as_deref().is_some_and(|value| match filter {
            "inbox" => matches!(value, "inbox" | "Inbox"),
            "sent" => matches!(value, "sent" | "Sent Mail"),
            "read" => value == "read",
            _ => false,
        })
        && values[3].as_deref().and_then(|value| value.parse::<usize>().ok()) == Some(expected_page)
}
