use std::{collections::HashSet, fmt, str};

use scraper::{ElementRef, Html, Selector};

use super::{
    MoogleMailError, MoogleMailErrorCategory, MoogleMailMessageFields, MoogleMailOperation,
    MoogleMailParser, MoogleMailToken, MoogleMailWorld,
    protocol::{valid_decimal_id, valid_line},
};

const MAX_COMPOSE_ATTACHMENTS: usize = 1000;
const MAX_RECIPIENT_BYTES: usize = 256;
const MAX_SUBJECT_BYTES: usize = 512;
const MAX_BODY_BYTES: usize = 64 * 1024;

/// 经过结构校验的 compose 页面状态。
///
/// token 只通过引用提供给下一步 typed form 构造；正文、收件人和附件 key 不进入 Debug 输出。
#[derive(Clone, PartialEq, Eq)]
pub struct MoogleMailComposeState {
    token: MoogleMailToken,
    recipient: String,
    subject: String,
    body: String,
    equipment_ids: Vec<String>,
    credits: i64,
    cod: Option<i64>,
    attachment_count: usize,
}

impl MoogleMailComposeState {
    /// 返回当前页面 token 的受控引用。
    pub fn token(&self) -> &MoogleMailToken {
        &self.token
    }

    /// 返回当前页面中的消息字段副本，供下一次 POST 重放。
    pub fn message_fields(&self) -> MoogleMailMessageFields {
        MoogleMailMessageFields {
            recipient: self.recipient.clone(),
            subject: self.subject.clone(),
            body: self.body.clone(),
        }
    }

    /// 返回已证明的装备 ID，顺序保持页面顺序。
    pub fn equipment_ids(&self) -> &[String] {
        &self.equipment_ids
    }

    /// 返回已证明的 Credits 附件金额。
    pub const fn credits(&self) -> i64 {
        self.credits
    }

    /// 返回 CoD；页面未提供可证明的 CoD 状态时返回 None。
    pub const fn cod(&self) -> Option<i64> {
        self.cod
    }

    /// 返回页面报告的附件条目数。
    pub const fn attachment_count(&self) -> usize {
        self.attachment_count
    }

    /// 判断 compose 是否已回到可安全复用的干净状态。
    pub fn is_clean(&self) -> bool {
        self.recipient.is_empty()
            && self.subject.is_empty()
            && self.body.is_empty()
            && self.equipment_ids.is_empty()
            && self.credits == 0
            && self.attachment_count == 0
            && self.cod == Some(0)
    }
}

impl fmt::Debug for MoogleMailComposeState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MoogleMailComposeState")
            .field("recipient", &"[REDACTED]")
            .field("subject", &"[REDACTED]")
            .field("body", &"[REDACTED]")
            .field("equipment_count", &self.equipment_ids.len())
            .field("credits", &self.credits)
            .field("cod", &self.cod)
            .field("attachment_count", &self.attachment_count)
            .finish()
    }
}

impl MoogleMailParser {
    /// 严格解析新邮件页面，提取下一步 POST 所需的 token 和可证明状态。
    pub fn parse_compose_state(
        &self,
        document: &[u8],
        world: MoogleMailWorld,
    ) -> Result<MoogleMailComposeState, MoogleMailError> {
        let _ = self;
        parse_compose_state(document, world)
    }

