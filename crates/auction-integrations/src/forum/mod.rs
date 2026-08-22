//! 论坛拍卖预览解析适配器。
//!
//! 该模块只消费 Admin 提交的 HTML/文本快照并返回内存中的安全预览，不连接论坛、不写本地
//! 状态，也不把解析成功误报为远程发布成功。语法和金额边界沿用 Auction-BF 论坛契约。

use std::collections::HashSet;

use auction_application::{
    AdminForumBidPost, AdminForumCatalog, AdminForumCommand, AdminForumLot, AdminForumPost,
    AdminForumPreviewCommand, AdminForumPreviewError, AdminForumPreviewPort,
    AdminForumPreviewResult, AdminForumWarning, ApplicationFuture,
};
use scraper::{ElementRef, Html, Selector};
use url::Url;

const DEFAULT_START_PRICE: i64 = 50_000;
const CREDIT_GRID: i64 = 1_000;
const CATALOG_CATEGORIES: [&str; 7] =
    ["One-Handed", "Two-Handed", "Staff", "Shield", "Cloth", "Light", "Heavy"];

type CatalogData = (Vec<String>, Vec<AdminForumLot>);
type CatalogParse = (CatalogData, Vec<AdminForumWarning>);

/// 无状态的论坛预览解析器。
#[derive(Debug, Default, Clone, Copy)]
pub struct ForumPreviewAdapter;

impl ForumPreviewAdapter {
    /// 建立不持有网络或数据库状态的解析器。
    pub const fn new() -> Self {
        Self
    }
}

impl AdminForumPreviewPort for ForumPreviewAdapter {
    fn preview(
        &self,
        command: AdminForumPreviewCommand,
    ) -> ApplicationFuture<'_, AdminForumPreviewResult> {
        Box::pin(async move {
            command.validate()?;
            let result = if let Some(html) = command.html {
                parse_html(&html).map_err(|_| AdminForumPreviewError::ParseFailed)?
            } else if let Some(text) = command.text {
                parse_thread_text(&text).map_err(|_| AdminForumPreviewError::ParseFailed)?
            } else {
                return Err(AdminForumPreviewError::InvalidInput.into());
            };
            result.validate()?;
            Ok(result)
        })
    }
}

#[derive(Debug, Clone)]
struct ParsedPost {
    id: String,
    author_id: Option<String>,
    author: Option<String>,
    text: String,
    edited: bool,
}

fn parse_html(input: &str) -> Result<AdminForumPreviewResult, ()> {
    let document = Html::parse_document(input);
    let selector = Selector::parse("div.postcolor").map_err(|_| ())?;
    let mut posts = Vec::new();
    for element in document.select(&selector) {
        let Some(raw_id) = element.attr("id") else {
            continue;
        };
        let Some(id) = raw_id.strip_prefix("post-").filter(|value| !value.is_empty()) else {
            continue;
        };
        let text = render_post_text(element);
        let (author_id, author) = extract_author(element);
        posts.push(ParsedPost {
            id: id.to_owned(),
            author_id,
            author,
            edited: text.to_ascii_lowercase().contains("this post has been edited by"),
            text,
        });
    }
    parse_thread(posts)
}

fn parse_thread_text(input: &str) -> Result<AdminForumPreviewResult, ()> {
    parse_thread(split_posts(input))
}

fn parse_thread(posts: Vec<ParsedPost>) -> Result<AdminForumPreviewResult, ()> {
    let Some(catalog_post) = posts.first() else {
        return Err(());
    };
    let (catalog, catalog_warnings) = parse_catalog(&catalog_post.text)?;
    let mut result = AdminForumPreviewResult {
        catalog: AdminForumCatalog {
            categories: catalog.0,
            lots: catalog.1,
            warnings: catalog_warnings.clone(),
        },
        posts: Vec::with_capacity(posts.len().saturating_sub(1)),
        warnings: catalog_warnings,
    };
    for post in posts.into_iter().skip(1) {
        if is_audit_post(&post.text) {
            result.posts.push(AdminForumBidPost {
                post: map_post(post),
                commands: Vec::new(),
                warnings: Vec::new(),
                ignored: true,
            });
            continue;
        }
        let parsed = parse_bid_post(post, &result.catalog.lots);
        result.warnings.extend(parsed.1.iter().cloned());
        result.posts.push(parsed.0);
    }
    sort_warnings(&mut result.warnings);
    Ok(result)
}

