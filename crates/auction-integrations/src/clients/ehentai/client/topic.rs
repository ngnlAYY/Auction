//! E-Hentai 主题表单、精确标题查找与发布回执解析。

use super::*;

impl EhentaiClient {
    pub(super) async fn load_topic_form(
        &self,
        board_id: i32,
        board_url: &Url,
        cookie: &EhentaiCookie,
    ) -> Result<TopicForm, EhentaiError> {
        let operation = EhentaiOperation::CreateTopic;
        let board_value = board_id.to_string();
        let target = self.index_url(
            &[("act", "Post"), ("CODE", "00"), ("f", board_value.as_str())],
            operation,
        )?;
        let (body, _) = self.get_page(operation, target.clone(), Some(board_url), cookie).await?;
        let html =
            std::str::from_utf8(&body).map_err(|_| EhentaiError::protocol(operation, false))?;
        let document = Html::parse_document(html);
        let form_selector =
            Selector::parse("form").map_err(|_| EhentaiError::protocol(operation, false))?;
        let input_selector = Selector::parse("input[type='hidden'][name]")
            .map_err(|_| EhentaiError::protocol(operation, false))?;
        let title_selector = Selector::parse("[name='TopicTitle']")
            .map_err(|_| EhentaiError::protocol(operation, false))?;
        let post_selector = Selector::parse("[name='Post']")
            .map_err(|_| EhentaiError::protocol(operation, false))?;
        for form in document.select(&form_selector) {
            let Some(values) = posting_form_values(
                form,
                &input_selector,
                &title_selector,
                &post_selector,
                board_id,
            ) else {
                continue;
            };
            let action_value = form.value().attr("action").unwrap_or(target.as_str());
            let action = self.resolve_same_origin(&target, action_value, operation)?;
            if !posting_action_matches_board(&action, board_id)
                || !form.value().attr("method").unwrap_or("GET").eq_ignore_ascii_case("POST")
            {
                continue;
            }
            return Ok(TopicForm { action, referer: target, values });
        }
        Err(EhentaiError::protocol(operation, false))
    }

    pub(super) async fn submit_topic(
        &self,
        form: TopicForm,
        board_id: i32,
        rendered: ForumRenderedThread,
        cookie: &EhentaiCookie,
    ) -> Result<auction_application::ForumPublishedThread, EhentaiError> {
        let operation = EhentaiOperation::CreateTopic;
        let mut values = form.values;
        set_form_value(&mut values, "TopicTitle", rendered.title);
        set_form_value(&mut values, "TopicDesc", String::new());
        set_form_value(&mut values, "Post", rendered.body);
        set_form_value(&mut values, "f", board_id.to_string());
        set_form_value(&mut values, "REPLIER", "1".to_owned());
        set_form_value(&mut values, "bbmode", "normal".to_owned());
        set_form_value(&mut values, "enableemo", "yes".to_owned());
        if values.len() > MAXIMUM_FORM_FIELDS {
            return Err(EhentaiError::protocol(operation, false));
        }
        let encoded = form_urlencoded::Serializer::new(String::new())
            .extend_pairs(values.iter().map(|(name, value)| (name.as_str(), value.as_str())))
            .finish();

        self.wait_rate(operation).await?;
        let response = self
            .client
            .post(form.action.clone())
            .header(header::USER_AGENT, USER_AGENT)
            .header(header::COOKIE, cookie_header(cookie, operation)?)
            .header(header::ORIGIN, self.origin.as_str())
            .header(header::REFERER, form.referer.as_str())
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .timeout(self.request_timeout)
            .body(encoded)
            .send()
            .await
            .map_err(|_| EhentaiError::write_unknown(operation))?;
        let status = response.status();
        let location = response.headers().get(header::LOCATION).cloned();
        let body = self.read_response(response, operation, true).await?;

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

    pub(super) fn find_topic(
        &self,
        board_url: &Url,
        board_id: i32,
        title: &str,
        body: &[u8],
        operation: EhentaiOperation,
    ) -> Result<Option<auction_application::ForumPublishedThread>, EhentaiError> {
        let html =
            std::str::from_utf8(body).map_err(|_| EhentaiError::protocol(operation, false))?;
        let document = Html::parse_document(html);
        let selector =
            Selector::parse("a[href]").map_err(|_| EhentaiError::protocol(operation, false))?;
        let wanted = clean_line(title);
        for element in document.select(&selector) {
            let label = clean_line(&element.text().collect::<String>());
            if label != wanted {
                continue;
            }
            let Some(reference) = element.value().attr("href") else {
                continue;
            };
            if let Some(published) =
                self.topic_from_reference(board_url, reference, board_id, operation)
            {
                return Ok(Some(published));
            }
        }
        Ok(None)
    }

    pub(super) fn topic_from_redirect_page(
        &self,
        base: &Url,
        body: &[u8],
        board_id: i32,
        operation: EhentaiOperation,
    ) -> Option<auction_application::ForumPublishedThread> {
        let html = std::str::from_utf8(body).ok()?;
        let document = Html::parse_document(html);
        let selector = Selector::parse("a[href]").ok()?;
        for element in document.select(&selector) {
            let Some(reference) = element.value().attr("href") else {
                continue;
            };
            if let Some(published) = self.topic_from_reference(base, reference, board_id, operation)
            {
                return Some(published);
            }
        }
        None
    }

    pub(super) fn topic_from_reference(
        &self,
        base: &Url,
        reference: &str,
        board_id: i32,
        operation: EhentaiOperation,
    ) -> Option<auction_application::ForumPublishedThread> {
        let resolved = self.resolve_same_origin(base, reference, operation).ok()?;
        let mut topic_id = None;
        for (key, value) in resolved.query_pairs() {
            if key == "showtopic" || key == "t" {
                topic_id = Some(value.into_owned());
                break;
            }
        }
        let topic_id = topic_id?;
        if !valid_topic_id(&topic_id) || !posting_action_matches_board(&resolved, board_id) {
            return None;
        }
        let canonical = self.index_url(&[("showtopic", topic_id.as_str())], operation).ok()?;
        Some(auction_application::ForumPublishedThread {
            board_id,
            thread_id: topic_id,
            thread_url: canonical.to_string(),
        })
    }
}
