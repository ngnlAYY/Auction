//! 主题与固定帖的写入提交；POST 后只接受可证明的远端身份。

use self::parent::*;
use crate::clients::ehentai::client::topic as parent;

use auction_application::core::diagnostics::DiagnosticContext;

use crate::clients::metrics::forum_operation;
use crate::diagnostics as safe_diagnostics;

impl EhentaiClient {
    pub(in crate::clients::ehentai::client) async fn submit_topic(
        &self,
        form: TopicForm,
        board_id: i32,
        rendered: ForumRenderedThread,
        metrics_world: auction_application::external_metrics::ForumWorld,
        cookie: &EhentaiCookie,
    ) -> Result<auction_application::forum::ForumPublishedThread, EhentaiError> {
        let operation = EhentaiOperation::CreateTopic;
        let mut values = form.values;
        set_form_value(&mut values, "TopicTitle", rendered.title);
        set_form_value(&mut values, "TopicDesc", rendered.description);
        set_form_value(&mut values, "Post", rendered.body);
        set_form_value(&mut values, "f", board_id.to_string());
        set_form_value(&mut values, "REPLIER", "1".to_owned());
        set_form_value(&mut values, "bbmode", "normal".to_owned());
        set_form_value(&mut values, "enableemo", "yes".to_owned());
        let (status, location, body) = self
            .submit_form(
                &form.action,
                &form.referer,
                values,
                operation,
                auction_application::external_metrics::ForumOperation::CreateTopic,
                ForumRequestScope::new(metrics_world, cookie),
            )
            .await?;
        if is_session_expired(status, location.as_ref(), &body) {
            return Err(EhentaiError::new(
                operation,
                EhentaiErrorCategory::SessionExpired,
                false,
                true,
            ));
        }
        if status.is_redirection() {
            let Some(location) = location.and_then(|value| value.to_str().ok().map(str::to_owned))
            else {
                return Err(EhentaiError::write_unknown(operation));
            };
            return self
                .topic_from_reference(&form.action, &location, board_id, operation)
                .ok_or(EhentaiError::write_unknown(operation));
        }
        if status == StatusCode::OK
            && let Some(published) =
                self.topic_from_redirect_page(&form.action, &body, board_id, operation)
        {
            return Ok(published);
        }
        Err(EhentaiError::write_unknown(operation))
    }

