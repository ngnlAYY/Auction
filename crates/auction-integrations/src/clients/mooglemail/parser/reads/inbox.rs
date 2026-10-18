//! MoogleMail inbox 分页解析。

use super::super::*;

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
            // 翻页期间新邮件到达会让整页下移并重复同一 MID；重复只跳过该行，
            // 不能因此中止整轮读取。真正缺行由调用方的数量/分页校验发现。
            if !seen.insert(external_mail_id.clone()) {
                continue;
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
}
