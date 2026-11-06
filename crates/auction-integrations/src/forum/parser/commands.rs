//! 论坛竞价命令、通知选项和金额解析。

use std::collections::HashSet;

use auction_application::admin::{AdminForumBidPost, AdminForumCommand, AdminForumWarning};

use super::CREDIT_GRID;
use super::model::ParsedPost;

pub(super) fn parse_bid_post(
    post: ParsedPost,
    lot_codes: &HashSet<&str>,
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
    for (line_index, line) in super::text::normalized_lines(&post.text).enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match parse_command_line(trimmed, line_index + 1, lot_codes, actor.as_deref()) {
            Ok(Some(command)) => commands.push(command),
            // 论坛只接受普通出价；代理命令属于 Web 专属能力，且按产品约定不写入系统更新楼。
            Ok(None) => {}
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
    lot_codes: &HashSet<&str>,
    actor: Option<&str>,
) -> Result<Option<AdminForumCommand>, (&'static str, &'static str)> {
    let mut tokens = line.split_whitespace().peekable();
    let mut codes = Vec::new();
    let mut unknown_lot = false;
    while let Some(raw) = tokens.peek().copied() {
        let Some(code) = super::helpers::normalize_lot_code(raw) else {
            break;
        };
        if !lot_codes.contains(code.as_str()) {
            unknown_lot = true;
        }
        codes.push(code);
        tokens.next();
    }
    if codes.is_empty() {
        return Err(("invalid_command", "a command must start with one or more lot codes"));
    }
    let action = tokens
        .next()
        .ok_or(("missing_action", "a lot code must be immediately followed by a bid or command"))?
        .to_ascii_lowercase();
    if action == "proxy" {
        return Ok(None);
    }
    if unknown_lot {
        return Err(("unknown_lot", "lot code is not present in the catalog"));
    }
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
        "notify" => {
            command.kind = "notify".to_owned();
            command.notify = Some(parse_notify_mode(&mut tokens)?);
        }
        _ => {
            command.kind = "bid".to_owned();
            command.amount = Some(parse_amount(&action).map_err(|_| {
                ("invalid_action", "the action is not start, cancel, notify, or a credits amount")
            })?);
        }
    }
    if command.kind != "notify"
        && tokens.peek().is_some_and(|token| token.eq_ignore_ascii_case("notify"))
    {
        tokens.next();
        command.notify = Some(parse_notify_mode(&mut tokens)?);
    }
    if tokens.next().is_some() {
        return Err(("extra_words", "other words are not allowed in a forum bid line"));
    }
    Ok(Some(command))
}

fn parse_notify_mode<'a>(
    tokens: &mut impl Iterator<Item = &'a str>,
) -> Result<String, (&'static str, &'static str)> {
    let Some(mode) = tokens.next() else {
        return Ok("both".to_owned());
    };
    match mode.to_ascii_lowercase().as_str() {
        "pm" if tokens.next().is_none() => Ok("pm".to_owned()),
        "mm" if tokens.next().is_none() => Ok("mm".to_owned()),
        _ => Err(("invalid_notify", "notify must optionally be followed by pm or mm")),
    }
}

pub(super) fn parse_amount(raw: &str) -> Result<i64, ()> {
    parse_positive_amount(raw, true)
}

pub(super) fn parse_catalog_amount(raw: &str) -> Result<i64, ()> {
    parse_positive_amount(raw, false)
}

fn parse_positive_amount(raw: &str, require_credit_grid: bool) -> Result<i64, ()> {
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
    if require_credit_grid && amount % i128::from(CREDIT_GRID) != 0 {
        return Err(());
    }
    i64::try_from(amount).map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forum_proxy_lines_are_silently_ignored() {
        let lot_codes = HashSet::from(["One01"]);
        let post = ParsedPost {
            id: "6956801".to_owned(),
            author_id: Some("5501029".to_owned()),
            author: Some("ngnlAYY".to_owned()),
            text: "One01 proxy 2m fast\nOne01 proxy lower 1.5m\nBad99 proxy 3m\nOne01 200k"
                .to_owned(),
            edited: false,
        };

        let (parsed, warnings) = parse_bid_post(post, &lot_codes);

        assert!(warnings.is_empty());
        assert_eq!(parsed.commands.len(), 1);
        assert_eq!(parsed.commands[0].kind, "bid");
        assert_eq!(parsed.commands[0].amount, Some(200_000));
    }
}