    async fn submit_form(
        &self,
        action: &Url,
        referer: &Url,
        values: Vec<(String, String)>,
        operation: EhentaiOperation,
        metrics_operation: auction_application::external_metrics::ForumOperation,
        scope: ForumRequestScope<'_>,
    ) -> Result<(StatusCode, Option<HeaderValue>, Vec<u8>), EhentaiError> {
        let ForumRequestScope { metrics_world, cookie } = scope;
        if values.len() > MAXIMUM_FORM_FIELDS {
            return Err(EhentaiError::protocol(operation, false));
        }
        let encoded = form_urlencoded::Serializer::new(String::new())
            .extend_pairs(values.iter().map(|(name, value)| (name.as_str(), value.as_str())))
            .finish();

        self.wait_rate(operation, metrics_world, metrics_operation).await?;
        // 尚未发送本次 POST；历史或前序写入的未知状态由调用方进度和持久门禁保留。
        self.check_ownership(cookie).await.map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "forum_topic",
                "check_send_owner",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            EhentaiError::credential(operation)
        })?;
        let request = self
            .client
            .post(action.clone())
            .header(header::USER_AGENT, USER_AGENT)
            .header(header::COOKIE, cookie_header(cookie, operation)?)
            .header(header::ORIGIN, self.origin.as_str())
            .header(header::REFERER, referer.as_str())
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .timeout(self.request_timeout)
            .body(encoded);
        if let Some(progress) = &self.write_progress {
            // 预读、限速、ownership 和请求构造均已完成；从此处起取消不能证明远端未写入。
            // Worker 已持有持久 claim；登记先于 request.send，已登记活动不再被关闭入口截断。
            if !progress.mark_write_started() {
                return Err(EhentaiError::credential(operation));
            }
        }
        let request_metrics = crate::clients::metrics::ForumRequestGuard::start(
            std::sync::Arc::clone(&self.external_metrics),
            metrics_world,
            metrics_operation,
        );
        let result = async {
            let response = request.send().await.map_err(|diagnostic_cause| {
                if diagnostic_cause.is_timeout() {
                    crate::clients::metrics::forum_signal(
                        self.external_metrics.as_ref(),
                        metrics_world,
                        metrics_operation,
                        auction_application::external_metrics::ForumSignal::Timeout,
                    );
                }
                safe_diagnostics::warn(
                    "forum_topic",
                    "submit_form",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext { write_started: Some(true), ..Default::default() },
                );
                EhentaiError::write_unknown(operation)
            })?;
            let status = response.status();
            if status == StatusCode::TOO_MANY_REQUESTS {
                crate::clients::metrics::forum_signal(
                    self.external_metrics.as_ref(),
                    metrics_world,
                    metrics_operation,
                    auction_application::external_metrics::ForumSignal::RateLimited,
                );
            }
            let location = response.headers().get(header::LOCATION).cloned();
            let body = self
                .read_response(response, operation, metrics_world, metrics_operation, true)
                .await?;
            Ok((status, location, body))
        }
        .await;
        request_metrics.finish(crate::clients::metrics::forum_result(&result));
        result
    }

    pub(in crate::clients::ehentai::client) async fn submit_fixed_post(
        &self,
        form: PostForm,
        board_id: i32,
        thread_id: &str,
        edit: Option<(&str, Option<&str>, Option<&str>)>,
        rendered: ForumRenderedPost,
        scope: ForumRequestScope<'_>,
    ) -> Result<ForumPublishedPost, EhentaiError> {
        let operation =
            if edit.is_some() { EhentaiOperation::EditPost } else { EhentaiOperation::ReplyTopic };
        let mut values = form.values;
        set_form_value(&mut values, "act", "Post".to_owned());
        set_form_value(&mut values, "CODE", if edit.is_some() { "09" } else { "03" }.to_owned());
        set_form_value(&mut values, "f", board_id.to_string());
        set_form_value(&mut values, "t", thread_id.to_owned());
        set_form_value(&mut values, "Post", rendered.body);
        set_form_value(&mut values, "st", "0".to_owned());
        if let Some((post_id, _, _)) = edit {
            set_form_value(&mut values, "p", post_id.to_owned());
            set_form_value(&mut values, "dosubmit", "Submit Modified Post".to_owned());
        } else {
            values.retain(|(name, _)| name != "p");
            set_form_value(&mut values, "dosubmit", "Add Reply".to_owned());
        }
        if let Some((_, title, description)) = edit {
            if let Some(title) = title {
                set_form_value(&mut values, "TopicTitle", title.to_owned());
            }
            if let Some(description) = description {
                set_form_value(&mut values, "TopicDesc", description.to_owned());
            }
        }
        let (status, location, body) = self
            .submit_form(
                &form.action,
                &form.referer,
                values,
                operation,
                forum_operation(operation),
                scope,
            )
            .await?;
        if is_session_expired(status, location.as_ref(), &body) {
            return Err(EhentaiError::new(
                operation,
                EhentaiErrorCategory::SessionExpired,
                false,
                true,
            ));
        }
        let proof = if status.is_redirection() {
            location.and_then(|value| value.to_str().ok().map(str::to_owned)).and_then(
                |reference| {
                    self.post_from_reference(
                        &form.action,
                        &reference,
                        board_id,
                        thread_id,
                        operation,
                    )
                },
            )
        } else if status == StatusCode::OK {
            self.post_from_acknowledgement_page(&form.action, &body, board_id, thread_id, operation)
        } else {
            None
        };
        let Some(proof) = proof else {
            return Err(EhentaiError::write_unknown(operation));
        };
        if edit.is_some_and(|(expected, _, _)| proof.post_id != expected) {
            return Err(EhentaiError::write_unknown(operation));
        }
        Ok(proof)
    }
}