    /// 证明一次附加或设置 CoD POST 后页面状态与预期完全一致。
    pub fn prove_staged_compose(
        &self,
        document: &[u8],
        world: MoogleMailWorld,
        message: &MoogleMailMessageFields,
        expected_credits: i64,
        expected_equipment_ids: &[String],
        expected_cod: i64,
    ) -> Result<MoogleMailComposeState, MoogleMailError> {
        let _ = self;
        if !message.validate()
            || expected_credits < 0
            || expected_cod < 0
            || expected_equipment_ids.iter().any(|id| !valid_decimal_id(id))
        {
            return Err(compose_error(world, MoogleMailErrorCategory::Configuration));
        }
        let state = parse_compose_state(document, world)?;
        let mut actual_equipment_ids = state.equipment_ids.clone();
        let mut expected_equipment_ids = expected_equipment_ids.to_vec();
        actual_equipment_ids.sort_unstable();
        expected_equipment_ids.sort_unstable();
        if actual_equipment_ids.windows(2).any(|pair| pair[0] == pair[1])
            || expected_equipment_ids.windows(2).any(|pair| pair[0] == pair[1])
            || state.recipient != message.recipient
            || state.subject != message.subject
            || canonical_body(&state.body) != canonical_body(&message.body)
            || state.credits != expected_credits
            || actual_equipment_ids != expected_equipment_ids
            || state.cod != Some(expected_cod)
        {
            return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
        Ok(state)
    }

    /// 证明最终发送响应成功，并尽可能提取远端邮件 ID。
    ///
    /// 返回 None 只表示成功页面没有暴露唯一 mid，不表示发送失败；调用方仍应保存整个
    /// outbox 的 request digest 并进入后续 sent/reconciliation 流程。
    pub fn prove_send_completed(
        &self,
        document: &[u8],
        world: MoogleMailWorld,
    ) -> Result<Option<String>, MoogleMailError> {
        let source = str::from_utf8(document)
            .map_err(|_| compose_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
        if source.trim().is_empty() {
            return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
        let parsed = Html::parse_document(source);
        let confirmation_selector = selector("#mmail_fin", world)?;
        let form_selector = selector("form#mailform", world)?;
        let confirmations = parsed.select(&confirmation_selector).collect::<Vec<_>>();
        let forms = parsed.select(&form_selector).count();
        if confirmations.len() > 1 {
            return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
        if confirmations.len() == 1 && forms == 0 {
            let phrase = visible_confirmation_text(confirmations[0], world)?;
            if !phrase.to_ascii_lowercase().contains("message has been sent") {
                return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
            }
            return Ok(response_mail_id(source));
        }

        let state = parse_compose_state(document, world)?;
        if !state.is_clean() {
            return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
        Ok(response_mail_id(source))
    }
}

fn parse_compose_state(
    document: &[u8],
    world: MoogleMailWorld,
) -> Result<MoogleMailComposeState, MoogleMailError> {
    let source = str::from_utf8(document)
        .map_err(|_| compose_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
    if source.trim().is_empty() || source.trim() == "state lock limiter in effect" {
        return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    let parsed = Html::parse_document(source);
    if has_visible_result_message(&parsed, world)? {
        return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    let form_selector = selector("form#mailform", world)?;
    let forms = parsed.select(&form_selector).collect::<Vec<_>>();
    if forms.len() != 1 {
        return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    let form = forms[0];
    let token_selector = selector("input[name='mmtoken']", world)?;
    let tokens = form.select(&token_selector).collect::<Vec<_>>();
    if tokens.len() != 1 {
        return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    let token_value = tokens[0]
        .value()
        .attr("value")
        .ok_or_else(|| compose_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
    let token = MoogleMailToken::new(token_value)
        .ok_or_else(|| compose_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
    let recipient = input_value(form, "message_to_name", world)?;
    let subject = input_value(form, "message_subject", world)?;
    if !valid_line(&recipient, MAX_RECIPIENT_BYTES, true)
        || !valid_line(&subject, MAX_SUBJECT_BYTES, true)
    {
        return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    let body_selector = selector("textarea[name='message_body']", world)?;
    let bodies = form.select(&body_selector).collect::<Vec<_>>();
    if bodies.len() != 1 || !valid_body(&bodies[0].text().collect::<String>()) {
        return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    let body = bodies[0].text().collect::<String>();
    let assets = parse_compose_assets(&parsed, world)?;
    Ok(MoogleMailComposeState {
        token,
        recipient,
        subject,
        body,
        equipment_ids: assets.equipment_ids,
        credits: assets.credits,
        cod: assets.cod,
        attachment_count: assets.attachment_count,
    })
}

struct ComposeAssets {
    equipment_ids: Vec<String>,
    credits: i64,
    cod: Option<i64>,
    attachment_count: usize,
}

fn parse_compose_assets(
    document: &Html,
    world: MoogleMailWorld,
) -> Result<ComposeAssets, MoogleMailError> {
    let list_selector = selector("#mmail_attachlist", world)?;
    let info_selector = selector("#mmail_attachinfo", world)?;
    let count_selector = selector("#mmail_attachcount", world)?;
    let cod_selector = selector("#newcod", world)?;
    let lists = document.select(&list_selector).collect::<Vec<_>>();
    let infos = document.select(&info_selector).count();
    let count_boxes = document.select(&count_selector).collect::<Vec<_>>();
    let cod_fields = document.select(&cod_selector).collect::<Vec<_>>();
    if lists.len() > 1 || infos > 1 || count_boxes.len() > 1 || cod_fields.len() > 1 {
        return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    if lists.is_empty() {
        if infos != 1 || !count_boxes.is_empty() || !cod_fields.is_empty() {
            return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
        return Ok(ComposeAssets {
            equipment_ids: Vec::new(),
            credits: 0,
            cod: Some(0),
            attachment_count: 0,
        });
    }

    let entries = direct_element_children(lists[0]);
    if entries.is_empty() || entries.len() > MAX_COMPOSE_ATTACHMENTS {
        return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    let list_text = normalize_text(lists[0].text().collect::<String>());
    let child_text = normalize_text(
        entries.iter().map(|entry| entry.text().collect::<String>()).collect::<String>(),
    );
    if list_text != child_text {
        return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    let hook_selector = selector("[onmouseover]", world)?;
    let mut equipment_ids = Vec::new();
    let mut seen_equipment_ids = HashSet::new();
    let mut credits = 0_i64;
    for entry in &entries {
        let mut hooks = Vec::new();
        if entry.value().attr("onmouseover").is_some() {
            hooks.push(*entry);
        }
        hooks.extend(entry.select(&hook_selector));
        let text = normalize_text(entry.text().collect::<String>());
        if hooks.is_empty() {
            let amount = parse_credit_attachment(&text)
                .ok_or_else(|| compose_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
            if credits != 0 {
                return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
            }
            credits = amount;
        } else {
            if hooks.len() != 1 || normalize_text(hooks[0].text().collect::<String>()) != text {
                return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
            }
            let script = hooks[0]
                .value()
                .attr("onmouseover")
                .ok_or_else(|| compose_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
            let equipment_id = extract_equipment_id(script)
                .ok_or_else(|| compose_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
            if !seen_equipment_ids.insert(equipment_id.clone()) {
                return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
            }
            equipment_ids.push(equipment_id);
        }
    }

    let mut cod = None;
    if let Some(count_box) = count_boxes.first() {
        let text = normalize_text(count_box.text().collect::<String>());
        let (count, count_cod) = parse_attachment_count(&text)
            .ok_or_else(|| compose_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
        if count != entries.len() {
            return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
        cod = count_cod;
    }
    if let Some(cod_field) = cod_fields.first() {
        let raw = cod_field
            .value()
            .attr("value")
            .ok_or_else(|| compose_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
        let value = parse_amount(raw)
            .ok_or_else(|| compose_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
        if cod.is_some_and(|existing| existing != value) {
            return Err(compose_error(world, MoogleMailErrorCategory::IdentityMismatch));
        }
        cod = Some(value);
    }
    if cod.is_none() && entries.is_empty() {
        cod = Some(0);
    }
    Ok(ComposeAssets { equipment_ids, credits, cod, attachment_count: entries.len() })
}

fn input_value(
    form: ElementRef<'_>,
    name: &str,
    world: MoogleMailWorld,
) -> Result<String, MoogleMailError> {
    let selector = selector(&format!("input[name='{name}']"), world)?;
    let fields = form.select(&selector).collect::<Vec<_>>();
    if fields.len() != 1 {
        return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    Ok(fields[0].value().attr("value").unwrap_or_default().to_owned())
}

fn parse_credit_attachment(value: &str) -> Option<i64> {
    let value = value.strip_suffix(" Credits")?;
    let value = value.strip_suffix('x').unwrap_or(value);
    let amount = parse_amount(value)?;
    (amount > 0).then_some(amount)
}

fn parse_attachment_count(value: &str) -> Option<(usize, Option<i64>)> {
    let (prefix, suffix) = value.split_once(" items attached")?;
    let (count, maximum) = prefix.split_once(" / ")?;
    let count = parse_unsigned(count)?;
    let maximum = parse_unsigned(maximum)?;
    if count > maximum {
        return None;
    }
    let cod = if suffix.is_empty() {
        None
    } else {
        let amount = suffix.strip_prefix(" - CoD: ").or_else(|| suffix.strip_prefix(" – CoD: "))?;
        Some(parse_amount(amount)?)
    };
    Some((usize::try_from(count).ok()?, cod))
}

fn parse_amount(value: &str) -> Option<i64> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || value
            .bytes()
            .enumerate()
            .any(|(index, byte)| byte == b',' && (index < 1 || index + 1 >= value.len()))
    {
        return None;
    }
    let groups = value.split(',').collect::<Vec<_>>();
    if groups.len() > 1
        && (groups[0].is_empty()
            || groups[0].len() > 3
            || !groups[0].bytes().all(|byte| byte.is_ascii_digit())
            || groups[0].starts_with('0')
            || groups[1..]
                .iter()
                .any(|group| group.len() != 3 || !group.bytes().all(|byte| byte.is_ascii_digit())))
    {
        return None;
    }
    if groups.len() == 1 && !groups[0].bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.replace(',', "").parse::<i64>().ok()
}

fn parse_unsigned(value: &str) -> Option<u64> {
    if value.is_empty() || (value.len() > 1 && value.starts_with('0')) {
        return None;
    }
    value.bytes().all(|byte| byte.is_ascii_digit()).then(|| value.parse().ok()).flatten()
}

fn visible_confirmation_text(
    element: ElementRef<'_>,
    world: MoogleMailWorld,
) -> Result<String, MoogleMailError> {
    let text = normalize_text(element.text().collect::<String>());
    if !text.is_empty() {
        return Ok(text);
    }
    let selector = selector("div", world)?;
    let mut decoded = String::new();
    for child in element.select(&selector) {
        let classes = child.value().attr("class").unwrap_or_default().split_whitespace();
        for class_name in classes {
            let bytes = class_name.as_bytes();
            if bytes.len() != 3 || bytes[0] != b'c' {
                continue;
            }
            match (bytes[1], bytes[2]) {
                (b'3' | b'5', b'9') => decoded.push(' '),
                (b'3' | b'5', b'a'..=b'z') => decoded.push(char::from(bytes[2])),
                (b'2' | b'4', b'0'..=b'9') => decoded.push(char::from(bytes[2])),
                _ => {}
            }
        }
    }
    Ok(normalize_text(decoded))
}

fn response_mail_id(source: &str) -> Option<String> {
    let bytes = source.as_bytes();
    let mut offset = 0;
    let mut found = None;
    while let Some(relative) = source[offset..].find("mid=") {
        let start = offset + relative;
        let previous = start.checked_sub(1).and_then(|index| bytes.get(index).copied());
        if !matches!(previous, Some(b'?' | b'&' | b';')) {
            offset = start.saturating_add(4);
            continue;
        }
        let value_start = start + 4;
        let mut value_end = value_start;
        while bytes.get(value_end).is_some_and(|byte| byte.is_ascii_digit()) {
            value_end += 1;
        }
        if value_start == value_end {
            return None;
        }
        let value = source[value_start..value_end].to_owned();
        if found.replace(value).is_some() {
            return None;
        }
        offset = value_end;
    }
    found
}

fn has_visible_result_message(
    document: &Html,
    world: MoogleMailWorld,
) -> Result<bool, MoogleMailError> {
    let selector = selector("#messagebox_inner > p", world)?;
    Ok(document
        .select(&selector)
        .any(|paragraph| !normalize_text(paragraph.text().collect::<String>()).is_empty()))
}

fn direct_element_children(element: ElementRef<'_>) -> Vec<ElementRef<'_>> {
    element.children().filter_map(ElementRef::wrap).collect()
}

fn selector(value: &str, world: MoogleMailWorld) -> Result<Selector, MoogleMailError> {
    Selector::parse(value).map_err(|_| compose_error(world, MoogleMailErrorCategory::ProtocolDrift))
}

fn extract_equipment_id(value: &str) -> Option<String> {
    let marker = "equips.set(";
    let matches = value.match_indices(marker).collect::<Vec<_>>();
    if matches.len() != 1 {
        return None;
    }
    let (start, _) = matches[0];
    if start > 0
        && !value[..start]
            .chars()
            .next_back()
            .is_some_and(|character| character.is_whitespace() || character == ';')
    {
        return None;
    }
    let remainder = value[start + marker.len()..].trim_start();
    let end = remainder.find(|character: char| !character.is_ascii_digit())?;
    let equipment_id = &remainder[..end];
    (valid_decimal_id(equipment_id) && remainder[end..].trim_start().starts_with(','))
        .then(|| equipment_id.to_owned())
}

fn valid_body(value: &str) -> bool {
    value.len() <= MAX_BODY_BYTES
        && !value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
}

fn canonical_body(value: &str) -> String {
    value.replace("\r\n", "\n").trim_end_matches([' ', '\t', '\n']).to_owned()
}

fn normalize_text(value: String) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn compose_error(world: MoogleMailWorld, category: MoogleMailErrorCategory) -> MoogleMailError {
    MoogleMailError::new(MoogleMailOperation::Compose, world, category, false)
}
