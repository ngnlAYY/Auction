use super::*;

impl MoogleMailParser {
    /// 严格解析一页 inbox，并将跨页重复 ID 视为协议漂移。
    pub fn scan_inbox_page(
        &self,
        document: &[u8],
        world: MoogleMailWorld,
        page: usize,
        seen: &mut HashSet<String>,
    ) -> Result<MoogleMailInboxPage, MoogleMailError> {
        let source =
            str::from_utf8(document).map_err(|_| protocol(MoogleMailOperation::Inbox, world))?;
        if source.trim().is_empty() {
            return Err(protocol(MoogleMailOperation::Inbox, world));
        }
        let document = Html::parse_document(source);
        let table_selector = selector("table#mmail_list", MoogleMailOperation::Inbox, world)?;
        let tables = document.select(&table_selector).collect::<Vec<_>>();
        if tables.len() != 1 {
            return Err(protocol(MoogleMailOperation::Inbox, world));
        }
        let row_selector = selector("tr", MoogleMailOperation::Inbox, world)?;
        let rows = tables[0].select(&row_selector).collect::<Vec<_>>();
        if rows.is_empty() || rows.len().saturating_sub(1) > MAX_INBOX_ROWS_PER_PAGE {
            return Err(protocol(MoogleMailOperation::Inbox, world));
        }
        if !valid_inbox_header(rows[0], world)? {
            return Err(protocol(MoogleMailOperation::Inbox, world));
        }

        let cell_selector = selector("td", MoogleMailOperation::Inbox, world)?;
        let mut summaries = Vec::with_capacity(rows.len().saturating_sub(1));
        let mut empty_row = false;
        for row in rows.iter().skip(1) {
            let cells = row.select(&cell_selector).collect::<Vec<_>>();
            let empty_selector = selector("td#mmail_nnm", MoogleMailOperation::Inbox, world)?;
            let is_empty = row.select(&empty_selector).count() == 1;
            if is_empty {
                if cells.len() != 1 || empty_row || !summaries.is_empty() {
                    return Err(protocol(MoogleMailOperation::Inbox, world));
                }
                empty_row = true;
                continue;
            }
            if empty_row || cells.len() != 4 {
                return Err(protocol(MoogleMailOperation::Inbox, world));
            }
            let onclick = row
                .value()
                .attr("onclick")
                .ok_or_else(|| protocol(MoogleMailOperation::Inbox, world))?;
            let external_mail_id =
                extract_mid(onclick).ok_or_else(|| protocol(MoogleMailOperation::Inbox, world))?;
            if !seen.insert(external_mail_id.clone()) {
                return Err(protocol(MoogleMailOperation::Inbox, world));
            }
            let sender = normalize_text(cells[0].text().collect::<String>());
            let subject = normalize_text(cells[1].text().collect::<String>());
            let received_at = parse_page_time(
                &normalize_text(cells[2].text().collect::<String>()),
                MoogleMailOperation::Inbox,
                world,
            )?;
            if !valid_line(&sender, 256, false)
                || !valid_line(&subject, MAX_SUMMARY_SUBJECT_BYTES, true)
            {
                return Err(protocol(MoogleMailOperation::Inbox, world));
            }
            summaries.push(MoogleMailSummary {
                external_mail_id,
                world,
                sender,
                subject,
                received_at,
            });
        }
        if !empty_row && summaries.is_empty() {
            return Err(protocol(MoogleMailOperation::Inbox, world));
        }
        let has_next_page =
            parse_next_page(&document, world, page, "inbox", MoogleMailOperation::Inbox)?;
        Ok(MoogleMailInboxPage { summaries, has_next_page })
    }