fn parse_catalog(input: &str) -> Result<CatalogParse, ()> {
    let mut categories = Vec::new();
    let mut category_set = HashSet::new();
    let mut lots = Vec::new();
    let mut lot_set = HashSet::new();
    let mut warnings = Vec::new();
    let mut category = None::<String>;

    for (line_index, line) in normalized_lines(input).into_iter().enumerate() {
        let line_number = line_index + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(name) = category_heading(trimmed) {
            if category_set.insert(name.to_owned()) {
                categories.push(name.to_owned());
            }
            category = Some(name.to_owned());
            continue;
        }
        let Some((raw_code, description)) = catalog_row(trimmed) else {
            continue;
        };
        let Some(code) = normalize_code(raw_code) else {
            warnings.push(warning(
                line_number,
                "invalid_catalog_row",
                "catalog row has no valid category or code",
                Some(trimmed),
                None,
            ));
            continue;
        };
        let Some(category) = category.as_deref() else {
            warnings.push(warning(
                line_number,
                "invalid_catalog_row",
                "catalog row has no valid category or code",
                Some(trimmed),
                None,
            ));
            continue;
        };
        if !lot_set.insert(code.clone()) {
            return Err(());
        }
        let start_price = match custom_start_price(description) {
            Some(Ok(value)) => value,
            Some(Err(())) => {
                warnings.push(warning(
                    line_number,
                    "invalid_start_price",
                    "invalid custom start price; default used",
                    Some(trimmed),
                    None,
                ));
                DEFAULT_START_PRICE
            }
            None => DEFAULT_START_PRICE,
        };
        let (seller, current_bidder, current_price) = seller_and_bid(description);
        lots.push(AdminForumLot {
            code,
            category: category.to_owned(),
            description: description.to_owned(),
            seller,
            start_price,
            material: is_material(description),
            current_bidder,
            current_price,
        });
    }
    if lots.is_empty() {
        return Err(());
    }
    Ok(((categories, lots), warnings))
}

fn parse_bid_post(
    post: ParsedPost,
    lots: &[AdminForumLot],
) -> (AdminForumBidPost, Vec<AdminForumWarning>) {
    let post_id = post.id.clone();
    if post.edited {
        let warning = warning(
            0,
            "edited_post_ignored",
            "edited posts cannot create bids or commands",
            None,
            Some(post_id.clone()),
        );
        return (
            AdminForumBidPost {
                post: map_post(post),
                commands: Vec::new(),
                warnings: vec![warning.clone()],
                ignored: true,
            },
            vec![warning],
        );
    }

    let mut commands = Vec::new();
    let mut warnings = Vec::new();
    let actor = post
        .author_id
        .clone()
        .or_else(|| post.author.clone())
        .filter(|value| !value.trim().is_empty());
    for (line_index, line) in normalized_lines(&post.text).into_iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match parse_command_line(trimmed, line_index + 1, lots, actor.as_deref()) {
            Ok(command) => commands.push(command),
            Err((code, message)) => warnings.push(warning(
                line_index + 1,
                code,
                message,
                Some(trimmed),
                Some(post_id.clone()),
            )),
        }
    }
    let result = AdminForumBidPost {
        post: map_post(post),
        commands,
        warnings: warnings.clone(),
        ignored: false,
    };
    (result, warnings)
}

