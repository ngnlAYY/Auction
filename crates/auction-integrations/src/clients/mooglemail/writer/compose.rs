//! compose 页面状态解析和 staged asset 证明。

use std::collections::HashSet;

use scraper::Html;

use super::common::{
    attachment_count_text, compose_error, has_visible_result_message, input_value,
    parse_attachment_count, selector,
};
use super::model::{MAX_COMPOSE_ATTACHMENTS, MAX_RECIPIENT_BYTES, MAX_SUBJECT_BYTES};
use super::*;
use crate::clients::mooglemail::{
    forms::valid_body,
    parser::{
        canonical_mail_body, direct_element_children, extract_equipment_id, normalize_text,
        optional_unique_element, parse_canonical_credits, parse_credits_text, unique_element,
    },
};

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

    /// 联合证据复用完成世界证明的 DOM，仍执行与普通 compose 读取相同的结构校验。
    pub(crate) fn parse_compose_document(
        &self,
        document: &Html,
        world: MoogleMailWorld,
    ) -> Result<MoogleMailComposeState, MoogleMailError> {
        parse_compose_document(document, world)
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
    parse_compose_document(&parsed, world)
}

fn parse_compose_document(
    parsed: &Html,
    world: MoogleMailWorld,
) -> Result<MoogleMailComposeState, MoogleMailError> {
    if has_visible_result_message(parsed, world)? {
        return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    let form_selector = selector("form#mailform", world)?;
    let form = unique_element(parsed.select(&form_selector), MoogleMailOperation::Compose, world)?;
    let token_selector = selector("input[name='mmtoken']", world)?;
    let token = unique_element(form.select(&token_selector), MoogleMailOperation::Compose, world)?;
    let token_value = token
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
    let body = unique_element(form.select(&body_selector), MoogleMailOperation::Compose, world)?
        .text()
        .collect::<String>();
    if !valid_body(&body) {
        return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    let assets = parse_compose_assets(parsed, world)?;
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
    let operation = MoogleMailOperation::Compose;
    let list = optional_unique_element(document.select(&list_selector), operation, world)?;
    let infos = document.select(&info_selector).take(2).count();
    let count_box = optional_unique_element(document.select(&count_selector), operation, world)?;
    let cod_field = optional_unique_element(document.select(&cod_selector), operation, world)?;
    if infos > 1 {
        return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    let Some(list) = list else {
        if infos != 1 || count_box.is_some() || cod_field.is_some() {
            return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
        return Ok(ComposeAssets {
            equipment_ids: Vec::new(),
            credits: 0,
            cod: Some(0),
            attachment_count: 0,
        });
    };

    let entries = direct_element_children(list);
    if entries.is_empty() || entries.len() > MAX_COMPOSE_ATTACHMENTS {
        return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    let list_text = normalize_text(list.text().collect::<String>());
    let child_text = normalize_text(entries.iter().flat_map(|entry| entry.text()).collect());
    if list_text != child_text {
        return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    let hook_selector = selector("[onmouseover]", world)?;
    let mut equipment_ids = Vec::new();
    let mut seen_equipment_ids = HashSet::new();
    let mut credits = 0_i64;
    for entry in &entries {
        let hooks = entry
            .value()
            .attr("onmouseover")
            .map(|_| *entry)
            .into_iter()
            .chain(entry.select(&hook_selector));
        let hook = optional_unique_element(hooks, operation, world)?;
        let text = normalize_text(entry.text().collect::<String>());
        if let Some(hook) = hook {
            if normalize_text(hook.text().collect::<String>()) != text {
                return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
            }
            let script = hook
                .value()
                .attr("onmouseover")
                .ok_or_else(|| compose_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
            let equipment_id = extract_equipment_id(script)
                .ok_or_else(|| compose_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
            if !seen_equipment_ids.insert(equipment_id.clone()) {
                return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
            }
            equipment_ids.push(equipment_id);
        } else {
            let amount = parse_credits_text(&text)
                .ok_or_else(|| compose_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
            if credits != 0 {
                return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
            }
            credits = amount;
        }
    }

    let mut cod = None;
    if let Some(count_box) = count_box {
        let text = attachment_count_text(count_box)
            .ok_or_else(|| compose_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
        let (count, count_cod) = parse_attachment_count(&text)
            .ok_or_else(|| compose_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
        if count != entries.len() {
            return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
        cod = count_cod;
    }
    if let Some(cod_field) = cod_field {
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
    Ok(ComposeAssets { equipment_ids, credits, cod, attachment_count: entries.len() })
}
