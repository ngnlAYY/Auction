//! 主题创建与固定帖回帖/编辑表单加载；动作、隐藏字段与目标身份必须一致。

use auction_application::core::diagnostics::DiagnosticContext;
use scraper::Html;
use url::Url;

use crate::clients::ehentai::MAXIMUM_FORUM_BODY_BYTES;
use crate::clients::ehentai::client::{EhentaiClient, ForumRequestScope};
use crate::clients::ehentai::credentials::EhentaiCookie;
use crate::clients::ehentai::error::{EhentaiError, EhentaiOperation};
use crate::clients::ehentai::forms::{
    PostForm, TopicForm, fixed_post_form_values, fixed_post_metadata, form_selector,
    hidden_input_selector, optional_query_identity, post_body_selector,
    posting_action_matches_board, posting_action_matches_target, posting_form_values,
    topic_title_selector,
};
use crate::clients::metrics::forum_operation;
use crate::safe_diagnostics;

struct FixedPostFormTarget<'a> {
    operation: EhentaiOperation,
    metrics_world: auction_application::external_metrics::ForumWorld,
    url: Url,
    board_id: i32,
    thread_id: &'a str,
    post_id: Option<&'a str>,
    expected_code: &'static str,
}

impl EhentaiClient {
    pub(in crate::clients::ehentai::client) async fn load_topic_form(
        &self,
        board_id: i32,
        board_url: &Url,
        metrics_world: auction_application::external_metrics::ForumWorld,
        cookie: &EhentaiCookie,
    ) -> Result<TopicForm, EhentaiError> {
        let operation = EhentaiOperation::CreateTopic;
        let board_value = board_id.to_string();
        let target = self.index_url(
            &[("act", "Post"), ("CODE", "00"), ("f", board_value.as_str())],
            operation,
        )?;
        let (body, _) = self
            .get_page(
                operation,
                auction_application::external_metrics::ForumOperation::CreateTopic,
                ForumRequestScope::new(metrics_world, cookie),
                target.clone(),
                Some(board_url),
            )
            .await?;
        let html = std::str::from_utf8(&body).map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "forum_topic",
                "decode_post_form",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            EhentaiError::protocol(operation, false)
        })?;
        let document = Html::parse_document(html);
        let form_selector = form_selector().map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "forum_topic",
                "load_post_form_selector",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            EhentaiError::protocol(operation, false)
        })?;
        let input_selector = hidden_input_selector().map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "forum_topic",
                "load_hidden_input_selector",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            EhentaiError::protocol(operation, false)
        })?;
        let title_selector = topic_title_selector().map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "forum_topic",
                "load_topic_title_selector",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            EhentaiError::protocol(operation, false)
        })?;
        let post_selector = post_body_selector().map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "forum_topic",
                "load_post_body_selector",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            EhentaiError::protocol(operation, false)
        })?;
        for form in document.select(form_selector) {
            let Some(values) =
                posting_form_values(form, input_selector, title_selector, post_selector, board_id)
            else {
                continue;
            };
            let action_value = form.value().attr("action").unwrap_or(target.as_str());
            let action = self.resolve_same_origin(&target, action_value, operation)?;
            // 隐藏字段不能覆盖 action 中的冲突路由；新主题不得投向其它端点或已有帖子。
            if action.path() != "/index.php"
                || !posting_action_matches_board(&action, board_id)
                || !optional_query_identity(&action, &["act"])
                    .is_ok_and(|value| value.is_none_or(|value| value == "Post"))
                || !optional_query_identity(&action, &["CODE"])
                    .is_ok_and(|value| value.is_none_or(|value| value == "01"))
                || !optional_query_identity(&action, &["t", "showtopic"])
                    .is_ok_and(|value| value.is_none())
                || !optional_query_identity(&action, &["p", "pid", "gopid"])
                    .is_ok_and(|value| value.is_none())
                || !form.value().attr("method").unwrap_or("GET").eq_ignore_ascii_case("POST")
            {
                continue;
            }
            return Ok(TopicForm { action, referer: target, values });
        }
        Err(EhentaiError::protocol(operation, false))
    }

    pub(in crate::clients::ehentai::client) async fn load_reply_form(
        &self,
        board_id: i32,
        thread_id: &str,
        metrics_world: auction_application::external_metrics::ForumWorld,
        cookie: &EhentaiCookie,
    ) -> Result<PostForm, EhentaiError> {
        let board = board_id.to_string();
        let target = self.index_url(
            &[("act", "post"), ("do", "reply_post"), ("f", board.as_str()), ("t", thread_id)],
            EhentaiOperation::ReplyTopic,
        )?;
        self.load_fixed_post_form(
            FixedPostFormTarget {
                operation: EhentaiOperation::ReplyTopic,
                metrics_world,
                url: target,
                board_id,
                thread_id,
                post_id: None,
                expected_code: "03",
            },
            cookie,
        )
        .await
    }

    pub(in crate::clients::ehentai::client) async fn load_edit_form(
        &self,
        board_id: i32,
        thread_id: &str,
        post_id: &str,
        metrics_world: auction_application::external_metrics::ForumWorld,
        cookie: &EhentaiCookie,
    ) -> Result<PostForm, EhentaiError> {
        let board = board_id.to_string();
        let target = self.index_url(
            &[
                ("act", "post"),
                ("do", "edit_post"),
                ("f", board.as_str()),
                ("t", thread_id),
                ("p", post_id),
                ("st", "0"),
            ],
            EhentaiOperation::EditPost,
        )?;
        self.load_fixed_post_form(
            FixedPostFormTarget {
                operation: EhentaiOperation::EditPost,
                metrics_world,
                url: target,
                board_id,
                thread_id,
                post_id: Some(post_id),
                expected_code: "09",
            },
            cookie,
        )
        .await
    }

    async fn load_fixed_post_form(
        &self,
        target: FixedPostFormTarget<'_>,
        cookie: &EhentaiCookie,
    ) -> Result<PostForm, EhentaiError> {
        let topic = self.index_url(&[("showtopic", target.thread_id)], target.operation)?;
        let (body, _) = self
            .get_page(
                target.operation,
                forum_operation(target.operation),
                ForumRequestScope::new(target.metrics_world, cookie),
                target.url.clone(),
                Some(&topic),
            )
            .await?;
        let html = std::str::from_utf8(&body).map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "forum_topic",
                "decode_fixed_post_form",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            EhentaiError::protocol(target.operation, false)
        })?;
        let document = Html::parse_document(html);
        let form_selector = form_selector().map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "forum_topic",
                "load_fixed_form_selector",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            EhentaiError::protocol(target.operation, false)
        })?;
        let input_selector = hidden_input_selector().map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "forum_topic",
                "load_fixed_input_selector",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            EhentaiError::protocol(target.operation, false)
        })?;
        let post_selector = post_body_selector().map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "forum_topic",
                "load_fixed_body_selector",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            EhentaiError::protocol(target.operation, false)
        })?;
        for form in document.select(form_selector) {
            let Some(body) =
                form.select(post_selector).next().map(|field| field.text().collect::<String>())
            else {
                continue;
            };
            if body.len() > MAXIMUM_FORUM_BODY_BYTES
                || body.chars().any(|character| {
                    character.is_control() && !matches!(character, '\n' | '\r' | '\t')
                })
            {
                continue;
            }
            let Some(values) = fixed_post_form_values(
                form,
                input_selector,
                post_selector,
                target.expected_code,
                target.board_id,
                target.thread_id,
                target.post_id,
            ) else {
                continue;
            };
            let action_value = form.value().attr("action").unwrap_or(target.url.as_str());
            let action = self.resolve_same_origin(&target.url, action_value, target.operation)?;
            if !posting_action_matches_target(
                &action,
                target.board_id,
                target.thread_id,
                target.post_id,
                target.expected_code,
            ) || !form.value().attr("method").unwrap_or("GET").eq_ignore_ascii_case("POST")
            {
                continue;
            }
            let metadata = if target.post_id.is_some() {
                fixed_post_metadata(form).map_err(|diagnostic_cause| {
                    safe_diagnostics::warn(
                        "forum_topic",
                        "parse_fixed_post_metadata",
                        safe_diagnostics::project(&diagnostic_cause),
                        DiagnosticContext::default(),
                    );
                    EhentaiError::protocol(target.operation, false)
                })?
            } else {
                None
            };
            return Ok(PostForm { action, referer: target.url, values, body, metadata });
        }
        Err(EhentaiError::protocol(target.operation, false))
    }
}