fn parse_command_line(
    line: &str,
    line_number: usize,
    lots: &[AdminForumLot],
    actor: Option<&str>,
) -> Result<AdminForumCommand, (&'static str, &'static str)> {
    let tokens = line.split_whitespace().collect::<Vec<_>>();
    if tokens.is_empty() {
        return Err(("invalid_command", "a command must start with one or more lot codes"));
    }
    let mut codes = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let Some(code) = normalize_code(tokens[index]) else {
            break;
        };
        if !lots.iter().any(|lot| lot.code == code) {
            return Err(("unknown_lot", "lot code is not present in the catalog"));
        }
        codes.push(code);
        index += 1;
    }
    if codes.is_empty() {
        return Err(("invalid_command", "a command must start with one or more lot codes"));
    }
    if index == tokens.len() {
        return Err((
            "missing_action",
            "a lot code must be immediately followed by a bid or command",
        ));
    }

    let action = tokens[index].to_ascii_lowercase();
    index += 1;
    let mut command = AdminForumCommand {
        line: line_number,
        text: line.to_owned(),
        actor: actor.map(str::to_owned),
        lot_codes: codes,
        kind: String::new(),
        amount: None,
        proxy: None,
        proxy_lower: false,
        notify: None,
    };
    match action.as_str() {
        "start" => command.kind = "start".to_owned(),
        "cancel" => command.kind = "cancel".to_owned(),
        "proxy" => {
            if index == tokens.len() {
                return Err(("invalid_proxy", "proxy must be followed by a credits maximum"));
            }
            if tokens[index].eq_ignore_ascii_case("lower") {
                command.proxy_lower = true;
                index += 1;
                if index == tokens.len() {
                    return Err((
                        "invalid_proxy",
                        "proxy lower must be followed by a credits maximum",
                    ));
                }
            }
            command.amount =
                Some(parse_amount(tokens[index]).map_err(|_| {
                    ("invalid_proxy", "proxy maximum is not a valid credits amount")
                })?);
            index += 1;
            command.kind = "proxy".to_owned();
            command.proxy = Some("fast".to_owned());
            if !command.proxy_lower
                && index < tokens.len()
                && (tokens[index].eq_ignore_ascii_case("fast")
                    || tokens[index].eq_ignore_ascii_case("slow"))
            {
                command.proxy = Some(tokens[index].to_ascii_lowercase());
                index += 1;
            }
        }
        "notify" => {
            command.kind = "notify".to_owned();
            command.notify = Some(parse_notify_mode(&tokens, index)?);
            index = tokens.len();
        }
        _ => {
            command.kind = "bid".to_owned();
            command.amount = Some(parse_amount(&action).map_err(|_| {
                ("invalid_action", "the action is not start, cancel, notify, or a credits amount")
            })?);
        }
    }
    if command.kind != "notify"
        && index < tokens.len()
        && tokens[index].eq_ignore_ascii_case("notify")
    {
        index += 1;
        command.notify = Some(parse_notify_mode(&tokens, index)?);
        index = tokens.len();
    }
    if index != tokens.len() {
        return Err(("extra_words", "other words are not allowed in a forum bid line"));
    }
    Ok(command)
}

fn parse_notify_mode(
    tokens: &[&str],
    index: usize,
) -> Result<String, (&'static str, &'static str)> {
    if index == tokens.len() {
        return Ok("both".to_owned());
    }
    match tokens[index].to_ascii_lowercase().as_str() {
        "pm" if index + 1 == tokens.len() => Ok("pm".to_owned()),
        "mm" if index + 1 == tokens.len() => Ok("mm".to_owned()),
        _ => Err(("invalid_notify", "notify must optionally be followed by pm or mm")),
    }
}

