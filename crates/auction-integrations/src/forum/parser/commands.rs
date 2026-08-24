//! 论坛竞价命令、通知选项和金额解析。

use auction_application::{AdminForumBidPost, AdminForumCommand, AdminForumLot, AdminForumWarning};

use super::CREDIT_GRID;
use super::model::ParsedPost;

pub(super) fn parse_bid_post(
    post: ParsedPost,
    lots: &[AdminForumLot],
) -> (AdminForumBidPost, Vec<AdminForumWarning>) {
    let post_id = post.id.clone();
    if post.edited {
        let warning = super::helpers::warning(
            0,
            "edited_post_ignored",
            "edited posts cannot create bids or commands",
            None,
            Some(post_id.clone()),
        );
        return (
            AdminForumBidPost {
                post: super::helpers::map_post(post),
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
    for (line_index, line) in super::text::normalized_lines(&post.text).into_iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match parse_command_line(trimmed, line_index + 1, lots, actor.as_deref()) {
            Ok(command) => commands.push(command),
            Err((code, message)) => warnings.push(super::helpers::warning(
                line_index + 1,
                code,
                message,
                Some(trimmed),
                Some(post_id.clone()),
            )),
        }
    }
    let result = AdminForumBidPost {
        post: super::helpers::map_post(post),
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

pub(super) fn parse_amount(raw: &str) -> Result<i64, ()> {
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
