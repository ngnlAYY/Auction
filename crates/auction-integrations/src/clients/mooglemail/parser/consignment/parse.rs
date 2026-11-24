//! 寄售列表行与详情页到原观察的解析；未判形状留在观察标记里，不提前报协议错误。

use std::collections::HashSet;

use auction_domain::consignment::EnvelopeItemKind;
use scraper::Html;
use sha2::{Digest, Sha256};

use super::CONSIGNMENT_PARSER_VERSION;
use super::ConsignmentAttachmentObservation;
use super::ConsignmentDetailObservation;
use super::ConsignmentListRow;
use crate::clients::mooglemail::parser::{
    MAX_ATTACHMENT_ENTRIES, RawAttachmentKind, canonical_mail_body_digest, canonical_positive_eid,
    equipment_keys, extract_mid, normalize_text, parse_detail_actions_lenient,
    parse_detail_fields_for, parse_raw_attachment_manifest, parse_world_marker, read_selector,
    subject_is_list_projection, unique_element, valid_inbox_header, valid_message_body,
};
use crate::clients::mooglemail::protocol::{valid_decimal_id, valid_line, valid_opaque_id};
use crate::clients::mooglemail::{MoogleMailError, MoogleMailOperation, MoogleMailWorld};

/// 读取一页列表中的指定 MID；重复行或结构漂移失败关闭。
pub fn parse_consignment_list_row(
    document: &[u8],
    world: MoogleMailWorld,
    external_mail_id: &str,
) -> Result<Option<ConsignmentListRow>, MoogleMailError> {
    if !valid_opaque_id(external_mail_id) {
        return Err(protocol(world));
    }
    // 非 UTF-8 页面只归为协议漂移，不把原始字节带进错误或日志。
    let Some(source) = std::str::from_utf8(document).ok() else {
        return Err(protocol(world));
    };
    if source.trim().is_empty() {
        return Err(protocol(world));
    }
    let document = Html::parse_document(source);
    let table_selector = read_selector!("table#mmail_list", MoogleMailOperation::ReadMail, world)?;
    let table =
        unique_element(document.select(table_selector), MoogleMailOperation::ReadMail, world)?;
    let row_selector = read_selector!("tr", MoogleMailOperation::ReadMail, world)?;
    let cell_selector = read_selector!("td", MoogleMailOperation::ReadMail, world)?;
    let rows = table.select(row_selector).take(MAX_ATTACHMENT_ENTRIES + 2).collect::<Vec<_>>();
    let Some(header) = rows.first() else { return Err(protocol(world)) };
    if !valid_inbox_header(*header) {
        return Err(protocol(world));
    }
    let world_marker = parse_world_marker(&document, world);
    let mut row = None;
    for candidate in rows.iter().skip(1) {
        let Some(onclick) = candidate.value().attr("onclick") else { continue };
        let Some(mid) = extract_mid(onclick) else { continue };
        if mid != external_mail_id {
            continue;
        }
        if row.is_some() {
            return Err(protocol(world));
        }
        let cells = candidate.select(cell_selector).take(5).collect::<Vec<_>>();
        if cells.len() != 4 {
            return Err(protocol(world));
        }
        let sender = normalize_text(cells[0].text().collect::<String>());
        let subject = normalize_text(cells[1].text().collect::<String>());
        let received_minute_raw = normalize_text(cells[2].text().collect::<String>());
        let received_at = auction_time::parse_minute_datetime(&received_minute_raw)
            .and_then(auction_time::Timestamp::from_utc_datetime)
            .ok_or_else(|| protocol(world))?;
        if !valid_line(&sender, 256, false) || !valid_line(&subject, 512, true) {
            return Err(protocol(world));
        }
        row = Some(ConsignmentListRow {
            external_mail_id: mid,
            world,
            world_marker,
            sender,
            subject,
            received_minute_raw,
            received_at,
        });
    }
    Ok(row)
}

