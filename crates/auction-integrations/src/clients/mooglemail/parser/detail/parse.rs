//! 详情页 token、字段与附件清单的解析。

use super::super::*;

pub(crate) fn parse_token(
    document: &Html,
    world: MoogleMailWorld,
) -> Result<MoogleMailToken, MoogleMailError> {
    parse_token_for_operation(document, world, MoogleMailOperation::Detail)
}

pub(crate) fn parse_token_for_operation(
    document: &Html,
    world: MoogleMailWorld,
    operation: MoogleMailOperation,
) -> Result<MoogleMailToken, MoogleMailError> {
    let form_selector = selector("form#mailform", operation, world)?;
    let token_selector = selector("input[name='mmtoken']", operation, world)?;
    let form = unique_element(document.select(&form_selector), operation, world)?;
    let input = unique_element(form.select(&token_selector), operation, world)?;
    let value = input.value().attr("value").ok_or_else(|| {
        detail_error_for(operation, world, MoogleMailErrorCategory::ProtocolDrift)
    })?;
    MoogleMailToken::new(value)
        .ok_or_else(|| detail_error_for(operation, world, MoogleMailErrorCategory::ProtocolDrift))
}

pub(crate) fn has_result_message(
    document: &Html,
    world: MoogleMailWorld,
) -> Result<bool, MoogleMailError> {
    let selector = selector("#messagebox_inner", MoogleMailOperation::Detail, world)?;
    let boxes = document.select(&selector).take(2).collect::<Vec<_>>();
    if boxes.len() > 1 {
        return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    Ok(boxes
        .into_iter()
        .flat_map(|item| direct_children(item, "p"))
        .any(|paragraph| !normalize_text(paragraph.text().collect::<String>()).is_empty()))
}

pub(crate) fn parse_detail_fields(
    form: ElementRef<'_>,
    world: MoogleMailWorld,
) -> Result<DetailFields, MoogleMailError> {
    parse_detail_fields_for(form, world, MoogleMailOperation::Detail)
}

pub(crate) fn parse_detail_fields_for(
    form: ElementRef<'_>,
    world: MoogleMailWorld,
    operation: MoogleMailOperation,
) -> Result<DetailFields, MoogleMailError> {
    let left_selector = selector("#mmail_left", operation, world)?;
    let row_selector = selector("tr", operation, world)?;
    let div_selector = selector("div", operation, world)?;
    let input_selector = selector("input", operation, world)?;
    let left = unique_element(form.select(&left_selector), operation, world)?;
    let mut values = [None; 3];
    for row in left.select(&row_selector) {
        let label = row.select(&div_selector).next().map(detail_field_label).unwrap_or_default();
        let index = match label.as_str() {
            "To" => 0,
            "From" => 1,
            "Subject" => 2,
            _ => continue,
        };
        let mut inputs = row.select(&input_selector);
        let value =
            inputs.next().and_then(|input| input.value().attr("value")).ok_or_else(|| {
                detail_error_for(operation, world, MoogleMailErrorCategory::ProtocolDrift)
            })?;
        if inputs.next().is_some() || values[index].replace(value).is_some() {
            return Err(detail_error_for(operation, world, MoogleMailErrorCategory::ProtocolDrift));
        }
    }
    let recipient = values[0].unwrap_or_default().trim();
    let sender = values[1].unwrap_or_default();
    let subject = values[2].unwrap_or_default();
    if recipient.is_empty()
        || !valid_line(recipient, 256, false)
        || !valid_line(sender, 256, false)
        || !valid_line(subject, MAX_SUMMARY_SUBJECT_BYTES, true)
    {
        return Err(detail_error_for(operation, world, MoogleMailErrorCategory::ProtocolDrift));
    }
    Ok(DetailFields {
        recipient: recipient.to_owned(),
        sender: sender.to_owned(),
        subject: subject.to_owned(),
    })
}

fn detail_field_label(label: ElementRef<'_>) -> String {
    let visible = normalize_text(label.text().collect::<String>());
    if !visible.is_empty() {
        return visible.trim_end_matches(':').to_owned();
    }
    decode_css_glyph_label(label, "fr f2b", true).unwrap_or_default()
}

pub(crate) fn parse_detail_assets(
    document: &Html,
    body: &str,
    world: MoogleMailWorld,
) -> Result<DetailAssets, MoogleMailError> {
    let manifest = parse_detail_asset_manifest(document, world)?;
    let wanted = manifest.equipment_ids.iter().cloned().collect::<HashSet<_>>();
    let mut keys = equipment_keys(document, body, &wanted, world)?;
    let attachments = manifest
        .equipment_ids
        .into_iter()
        .map(|equipment_id| {
            let key = keys
                .remove(&equipment_id)
                .ok_or_else(|| detail_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
            Ok(MoogleMailAttachment { equipment_id, key })
        })
        .collect::<Result<Vec<_>, MoogleMailError>>()?;
    Ok(DetailAssets { attachments, credits: manifest.credits, cod: manifest.cod })
}

/// 动作前置核对只需附件 ID 与金额，不应为远端写入链路额外保留装备 key。
pub(crate) fn parse_detail_asset_manifest(
    document: &Html,
    world: MoogleMailWorld,
) -> Result<DetailAssetManifest, MoogleMailError> {
    let panes_selector = selector("#mmail_attachpanes", MoogleMailOperation::Detail, world)?;
    let list_selector = selector("#mmail_attachlist", MoogleMailOperation::Detail, world)?;
    let pane =
        unique_element(document.select(&panes_selector), MoogleMailOperation::Detail, world)?;
    let script = script_text(document, world, MoogleMailOperation::Detail)?;
    let executable = executable_javascript(&script)
        .ok_or_else(|| detail_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
    let count = required_script_value(&executable, "attach_count")
        .ok_or_else(|| detail_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
    let cod = required_script_value(&executable, "attach_cod")
        .ok_or_else(|| detail_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
    let state = required_script_value(&executable, "mail_state")
        .ok_or_else(|| detail_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
    // 当前 Inbox 在详情被打开后会把空附件邮件标记为已读（mail_state=2）；
    // 含资产邮件仍必须处于可领取态（mail_state=1），避免把不可领取资产误交给业务层。
    let valid_state = if count == 0 && cod == 0 { matches!(state, 1 | 2) } else { state == 1 };
    if !(0..=MAX_ATTACHMENT_ENTRIES as i64).contains(&count) || cod < 0 || !valid_state {
        return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    let all_lists = document.select(&list_selector).take(2).collect::<Vec<_>>();
    let nested_lists = pane.select(&list_selector).take(2).collect::<Vec<_>>();
    if all_lists.len() != nested_lists.len() || nested_lists.len() > 1 {
        return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    let (equipment_ids, credits, entries) = if let Some(list) = nested_lists.first() {
        parse_attachment_manifest(*list, world)?
    } else {
        (Vec::new(), 0, 0)
    };
    if entries as i64 != count
        || (count == 0 && !nested_lists.is_empty())
        || (count > 0 && nested_lists.len() != 1)
        || (cod > 0 && count == 0)
    {
        return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    Ok(DetailAssetManifest { equipment_ids, credits, cod, entry_count: entries })
}

pub(crate) fn parse_attachment_manifest(
    list: ElementRef<'_>,
    world: MoogleMailWorld,
) -> Result<(Vec<String>, i64, usize), MoogleMailError> {
    let entries = direct_element_children(list);
    if entries.is_empty() || entries.len() > MAX_ATTACHMENT_ENTRIES {
        return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    let list_text = normalize_text(list.text().collect::<String>());
    let child_text = normalize_text(entries.iter().flat_map(|entry| entry.text()).collect());
    if list_text != child_text {
        return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    let onmouseover_selector = selector("[onmouseover]", MoogleMailOperation::Detail, world)?;
    let mut equipment_ids = Vec::new();
    let mut seen = HashSet::new();
    let mut credits = 0_i64;
    for entry in &entries {
        let hooks = entry
            .value()
            .attr("onmouseover")
            .map(|_| *entry)
            .into_iter()
            .chain(entry.select(&onmouseover_selector));
        let hook = optional_unique_element(hooks, MoogleMailOperation::Detail, world)?;
        let text = normalize_text(entry.text().collect::<String>());
        if let Some(hook) = hook {
            let script = hook
                .value()
                .attr("onmouseover")
                .ok_or_else(|| detail_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
            let equipment_id = extract_equipment_id(script)
                .ok_or_else(|| detail_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
            if normalize_text(hook.text().collect::<String>()) != text
                || !seen.insert(equipment_id.clone())
            {
                return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
            }
            equipment_ids.push(equipment_id);
        } else {
            let amount = parse_credits_text(&text)
                .ok_or_else(|| detail_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
            if credits != 0 || amount <= 0 {
                return Err(detail_error(world, MoogleMailErrorCategory::ProtocolDrift));
            }
            credits = amount;
        }
    }
    Ok((equipment_ids, credits, entries.len()))
}

pub(crate) fn parse_credits_text(value: &str) -> Option<i64> {
    let value = value.strip_suffix(" Credits")?;
    let value = value.strip_suffix('x').unwrap_or(value);
    let amount = parse_canonical_credits(value)?;
    (amount > 0).then_some(amount)
}

pub(crate) fn parse_canonical_credits(value: &str) -> Option<i64> {
    let mut groups = value.split(',').peekable();
    let first = groups.next()?;
    let grouped = groups.peek().is_some();
    if first.is_empty()
        || !first.bytes().all(|byte| byte.is_ascii_digit())
        || (first.len() > 1 && first.starts_with('0'))
        || (grouped && (first.len() > 3 || first == "0"))
        || groups.any(|group| group.len() != 3 || !group.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return None;
    }
    // 校验分组后直接累加数字；不为金额解析分配分组 Vec 或去逗号的副本。
    value.bytes().filter(|byte| *byte != b',').try_fold(0_i64, |amount, digit| {
        amount.checked_mul(10)?.checked_add(i64::from(digit - b'0'))
    })
}

pub(crate) fn parse_current_cod(value: &str) -> Option<i64> {
    const PREFIXES: [&str; 4] =
        ["Current CoD:", "CoD:", "Requested Payment on Delivery:", "Requested CoD:"];
    let value = PREFIXES
        .iter()
        .find_map(|prefix| {
            value
                .get(..prefix.len())
                .filter(|head| head.eq_ignore_ascii_case(prefix))
                .map(|_| &value[prefix.len()..])
        })?
        .trim();
    // 页面可能用 `Credits` 或 `credits` 收尾，大小写不能影响金额证明。
    let value = value
        .strip_suffix(" Credits")
        .or_else(|| value.strip_suffix(" credits"))
        .unwrap_or(value)
        .trim();
    parse_canonical_credits(value)
}

pub(crate) fn parse_networth_credits(value: &str) -> Option<i64> {
    // HentaiVerse 当前页面在标签后使用冒号，旧页面使用空格；只兼容这两个完整分隔符，
    // 避免把 `Credits123` 或其他相似前缀误认成余额证明。
    let value = value.strip_prefix("Credits: ").or_else(|| value.strip_prefix("Credits "))?;
    parse_canonical_credits(value)
}

pub(crate) fn canonical_equipment_ids(values: &[String]) -> bool {
    values.iter().enumerate().all(|(index, value)| {
        valid_decimal_id(value) && (index == 0 || values[index - 1].as_str() < value.as_str())
    })
}