    /// 严格解析一页 Sent 列表，并将跨页重复 ID 视为协议漂移。
    pub fn scan_sent_page(
        &self,
        document: &[u8],
        world: MoogleMailWorld,
        page: usize,
        seen: &mut HashSet<String>,
    ) -> Result<MoogleMailSentPage, MoogleMailError> {
        let source =
            str::from_utf8(document).map_err(|_| protocol(MoogleMailOperation::Sent, world))?;
        if source.trim().is_empty() {
            return Err(protocol(MoogleMailOperation::Sent, world));
        }
        let document = Html::parse_document(source);
        let table_selector = selector("table#mmail_list", MoogleMailOperation::Sent, world)?;
        let tables = document.select(&table_selector).collect::<Vec<_>>();
        if tables.len() != 1 {
            return Err(protocol(MoogleMailOperation::Sent, world));
        }
        let row_selector = selector("tr", MoogleMailOperation::Sent, world)?;
        let rows = tables[0].select(&row_selector).collect::<Vec<_>>();
        if rows.is_empty() || rows.len().saturating_sub(1) > MAX_SENT_ROWS_PER_PAGE {
            return Err(protocol(MoogleMailOperation::Sent, world));
        }
        if !valid_sent_header(rows[0], world)? {
            return Err(protocol(MoogleMailOperation::Sent, world));
        }

        let cell_selector = selector("td", MoogleMailOperation::Sent, world)?;
        let mut summaries = Vec::with_capacity(rows.len().saturating_sub(1));
        let mut empty_row = false;
        for row in rows.iter().skip(1) {
            let cells = row.select(&cell_selector).collect::<Vec<_>>();
            let empty_selector = selector("td#mmail_nnm", MoogleMailOperation::Sent, world)?;
            let is_empty = row.select(&empty_selector).count() == 1;
            if is_empty {
                if cells.len() != 1 || empty_row || !summaries.is_empty() {
                    return Err(protocol(MoogleMailOperation::Sent, world));
                }
                empty_row = true;
                continue;
            }
            if empty_row || cells.len() != 4 {
                return Err(protocol(MoogleMailOperation::Sent, world));
            }
            let onclick = row
                .value()
                .attr("onclick")
                .ok_or_else(|| protocol(MoogleMailOperation::Sent, world))?;
            let external_mail_id =
                extract_mid(onclick).ok_or_else(|| protocol(MoogleMailOperation::Sent, world))?;
            if !seen.insert(external_mail_id.clone()) {
                return Err(protocol(MoogleMailOperation::Sent, world));
            }
            let recipient = normalize_text(cells[0].text().collect::<String>());
            let subject = normalize_text(cells[1].text().collect::<String>());
            let sent_at = parse_page_time(
                &normalize_text(cells[2].text().collect::<String>()),
                MoogleMailOperation::Sent,
                world,
            )?;
            let read_state = normalize_text(cells[3].text().collect::<String>());
            if !valid_line(&recipient, 256, false)
                || !valid_line(&subject, MAX_SUMMARY_SUBJECT_BYTES, false)
                || !valid_line(&read_state, 64, true)
            {
                return Err(protocol(MoogleMailOperation::Sent, world));
            }
            summaries.push(MoogleMailSentSummary {
                external_mail_id,
                world,
                recipient,
                subject,
                sent_at,
            });
        }
        if !empty_row && summaries.is_empty() {
            return Err(protocol(MoogleMailOperation::Sent, world));
        }
        let has_next_page =
            parse_next_page(&document, world, page, "sent", MoogleMailOperation::Sent)?;
        Ok(MoogleMailSentPage { summaries, has_next_page })
    }

