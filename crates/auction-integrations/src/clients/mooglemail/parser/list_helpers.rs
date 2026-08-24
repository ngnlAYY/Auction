use super::*;

pub(crate) fn valid_inbox_header(
    row: ElementRef<'_>,
    world: MoogleMailWorld,
) -> Result<bool, MoogleMailError> {
    let selector = selector("th", MoogleMailOperation::Inbox, world)?;
    let headers = row.select(&selector).collect::<Vec<_>>();
    let expected = ["From", "Subject", "Sent", "Read"];
    Ok(headers.len() == expected.len()
        && headers.iter().zip(expected).all(|(header, expected)| {
            normalize_text(header.text().collect::<String>()) == expected
        }))
}

pub(crate) fn valid_sent_header(
    row: ElementRef<'_>,
    world: MoogleMailWorld,
) -> Result<bool, MoogleMailError> {
    let selector = selector("th", MoogleMailOperation::Sent, world)?;
    let headers = row.select(&selector).collect::<Vec<_>>();
    let expected = ["To", "Subject", "Sent", "Read"];
    Ok(headers.len() == expected.len()
        && headers.iter().zip(expected).all(|(header, expected)| {
            normalize_text(header.text().collect::<String>()) == expected
        }))
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
    if raw.is_empty()
        || raw != raw.trim()
        || raw.chars().any(char::is_control)
        || !raw.starts_with('/')
    {
        return false;
    }
    if raw.starts_with("//") {
        return false;
    }
    let base = match Url::parse("https://hentaiverse.org/") {
        Ok(base) => base,
        Err(_) => return false,
    };
    let parsed = match base.join(raw) {
        Ok(parsed) => parsed,
        Err(_) => return false,
    };
    if parsed.username() != ""
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