fn split_posts(input: &str) -> Vec<ParsedPost> {
    let mut posts = Vec::new();
    let mut current = None::<ParsedPost>;
    for line in normalized_lines(input) {
        if let Some((id, author)) = post_separator(&line) {
            if let Some(mut post) = current.take() {
                post.text = post.text.trim().to_owned();
                post.edited =
                    post.text.to_ascii_lowercase().contains("this post has been edited by");
                posts.push(post);
            }
            current = Some(ParsedPost {
                id,
                author_id: None,
                author,
                text: String::new(),
                edited: false,
            });
            continue;
        }
        let Some(post) = current.as_mut() else {
            continue;
        };
        if post.text.is_empty() && line.trim_start().starts_with("Author:") {
            post.author = Some(line.trim_start()["Author:".len()..].trim().to_owned());
        } else {
            post.text.push_str(&line);
            post.text.push('\n');
        }
    }
    if let Some(mut post) = current {
        post.text = post.text.trim().to_owned();
        post.edited = post.text.to_ascii_lowercase().contains("this post has been edited by");
        posts.push(post);
    }
    posts
}

fn post_separator(line: &str) -> Option<(String, Option<String>)> {
    let trimmed = line.trim();
    let inner = trimmed.strip_prefix("=====")?.strip_suffix("=====")?.trim();
    let mut fields = inner.split_whitespace();
    let id = fields.next()?.strip_prefix("post-")?.to_owned();
    if id.is_empty() {
        return None;
    }
    let author = fields
        .find_map(|field| field.strip_prefix("author=").map(str::to_owned))
        .filter(|value| !value.is_empty());
    Some((id, author))
}

fn catalog_row(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix('[')?;
    let end = rest.find(']')?;
    let code = &rest[..end];
    Some((code, rest[end + 1..].trim()))
}

fn category_heading(value: &str) -> Option<&'static str> {
    CATALOG_CATEGORIES.iter().copied().find(|category| *category == value)
}

fn normalize_code(raw: &str) -> Option<String> {
    let value = raw
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .replace('-', "")
        .to_ascii_uppercase();
    if value.len() != 5
        || !value.is_ascii()
        || !value.as_bytes()[..3].iter().all(u8::is_ascii_uppercase)
        || !value.as_bytes()[3..].iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    Some(format!("{}{}{}", &value[0..1], value[1..3].to_ascii_lowercase(), &value[3..]))
}

fn custom_start_price(description: &str) -> Option<Result<i64, ()>> {
    let lower = description.to_ascii_lowercase();
    let start = lower.find("start")?;
    let tail = lower[start + "start".len()..].trim_start();
    let tail = tail.strip_prefix(':')?.trim_start();
    let token = tail
        .split_whitespace()
        .next()?
        .trim_matches(|character: char| matches!(character, ',' | ';' | ')' | ']'));
    Some(parse_amount(token))
}

fn seller_and_bid(description: &str) -> (Option<String>, Option<String>, Option<i64>) {
    let lower = description.to_ascii_lowercase();
    let Some(start) = lower.find("(seller:") else {
        return (None, None, None);
    };
    let content_start = start + "(seller:".len();
    let Some(end_offset) = description[content_start..].find(')') else {
        return (None, None, None);
    };
    let end = content_start + end_offset;
    let seller = description[content_start..end].trim().to_owned();
    let tail = description[end + 1..].trim();
    let fields = tail.split_whitespace().collect::<Vec<_>>();
    let mut bidder = None;
    let mut price = None;
    for index in 0..fields.len() {
        if let Ok(value) = parse_amount(fields[index]) {
            if index > 0 {
                bidder = Some(fields[..index].join(" "));
            }
            price = Some(value);
            break;
        }
    }
    ((!seller.is_empty()).then_some(seller), bidder, price)
}

fn is_material(description: &str) -> bool {
    let lower = description.to_ascii_lowercase();
    lower.contains("[mat]")
        || lower
            .split(|character: char| !character.is_ascii_alphanumeric())
            .any(|word| word == "mat" || word == "material")
}

