//! 精确标题查找；不同精确标题或同标题重复主题必须失败关闭。

use self::parent::*;
use crate::clients::ehentai::client::topic as parent;

use auction_application::core::diagnostics::DiagnosticContext;

use crate::safe_diagnostics;

impl EhentaiClient {
    pub(in crate::clients::ehentai::client) fn find_topic(
        &self,
        board_url: &Url,
        board_id: i32,
        title: &str,
        reconciliation_titles: &[String],
        body: &[u8],
        operation: EhentaiOperation,
    ) -> Result<Option<auction_application::forum::ForumPublishedThread>, EhentaiError> {
        let html = std::str::from_utf8(body).map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "forum_topic",
                "decode_topic_lookup",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            EhentaiError::protocol(operation, false)
        })?;
        let document = Html::parse_document(html);
        let selector = link_selector().map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "forum_topic",
                "load_lookup_link_selector",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            EhentaiError::protocol(operation, false)
        })?;
        let mut matched = None::<ForumPublishedThread>;
        for element in document.select(selector) {
            let label = clean_line(&element.text().collect::<String>());
            if label != title && !reconciliation_titles.iter().any(|candidate| candidate == &label)
            {
                continue;
            }
            let Some(reference) = element.value().attr("href") else {
                continue;
            };
            if let Some(published) =
                self.topic_from_reference(board_url, reference, board_id, operation)
            {
                // 不同精确标题及同标题重复主题都必须失败关闭，不能取第一个结果后新建或更新。
                if matched
                    .as_ref()
                    .is_some_and(|previous| previous.thread_id != published.thread_id)
                {
                    return Err(EhentaiError::protocol(operation, false));
                }
                matched = Some(published);
            }
        }
        Ok(matched)
    }
}
