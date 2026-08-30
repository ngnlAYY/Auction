use super::*;

pub(crate) fn valid_inbox_header(
    row: ElementRef<'_>,
    world: MoogleMailWorld,
) -> Result<bool, MoogleMailError> {
    valid_list_header(row, world, MoogleMailOperation::Inbox, ["From", "Subject", "Sent", "Read"])
}

pub(crate) fn valid_sent_header(
    row: ElementRef<'_>,
    world: MoogleMailWorld,
) -> Result<bool, MoogleMailError> {
    valid_list_header(row, world, MoogleMailOperation::Sent, ["To", "Subject", "Sent", "Read"])
}

fn valid_list_header(
    row: ElementRef<'_>,
    _world: MoogleMailWorld,
    _operation: MoogleMailOperation,
    expected: [&str; 4],
) -> Result<bool, MoogleMailError> {
    // 只接收表头行的直接 th，避免嵌套 table 提供一组伪造列名。
    let headers = direct_children(row, "th");
    Ok(headers.len() == expected.len()
        && headers
            .iter()
            .zip(expected)
            .all(|(header, expected)| header_label(*header).as_deref() == Some(expected)))
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
    let pagers = document.select(&pager_selector).collect::<Vec<_>>();
    if pagers.len() != 1 {
        return Err(protocol(operation, world));
    }
    let controls = direct_children(pagers[0], "div");
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
        Err(_) => return false,
    };
    let parsed = if raw.starts_with('/') {
        if raw.starts_with("//") {
            return false;
        }
        match base.join(raw) {
            Ok(parsed) => parsed,
            Err(_) => return false,
        }
    } else {
        // 当前 MoogleMail 会输出规范同源绝对分页 URL；只兼容精确 HTTPS origin，
        // 不接受协议相对、用户信息、显式端口或其它 host。
        if !raw.starts_with("https://hentaiverse.org/") {
            return false;
        }
        match Url::parse(raw) {
            Ok(parsed) => parsed,
            Err(_) => return false,
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
    let pairs = parsed.query_pairs().collect::<Vec<_>>();
    if pairs.len() != 4 {
        return false;
    }
    let mut values = HashMap::with_capacity(4);
    for (key, value) in pairs {
        if values.insert(key.into_owned(), value.into_owned()).is_some() {
            return false;
        }
    }
    values.get("s").map(String::as_str) == Some("Bazaar")
        && values.get("ss").map(String::as_str) == Some("mm")
        && values.get("filter").map(String::as_str) == Some(filter)
        && values.get("page").and_then(|value| value.parse::<usize>().ok()) == Some(expected_page)
}