/// 解析详情页为受控观察；任何清单不明都留在观察标记里，不提前丢弃原信。
pub fn parse_consignment_detail(
    document: &[u8],
    world: MoogleMailWorld,
    row: &ConsignmentListRow,
) -> ConsignmentDetailObservation {
    if row.world != world || row.external_mail_id.is_empty() {
        return unavailable_observation(world, row);
    }
    let Ok(source) = std::str::from_utf8(document) else {
        return unavailable_observation(world, row);
    };
    if source.trim().is_empty() {
        return unavailable_observation(world, row);
    }
    let parsed = Html::parse_document(source);
    let world_marker = parse_world_marker(&parsed, world);
    let envelope_selector = read_selector!("#mmail_outer", MoogleMailOperation::Detail, world);
    let form_selector = read_selector!("form#mailform", MoogleMailOperation::Detail, world);
    let panes_selector = read_selector!("#mmail_attachpanes", MoogleMailOperation::Detail, world);
    let (Ok(envelope_selector), Ok(form_selector), Ok(panes_selector)) =
        (envelope_selector, form_selector, panes_selector)
    else {
        return unavailable_observation(world, row);
    };
    let envelope =
        unique_element(parsed.select(envelope_selector), MoogleMailOperation::Detail, world).ok();
    let form =
        unique_element(parsed.select(form_selector), MoogleMailOperation::Detail, world).ok();
    let panes =
        unique_element(parsed.select(panes_selector), MoogleMailOperation::Detail, world).ok();
    let (Some(envelope), Some(form), Some(_panes)) = (envelope, form, panes) else {
        return unavailable_observation(world, row);
    };
    let fields = parse_detail_fields_for(form, world, MoogleMailOperation::Detail);
    // 字段错误只决定本封未判；观察保留原定位供上层隔离，错误原文不进入任何输出。
    let (recipient, sender, subject, fields_proven) = if let Ok(fields) = fields {
        (fields.recipient, fields.sender, fields.subject, true)
    } else {
        (String::new(), String::new(), String::new(), false)
    };
    let body = read_selector!("textarea", MoogleMailOperation::Detail, world)
        .ok()
        .and_then(|selector| {
            unique_element(form.select(selector), MoogleMailOperation::Detail, world).ok()
        })
        .map(|textarea| textarea.text().collect::<String>())
        .filter(|body| valid_message_body(body));
    let body_proven = body.is_some();
    let body = body.unwrap_or_default();
    let (returnable, claimable, actions_proven) = parse_detail_actions_lenient(envelope, world);
    let raw = parse_raw_attachment_manifest(&parsed, world);
    let wanted =
        raw.entries.iter().filter_map(|entry| entry.raw_eid.clone()).collect::<HashSet<_>>();
    let mut keys = equipment_keys(&parsed, &body, &wanted, world).unwrap_or_default();
    for raw_eid in &wanted {
        if keys.contains_key(raw_eid) {
            continue;
        }
        let singleton = HashSet::from([raw_eid.clone()]);
        if let Ok(mut found) = equipment_keys(&parsed, &body, &singleton, world)
            && let Some(key) = found.remove(raw_eid)
        {
            keys.insert(raw_eid.clone(), key);
        }
    }

    let mut items = Vec::with_capacity(raw.entries.len());
    let mut raw_seen = HashSet::new();
    let mut canonical_seen = HashSet::new();
    let mut duplicate_raw_eid = false;
    let mut canonical_alias_ambiguous = false;
    let mut unknown_attachment = raw.unknown;
    for (index, entry) in raw.entries.into_iter().enumerate() {
        let kind = match entry.kind {
            RawAttachmentKind::Equipment => EnvelopeItemKind::Equipment,
            RawAttachmentKind::Credits => EnvelopeItemKind::Credits,
            RawAttachmentKind::Unknown => {
                unknown_attachment = true;
                EnvelopeItemKind::Unknown
            }
        };
        let canonical_eid = entry.raw_eid.as_deref().and_then(canonical_positive_eid);
        if kind == EnvelopeItemKind::Equipment {
            if let Some(raw_eid) = entry.raw_eid.as_deref()
                && !raw_seen.insert(raw_eid.to_owned())
            {
                duplicate_raw_eid = true;
            }
            match canonical_eid.as_deref() {
                Some(eid) if !valid_decimal_id(eid) => unknown_attachment = true,
                Some(eid) => {
                    if !canonical_seen.insert(eid.to_owned()) {
                        canonical_alias_ambiguous = true;
                    }
                }
                None => unknown_attachment = true,
            }
        }
        let key = entry.raw_eid.as_deref().and_then(|raw_eid| keys.remove(raw_eid));
        if kind == EnvelopeItemKind::Equipment && key.is_none() {
            // key 缺失仍保留装备身份；调用方会在隔离/重读流程中补证，不把整封当空信。
            unknown_attachment = true;
        }
        items.push(ConsignmentAttachmentObservation {
            ordinal: u32::try_from(index + 1).unwrap_or(u32::MAX),
            kind,
            raw_eid: entry.raw_eid,
            canonical_eid,
            amount: entry.amount,
            name: entry.name,
            key,
        });
    }
    let list_detail_consistent = fields_proven
        && body_proven
        && actions_proven
        && world_marker == Some(world)
        && row.world_marker == Some(world)
        && subject_is_list_projection(&row.subject, &subject)
        && subject_is_list_projection(&row.sender, &sender);
    let body_digest = canonical_mail_body_digest(&body);
    let subject_digest: [u8; 32] = Sha256::digest(subject.as_bytes()).into();
    let mut observation = ConsignmentDetailObservation {
        external_mail_id: row.external_mail_id.clone(),
        world,
        world_marker,
        list_world_marker: row.world_marker,
        recipient,
        sender,
        subject,
        body,
        body_digest,
        subject_digest,
        received_minute_raw: row.received_minute_raw.clone(),
        received_at: row.received_at,
        reported_attachment_count: raw.reported_count,
        credits: raw.credits,
        cod: raw.cod,
        returnable,
        claimable,
        actions_proven,
        list_detail_consistent,
        duplicate_raw_eid,
        canonical_alias_ambiguous,
        unknown_attachment,
        asset_count_unknown: raw.count.is_none()
            || raw.count.and_then(|count| usize::try_from(count).ok()) != Some(items.len()),
        mail_state: raw.mail_state,
        items,
        parser_version: CONSIGNMENT_PARSER_VERSION,
    };
    if observation.reported_attachment_count.is_none() {
        observation.asset_count_unknown = true;
    }
    observation
}

pub(super) fn unavailable_observation(
    world: MoogleMailWorld,
    row: &ConsignmentListRow,
) -> ConsignmentDetailObservation {
    ConsignmentDetailObservation {
        external_mail_id: row.external_mail_id.clone(),
        world,
        world_marker: None,
        list_world_marker: row.world_marker,
        recipient: String::new(),
        sender: String::new(),
        subject: row.subject.clone(),
        body: String::new(),
        body_digest: canonical_mail_body_digest(""),
        subject_digest: Sha256::digest(row.subject.as_bytes()).into(),
        received_minute_raw: row.received_minute_raw.clone(),
        received_at: row.received_at,
        reported_attachment_count: None,
        credits: None,
        cod: None,
        returnable: false,
        claimable: false,
        actions_proven: false,
        list_detail_consistent: false,
        duplicate_raw_eid: false,
        canonical_alias_ambiguous: false,
        unknown_attachment: true,
        asset_count_unknown: true,
        mail_state: None,
        items: Vec::new(),
        parser_version: CONSIGNMENT_PARSER_VERSION,
    }
}

fn protocol(world: MoogleMailWorld) -> MoogleMailError {
    MoogleMailError::protocol_drift(MoogleMailOperation::ReadMail, world)
}
