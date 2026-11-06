//! Inbox 与 Read Mail 的真实收件列表解析；页签来源由调用方明确指定。

use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};

use crate::diagnostics as safe_diagnostics;

use super::super::*;

impl MoogleMailParser {
    /// 严格解析一页 inbox，去重已见 MID；非空页若全部重复仍按协议漂移处理。
    pub fn scan_inbox_page(
        &self,
        document: &[u8],
        world: MoogleMailWorld,
        page: usize,
        seen: &mut HashSet<String>,
    ) -> Result<MoogleMailInboxPage, MoogleMailError> {
        self.scan_received_page(document, world, page, seen, MoogleMailOperation::Inbox)
    }

    /// Read Mail 只供已知邮件恢复和普通观测，不能从历史列表创建新业务指令。
    pub fn scan_read_mail_page(
        &self,
        document: &[u8],
        world: MoogleMailWorld,
        page: usize,
        seen: &mut HashSet<String>,
    ) -> Result<MoogleMailInboxPage, MoogleMailError> {
        self.scan_received_page(document, world, page, seen, MoogleMailOperation::ReadMail)
    }

    fn scan_received_page(
        &self,
        document: &[u8],
        world: MoogleMailWorld,
        page: usize,
        seen: &mut HashSet<String>,
        operation: MoogleMailOperation,
    ) -> Result<MoogleMailInboxPage, MoogleMailError> {
        let filter = match operation {
            MoogleMailOperation::Inbox => "inbox",
            MoogleMailOperation::ReadMail => "read",
            _ => return Err(protocol(operation, world)),
        };
        let source = str::from_utf8(document).map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "decode_inbox_page",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            protocol(operation, world)
        })?;
        if source.trim().is_empty() {
            return Err(protocol(operation, world));
        }
        let document = Html::parse_document(source);
        let table_selector = selector("table#mmail_list", operation, world)?;
        let table = unique_element(document.select(&table_selector), operation, world)?;
        let row_selector = selector("tr", operation, world)?;
        // 多保留一行用于拒绝超限；不先为所有异常行分配临时索引。
        let rows =
            table.select(&row_selector).take(MAX_INBOX_ROWS_PER_PAGE + 2).collect::<Vec<_>>();
        if rows.is_empty() || rows.len().saturating_sub(1) > MAX_INBOX_ROWS_PER_PAGE {
            return Err(protocol(operation, world));
        }
        if !valid_inbox_header(rows[0], world)? {
            return Err(protocol(operation, world));
        }

        let cell_selector = selector("td", operation, world)?;
        let empty_selector = selector("td#mmail_nnm", operation, world)?;
        let mut summaries = Vec::with_capacity(rows.len().saturating_sub(1));
        let mut empty_row = false;
        for row in rows.iter().skip(1) {
            let cells = row.select(&cell_selector).take(5).collect::<Vec<_>>();
            let is_empty = row.select(&empty_selector).take(2).count() == 1;
            if is_empty {
                if cells.len() != 1 || empty_row || !summaries.is_empty() {
                    return Err(protocol(operation, world));
                }
                empty_row = true;
                continue;
            }
            if empty_row || cells.len() != 4 {
                return Err(protocol(operation, world));
            }
            let onclick = row.value().attr("onclick").ok_or_else(|| protocol(operation, world))?;
            let external_mail_id =
                extract_mid(onclick).ok_or_else(|| protocol(operation, world))?;
            // 翻页期间新邮件到达可能重复 MID，跳过已见行；去重不等于远端一致性快照，
            // 调用方必须读到末页才允许详情 GET 改变列表成员关系。
            if !seen.insert(external_mail_id.clone()) {
                continue;
            }
            let sender = normalize_text(cells[0].text().collect::<String>());
            let subject = normalize_text(cells[1].text().collect::<String>());
            let received_at = parse_page_time(
                &normalize_text(cells[2].text().collect::<String>()),
                operation,
                world,
            )?;
            if !valid_line(&sender, 256, false)
                || !valid_line(&subject, MAX_SUMMARY_SUBJECT_BYTES, true)
            {
                return Err(protocol(operation, world));
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
            return Err(protocol(operation, world));
        }
        let has_next_page = parse_next_page(&document, world, page, filter, operation)?;
        Ok(MoogleMailInboxPage { summaries, has_next_page })
    }

    /// 缺失只有完整的远端邮件不存在提示才能证明；列表缺席、登录页和结构漂移均不等价。
    pub(crate) fn parse_action_state_or_missing(
        &self,
        document: &[u8],
        world: MoogleMailWorld,
    ) -> Result<Option<MoogleMailActionState>, MoogleMailError> {
        let error = match self.parse_action_state(document, world) {
            Ok(state) => return Ok(Some(state)),
            Err(error) => error,
        };
        let source = str::from_utf8(document).map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "decode_missing_detail",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            error
        })?;
        let html = Html::parse_document(source);
        let content = selector(
            "#mmail_outer, form#mailform, #mmail_attachpanes",
            MoogleMailOperation::Detail,
            world,
        )?;
        if html.select(&content).next().is_some() {
            return Err(error);
        }
        let message_selector = selector("#messagebox_inner", MoogleMailOperation::Detail, world)?;
        let mut messages = html.select(&message_selector);
        let Some(message) = messages.next() else { return Err(error) };
        let hidden =
            selector("script, style, noscript, textarea", MoogleMailOperation::Detail, world)?;
        // 只接受完整的邮件缺失提示；其他对象不存在、隐藏正文或残缺详情都不能证明 Take。
        // 线上提示带 MoogleMail 的语气词（`, kupo!`），旧页面没有；两个完整形态之外不放宽。
        let text = normalize_text(message.text().collect::<String>());
        let text = text.trim_end_matches('.');
        let missing = text.eq_ignore_ascii_case("Mail does not exist")
            || text.eq_ignore_ascii_case("Mail does not exist, kupo!");
        if messages.next().is_none() && message.select(&hidden).next().is_none() && missing {
            Ok(None)
        } else {
            Err(error)
        }
    }
}
