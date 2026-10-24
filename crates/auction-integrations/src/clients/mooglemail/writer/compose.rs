//! compose 页面状态解析和 staged asset 证明。

use std::collections::HashSet;

use scraper::Html;

use super::common::{
    attachment_count_text, compose_error, direct_element_children, extract_equipment_id,
    has_visible_result_message, input_value, normalize_text, parse_attachment_count,
    parse_credit_attachment, selector, valid_body,
};
use super::model::{MAX_COMPOSE_ATTACHMENTS, MAX_RECIPIENT_BYTES, MAX_SUBJECT_BYTES};
use super::*;
use crate::clients::mooglemail::parser::{canonical_mail_body, parse_canonical_credits};

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
        let mut actual_equipment_ids = state.equipment_ids.iter().collect::<Vec<_>>();
        let mut expected_equipment_ids = expected_equipment_ids.iter().collect::<Vec<_>>();
        actual_equipment_ids.sort_unstable();
        expected_equipment_ids.sort_unstable();
        if actual_equipment_ids.windows(2).any(|pair| pair[0] == pair[1])
            || expected_equipment_ids.windows(2).any(|pair| pair[0] == pair[1])
            || state.recipient != message.recipient
            || state.subject != message.subject
            || canonical_mail_body(&state.body) != canonical_mail_body(&message.body)
            || state.credits != expected_credits
            || actual_equipment_ids != expected_equipment_ids
            || state.cod != Some(expected_cod)
        {
            return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
        Ok(state)
    }
}

fn parse_compose_state(
    document: &[u8],
    world: MoogleMailWorld,
) -> Result<MoogleMailComposeState, MoogleMailError> {
    let source = std::str::from_utf8(document)
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
    if bodies.len() != 1 {
        return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    let body = bodies[0].text().collect::<String>();
    if !valid_body(&body) {
        return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
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
        let text = attachment_count_text(*count_box)
            .ok_or_else(|| compose_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
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
        let value = parse_canonical_credits(raw)
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
