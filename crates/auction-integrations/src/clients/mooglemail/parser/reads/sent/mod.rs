//! MoogleMail Sent 分页解析；详情解析位于同目录的 `detail.rs`。

mod detail;

use super::super::*;

impl MoogleMailParser {
    /// 严格解析一页 Sent，去重已见 MID；非空页若全部重复仍按协议漂移处理。
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
        let empty_selector = selector("td#mmail_nnm", MoogleMailOperation::Sent, world)?;
        let mut summaries = Vec::with_capacity(rows.len().saturating_sub(1));
        let mut empty_row = false;
        for row in rows.iter().skip(1) {
            let cells = row.select(&cell_selector).collect::<Vec<_>>();
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
            // 翻页期间新邮件到达可能重复 MID，跳过已见行；去重不等于远端一致性快照，
            // 调用方仍须读到末页，并独立核对新增 MID 的完整详情。
            if !seen.insert(external_mail_id.clone()) {
                continue;
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
}