fn parse_amount(raw: &str) -> Result<i64, ()> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(());
    }
    let (number, multiplier) = match raw.as_bytes().last().copied() {
        Some(b'k' | b'K') => (&raw[..raw.len() - 1], 1_000_i128),
        Some(b'm' | b'M') => (&raw[..raw.len() - 1], 1_000_000_i128),
        _ => (raw, 1_i128),
    };
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    if whole.is_empty()
        || (number.contains('.') && fraction.is_empty())
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        || fraction.len() > 6
    {
        return Err(());
    }
    let denominator = 10_i128.checked_pow(fraction.len() as u32).ok_or(())?;
    let whole = whole.parse::<i128>().map_err(|_| ())?;
    let fractional =
        if fraction.is_empty() { 0 } else { fraction.parse::<i128>().map_err(|_| ())? };
    let scaled =
        whole.checked_mul(denominator).and_then(|value| value.checked_add(fractional)).ok_or(())?;
    let total = scaled.checked_mul(multiplier).ok_or(())?;
    if total <= 0 || total % denominator != 0 {
        return Err(());
    }
    let amount = total / denominator;
    if amount % i128::from(CREDIT_GRID) != 0 {
        return Err(());
    }
    i64::try_from(amount).map_err(|_| ())
}

fn render_post_text(element: ElementRef<'_>) -> String {
    let mut source = element.inner_html();
    for tag in ["<br>", "<br/>", "<br />", "</p>", "</li>", "</blockquote>", "</div>"] {
        source = source.replace(tag, "\n");
    }
    let fragment = Html::parse_fragment(&source);
    normalize_text(&fragment.root_element().text().collect::<String>())
}

fn normalize_text(value: &str) -> String {
    normalized_lines(value)
        .into_iter()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn extract_author(element: ElementRef<'_>) -> (Option<String>, Option<String>) {
    let selector =
        Selector::parse(".normalname, .author, .username, .post-author, .member_name").ok();
    let mut current = Some(element);
    for _ in 0..8 {
        let Some(node) = current else {
            break;
        };
        let author_id = ["data-author-id", "data-user-id", "data-player-id", "data-game-player-id"]
            .iter()
            .find_map(|key| {
                node.attr(key).map(str::trim).filter(|value| !value.is_empty()).map(str::to_owned)
            });
        let author_element = selector.as_ref().and_then(|selector| node.select(selector).next());
        let author = author_element.map(render_post_text).filter(|value| !value.is_empty());
        let author_id = author_id.or_else(|| {
            author_element.and_then(|value| {
                let links = Selector::parse("a").ok()?;
                value.select(&links).find_map(|link| {
                    let href = link.attr("href")?;
                    let url = Url::parse(href)
                        .or_else(|_| Url::parse(&format!("https://invalid.local{href}")))
                        .ok()?;
                    ["game_player_id", "player_id"].iter().find_map(|key| {
                        url.query_pairs()
                            .find(|(name, _)| name == key)
                            .map(|(_, value)| value.into_owned())
                    })
                })
            })
        });
        if author_id.is_some() || author.is_some() {
            return (author_id, author);
        }
        current = node.parent().and_then(ElementRef::wrap);
    }
    (None, None)
}

fn map_post(post: ParsedPost) -> AdminForumPost {
    AdminForumPost {
        id: post.id,
        author_id: post.author_id,
        author: post.author,
        text: post.text,
        edited: post.edited,
    }
}

fn is_audit_post(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.contains("warning log:") || value.contains("manually ignored posts")
}

fn normalized_lines(value: &str) -> Vec<String> {
    value.replace("\r\n", "\n").replace('\r', "\n").split('\n').map(str::to_owned).collect()
}

fn warning(
    line: usize,
    code: &str,
    message: &str,
    text: Option<&str>,
    post_id: Option<String>,
) -> AdminForumWarning {
    AdminForumWarning {
        line,
        code: code.to_owned(),
        message: message.to_owned(),
        text: text.map(str::to_owned),
        post_id,
    }
}

fn sort_warnings(warnings: &mut [AdminForumWarning]) {
    warnings.sort_by(|left, right| {
        left.post_id.cmp(&right.post_id).then_with(|| left.line.cmp(&right.line))
    });
}