    /// 严格解析与 Sent 摘要身份一致的详情快照。
    pub fn parse_sent_detail(
        &self,
        document: &[u8],
        summary: &MoogleMailSentSummary,
        observed_at: DateTime<Utc>,
    ) -> Result<MoogleMailSentSnapshot, MoogleMailError> {
        if !summary.validate() || observed_at < summary.sent_at {
            return Err(detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::Configuration,
            ));
        }
        let source = str::from_utf8(document).map_err(|_| {
            detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            )
        })?;
        if source.trim().is_empty() {
            return Err(detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        }
        let document = Html::parse_document(source);
        parse_token_for_operation(&document, summary.world, MoogleMailOperation::SentDetail)?;
        let envelope_selector =
            selector("#mmail_outer", MoogleMailOperation::SentDetail, summary.world)?;
        let form_selector =
            selector("form#mailform", MoogleMailOperation::SentDetail, summary.world)?;
        let panes_selector =
            selector("#mmail_attachpanes", MoogleMailOperation::SentDetail, summary.world)?;
        let envelopes = document.select(&envelope_selector).collect::<Vec<_>>();
        let forms = document.select(&form_selector).collect::<Vec<_>>();
        let panes = document.select(&panes_selector).collect::<Vec<_>>();
        if envelopes.len() != 1 || forms.len() != 1 || panes.len() != 1 {
            return Err(detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        }
        if envelopes[0].select(&form_selector).count() != 1
            || envelopes[0].select(&panes_selector).count() != 1
        {
            return Err(detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        }
        let fields =
            parse_detail_fields_for(forms[0], summary.world, MoogleMailOperation::SentDetail)?;
        if !fields.recipient.eq_ignore_ascii_case(&summary.recipient)
            || fields.subject != summary.subject
        {
            return Err(detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::IdentityMismatch,
            ));
        }

        let script = script_text(&document, summary.world, MoogleMailOperation::SentDetail)?;
        let executable = executable_javascript(&script).ok_or_else(|| {
            detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            )
        })?;
        let count = required_script_value(&executable, "attach_count");
        let cod = required_script_value(&executable, "attach_cod");
        let state = required_script_value(&executable, "mail_state");
        if count.is_none()
            || cod.is_none()
            || state != Some(1)
            || count.is_some_and(|value| !(0..=MAX_ATTACHMENT_ENTRIES as i64).contains(&value))
            || cod.is_some_and(|value| value < 0)
        {
            return Err(detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        }
        let (Some(count), Some(cod)) = (count, cod) else {
            return Err(detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        };

        let list_selector =
            selector("#mmail_attachlist", MoogleMailOperation::SentDetail, summary.world)?;
        let all_lists = document.select(&list_selector).collect::<Vec<_>>();
        let nested_lists = panes[0].select(&list_selector).collect::<Vec<_>>();
        if all_lists.len() != nested_lists.len() || nested_lists.len() > 1 {
            return Err(detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        }
        let (remaining_equipment_ids, remaining_credits, entries) =
            if let Some(list) = nested_lists.first() {
                let (equipment_ids, credits, entries) =
                    parse_attachment_manifest(*list, summary.world)?;
                (equipment_ids, credits, entries)
            } else {
                (Vec::new(), 0, 0)
            };
        if entries as i64 != count
            || (count == 0 && !nested_lists.is_empty())
            || (count > 0 && nested_lists.len() != 1)
            || (cod > 0 && count == 0)
        {
            return Err(detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        }

        let current_cod_selector =
            selector("#mmail_currentcod", MoogleMailOperation::SentDetail, summary.world)?;
        let current_cod = document.select(&current_cod_selector).collect::<Vec<_>>();
        if current_cod.len() > 1
            || current_cod.first().is_some_and(|element| {
                parse_current_cod(&normalize_text(element.text().collect::<String>())) != Some(cod)
            })
        {
            return Err(detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        }
        let mut remaining_equipment_ids = remaining_equipment_ids;
        remaining_equipment_ids.sort_unstable();
        let snapshot = MoogleMailSentSnapshot {
            external_mail_id: summary.external_mail_id.clone(),
            world: summary.world,
            recipient: summary.recipient.clone(),
            subject: summary.subject.clone(),
            sent_at: summary.sent_at,
            remaining_credits,
            cod_pending: cod,
            collected: count == 0 && cod == 0,
            remaining_equipment_ids,
            observed_at,
        };
        if !snapshot.validate() {
            return Err(detail_error_for(
                MoogleMailOperation::SentDetail,
                summary.world,
                MoogleMailErrorCategory::ProtocolDrift,
            ));
        }
        Ok(snapshot)
    }

    /// 严格解析 Item Shop 余额，并要求可见余额与脚本余额唯一且一致。
    pub fn parse_balance(
        &self,
        document: &[u8],
        world: MoogleMailWorld,
        observed_at: DateTime<Utc>,
    ) -> Result<MoogleMailBalanceSnapshot, MoogleMailError> {
        if observed_at.timestamp_nanos_opt().is_none() {
            return Err(protocol(MoogleMailOperation::Balance, world));
        }
        let source =
            str::from_utf8(document).map_err(|_| protocol(MoogleMailOperation::Balance, world))?;
        if source.trim().is_empty() {
            return Err(protocol(MoogleMailOperation::Balance, world));
        }
        let document = Html::parse_document(source);
        let networth_selector = selector("#networth", MoogleMailOperation::Balance, world)?;
        let networth = document.select(&networth_selector).collect::<Vec<_>>();
        if networth.len() != 1 {
            return Err(protocol(MoogleMailOperation::Balance, world));
        }
        let visible_credits =
            parse_networth_credits(&normalize_text(networth[0].text().collect::<String>()));
        let script = script_text(&document, world, MoogleMailOperation::Balance)?;
        let executable = executable_javascript(&script)
            .ok_or_else(|| protocol(MoogleMailOperation::Balance, world))?;
        let script_credits = required_script_value(&executable, "current_credits");
        if visible_credits.is_none()
            || script_credits.is_none()
            || visible_credits != script_credits
        {
            return Err(protocol(MoogleMailOperation::Balance, world));
        }
        let Some(credits) = visible_credits else {
            return Err(protocol(MoogleMailOperation::Balance, world));
        };
        let snapshot = MoogleMailBalanceSnapshot { world, credits, observed_at };
        if !snapshot.validate() {
            return Err(protocol(MoogleMailOperation::Balance, world));
        }
        Ok(snapshot)
    }
}
