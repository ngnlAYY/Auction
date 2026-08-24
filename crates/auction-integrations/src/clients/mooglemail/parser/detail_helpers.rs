use super::*;

pub(crate) struct DetailFields {
    pub(crate) recipient: String,
    pub(crate) sender: String,
    pub(crate) subject: String,
}

pub(crate) struct DetailAssets {
    pub(crate) attachments: Vec<MoogleMailAttachment>,
    pub(crate) credits: i64,
    pub(crate) cod: i64,
}

pub(crate) struct DetailAssetManifest {
    pub(crate) equipment_ids: Vec<String>,
    pub(crate) credits: i64,
    pub(crate) cod: i64,
    pub(crate) entry_count: usize,
}

pub(crate) fn parse_terminal_empty_detail(
    source_bytes: &[u8],
    world: MoogleMailWorld,
) -> Result<(DetailFields, String), MoogleMailError> {
    let operation = MoogleMailOperation::Take;
    let source = str::from_utf8(source_bytes).map_err(|_| action_proof_error(operation, world))?;
    if source.trim().is_empty() {
        return Err(action_proof_error(operation, world));
    }
    let document = Html::parse_document(source);
    if has_result_message(&document, world).map_err(|_| action_proof_error(operation, world))? {
        return Err(action_proof_error(operation, world));
    }
    parse_token_for_operation(&document, world, operation)
        .map_err(|_| action_proof_error(operation, world))?;

    let envelope_selector = selector("#mmail_outer", operation, world)
        .map_err(|_| action_proof_error(operation, world))?;
    let form_selector = selector("form#mailform", operation, world)
        .map_err(|_| action_proof_error(operation, world))?;
    let all_form_selector =
        selector("form", operation, world).map_err(|_| action_proof_error(operation, world))?;
    let panes_selector = selector("#mmail_attachpanes", operation, world)
        .map_err(|_| action_proof_error(operation, world))?;
    let buttons_selector = selector("#mmail_showbuttons", operation, world)
        .map_err(|_| action_proof_error(operation, world))?;
    let envelopes = document.select(&envelope_selector).collect::<Vec<_>>();
    let forms = document.select(&form_selector).collect::<Vec<_>>();
    let panes = document.select(&panes_selector).collect::<Vec<_>>();
    let buttons = document.select(&buttons_selector).collect::<Vec<_>>();
    if envelopes.len() != 1
        || forms.len() != 1
        || document.select(&all_form_selector).count() != 1
        || panes.len() != 1
        || buttons.len() != 1
        || envelopes[0].select(&form_selector).count() != 1
        || envelopes[0].select(&panes_selector).count() != 1
        || envelopes[0].select(&buttons_selector).count() != 1
    {
        return Err(action_proof_error(operation, world));
    }

    let onclick_selector = selector("[onclick]", operation, world)
        .map_err(|_| action_proof_error(operation, world))?;
    let asset_selector =
        selector("#mmail_attachlist, #mmail_attachcount, #newcod", operation, world)
            .map_err(|_| action_proof_error(operation, world))?;
    let equipment_selector = selector("[onmouseover]", operation, world)
        .map_err(|_| action_proof_error(operation, world))?;
    if document.select(&onclick_selector).next().is_some()
        || document.select(&asset_selector).next().is_some()
        || document.select(&equipment_selector).next().is_some()
        || ["attach_count", "attach_cod", "mail_state", "equips.set("]
            .iter()
            .any(|marker| source.contains(marker))
    {
        return Err(action_proof_error(operation, world));
    }

    let fields = parse_detail_fields_for(forms[0], world, operation)
        .map_err(|_| action_proof_error(operation, world))?;
    let textarea_selector =
        selector("textarea", operation, world).map_err(|_| action_proof_error(operation, world))?;
    let textareas = forms[0].select(&textarea_selector).collect::<Vec<_>>();
    if textareas.len() != 1 {
        return Err(action_proof_error(operation, world));
    }
    let body = textareas[0].text().collect::<String>();
    if !valid_message_body(&body) {
        return Err(action_proof_error(operation, world));
    }
    Ok((fields, body))
}

pub(crate) fn prove_take_identity_and_receipts(
    before: &MoogleMailActionState,
    recipient: &str,
    sender: &str,
    subject: &str,
    body: &str,
    world: MoogleMailWorld,
) -> Result<(), MoogleMailError> {
    let operation = MoogleMailOperation::Take;
    if recipient != before.recipient || sender != before.sender || subject != before.subject {
        return Err(action_proof_error(operation, world));
    }
    let base = canonical_mail_body(&before.body);
    let observed = canonical_mail_body(body);
    let Some(receipts) = observed.strip_prefix(&base) else {
        return Err(action_proof_error(operation, world));
    };
    if (before.cod > 0 && take_cod_receipt_amount(receipts) != Some(before.cod))
        || (before.cod == 0 && receipts.contains("CoD Paid:"))
        || !valid_removed_attachment_receipt(receipts, before.attachment_count > 0)
    {
        return Err(action_proof_error(operation, world));
    }
    Ok(())
}

pub(crate) fn canonical_mail_body(body: &str) -> String {
    body.replace("\r\n", "\n").trim_end_matches([' ', '\t', '\n']).to_owned()
}

pub(crate) fn take_cod_receipt_amount(value: &str) -> Option<i64> {
    const MARKER: &str = "CoD Paid: ";
    let matches = value.match_indices(MARKER).take(2).collect::<Vec<_>>();
    if matches.len() != 1 {
        return None;
    }
    let remainder = &value[matches[0].0 + MARKER.len()..];
    let amount_end = remainder
        .find(|character: char| !character.is_ascii_digit() && character != ',')
        .unwrap_or(remainder.len());
    if amount_end == 0 || !remainder[amount_end..].starts_with(" Credits") {
        return None;
    }
    parse_canonical_credits(&remainder[..amount_end])
}

pub(crate) fn valid_removed_attachment_receipt(value: &str, expected: bool) -> bool {
    const MARKER: &str = "Removed attachment:";
    let matches = value.match_indices(MARKER).take(2).collect::<Vec<_>>();
    if !expected {
        return matches.is_empty();
    }
    if matches.len() != 1 {
        return false;
    }
    let remainder = &value[matches[0].0 + MARKER.len()..];
    let line =
        remainder.trim_start_matches([' ', '\t']).split(['\r', '\n']).next().unwrap_or_default();
    !line.trim().is_empty()
}
