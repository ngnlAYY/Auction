//! E-Hentai 主题表单、精确标题查找与发布回执解析。

use std::sync::LazyLock;

use super::*;

static ACKNOWLEDGEMENT_TITLE_SELECTOR: LazyLock<Result<Selector, ()>> =
    LazyLock::new(|| Selector::parse("title").map_err(|_| ()));
static ACKNOWLEDGEMENT_REFRESH_SELECTOR: LazyLock<Result<Selector, ()>> =
    LazyLock::new(|| Selector::parse("meta[http-equiv][content]").map_err(|_| ()));

struct FixedPostFormTarget<'a> {
    operation: EhentaiOperation,
    url: Url,
    board_id: i32,
    thread_id: &'a str,
    post_id: Option<&'a str>,
    expected_code: &'static str,
}

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
            form_selector().map_err(|_| EhentaiError::protocol(operation, false))?;
        let input_selector =
            hidden_input_selector().map_err(|_| EhentaiError::protocol(operation, false))?;
        let title_selector =
            topic_title_selector().map_err(|_| EhentaiError::protocol(operation, false))?;
        let post_selector =
            post_body_selector().map_err(|_| EhentaiError::protocol(operation, false))?;
        for form in document.select(form_selector) {
            let Some(values) =
                posting_form_values(form, input_selector, title_selector, post_selector, board_id)
            else {
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
        set_form_value(&mut values, "TopicDesc", rendered.description);
        set_form_value(&mut values, "Post", rendered.body);
        set_form_value(&mut values, "f", board_id.to_string());
        set_form_value(&mut values, "REPLIER", "1".to_owned());
        set_form_value(&mut values, "bbmode", "normal".to_owned());
        set_form_value(&mut values, "enableemo", "yes".to_owned());
        let (status, location, body) =
            self.submit_form(&form.action, &form.referer, values, operation, cookie).await?;
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
        cookie: &EhentaiCookie,
    ) -> Result<(StatusCode, Option<HeaderValue>, Vec<u8>), EhentaiError> {
        if values.len() > MAXIMUM_FORM_FIELDS {
            return Err(EhentaiError::protocol(operation, false));
        }
        let encoded = form_urlencoded::Serializer::new(String::new())
            .extend_pairs(values.iter().map(|(name, value)| (name.as_str(), value.as_str())))
            .finish();

        self.wait_rate(operation).await?;
        // 即使此次 POST 尚未发出，前面可能已有远端写入，必须保守保留未知结果。
        self.check_ownership().await.map_err(|_| EhentaiError::write_unknown(operation))?;
        let response = self
            .client
            .post(action.clone())
            .header(header::USER_AGENT, USER_AGENT)
            .header(header::COOKIE, cookie_header(cookie, operation)?)
            .header(header::ORIGIN, self.origin.as_str())
            .header(header::REFERER, referer.as_str())
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .timeout(self.request_timeout)
            .body(encoded)
            .send()
            .await
            .map_err(|_| EhentaiError::write_unknown(operation))?;
        let status = response.status();
        let location = response.headers().get(header::LOCATION).cloned();
        let body = self.read_response(response, operation, true).await?;
        Ok((status, location, body))
    }

    pub(super) async fn load_reply_form(
        &self,
        board_id: i32,
        thread_id: &str,
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

    pub(super) async fn load_edit_form(
        &self,
        board_id: i32,
        thread_id: &str,
        post_id: &str,
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
        let (body, _) =
            self.get_page(target.operation, target.url.clone(), Some(&topic), cookie).await?;
        let html = std::str::from_utf8(&body)
            .map_err(|_| EhentaiError::protocol(target.operation, false))?;
        let document = Html::parse_document(html);
        let form_selector =
            form_selector().map_err(|_| EhentaiError::protocol(target.operation, false))?;
        let input_selector =
            hidden_input_selector().map_err(|_| EhentaiError::protocol(target.operation, false))?;
        let post_selector =
            post_body_selector().map_err(|_| EhentaiError::protocol(target.operation, false))?;
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
            return Ok(PostForm { action, referer: target.url, values, body });
        }
        Err(EhentaiError::protocol(target.operation, false))
    }

    pub(super) async fn submit_fixed_post(
        &self,
        form: PostForm,
        board_id: i32,
        thread_id: &str,
        edit: Option<(&str, Option<&str>, Option<&str>)>,
        rendered: ForumRenderedPost,
        cookie: &EhentaiCookie,
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
        let (status, location, body) =
            self.submit_form(&form.action, &form.referer, values, operation, cookie).await?;
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

    fn post_from_acknowledgement_page(
        &self,
        base: &Url,
        body: &[u8],
        board_id: i32,
        thread_id: &str,
        operation: EhentaiOperation,
    ) -> Option<ForumPublishedPost> {
        let html = std::str::from_utf8(body).ok()?;
        let document = Html::parse_document(html);
        let title_selector = LazyLock::force(&ACKNOWLEDGEMENT_TITLE_SELECTOR).as_ref().ok()?;
        let mut titles = document.select(title_selector);
        let title = clean_line(&titles.next()?.text().collect::<String>());
        if title != "Please stand by..." || titles.next().is_some() {
            return None;
        }

        let refresh_selector = LazyLock::force(&ACKNOWLEDGEMENT_REFRESH_SELECTOR).as_ref().ok()?;
        let mut refreshes = document.select(refresh_selector).filter(|element| {
            element
                .value()
                .attr("http-equiv")
                .is_some_and(|value| value.eq_ignore_ascii_case("refresh"))
        });
        let refresh = refreshes.next()?;
        if refreshes.next().is_some() {
            return None;
        }
        let refresh_reference = refresh_reference(refresh.value().attr("content")?)?;
        let refresh_proof =
            self.post_from_reference(base, refresh_reference, board_id, thread_id, operation)?;

        let links = link_selector().ok()?;
        let link_proofs = document
            .select(links)
            .filter_map(|link| link.value().attr("href"))
            .filter_map(|reference| {
                self.post_from_reference(base, reference, board_id, thread_id, operation)
            });
        let link_proof = consistent_post_proof(link_proofs)?;
        (refresh_proof == link_proof).then_some(refresh_proof)
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
        let selector = link_selector().map_err(|_| EhentaiError::protocol(operation, false))?;
        let wanted = clean_line(title);
        for element in document.select(selector) {
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
        let selector = link_selector().ok()?;
        for element in document.select(selector) {
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
        let topic_id = optional_query_identity(&resolved, &["showtopic", "t"]).ok()??;
        if resolved.path() != "/index.php"
            || !valid_topic_id(&topic_id)
            || !posting_action_matches_board(&resolved, board_id)
        {
            return None;
        }
        let canonical = self.index_url(&[("showtopic", topic_id.as_str())], operation).ok()?;
        Some(auction_application::ForumPublishedThread {
            board_id,
            thread_id: topic_id,
            thread_url: canonical.to_string(),
        })
    }

    fn post_from_reference(
        &self,
        base: &Url,
        reference: &str,
        board_id: i32,
        thread_id: &str,
        operation: EhentaiOperation,
    ) -> Option<ForumPublishedPost> {
        let resolved = self.resolve_same_origin(base, reference, operation).ok()?;
        let observed_thread = optional_query_identity(&resolved, &["showtopic", "t"]).ok()??;
        let query_post = optional_query_identity(&resolved, &["p", "pid", "gopid"]).ok()?;
        let fragment_post = resolved.fragment().and_then(|fragment| {
            fragment
                .strip_prefix("entry")
                .or_else(|| fragment.strip_prefix("post-"))
                .map(str::to_owned)
        });
        let observed_post = match (query_post, fragment_post) {
            (Some(query), Some(fragment)) if query != fragment => return None,
            (Some(query), _) => query,
            (None, Some(fragment)) => fragment,
            (None, None) => return None,
        };
        if resolved.path() != "/index.php"
            || !posting_action_matches_board(&resolved, board_id)
            || observed_thread != thread_id
            || !valid_topic_id(&observed_thread)
            || !valid_post_id(&observed_post)
        {
            return None;
        }
        Some(ForumPublishedPost { thread_id: observed_thread, post_id: observed_post })
    }
}

fn refresh_reference(content: &str) -> Option<&str> {
    if content.is_empty() || content.len() > 4 * 1024 || content.chars().any(char::is_control) {
        return None;
    }
    let (_, target) = content.split_once(';')?;
    let (name, value) = target.trim().split_once('=')?;
    if !name.trim().eq_ignore_ascii_case("url") {
        return None;
    }
    let value = value.trim();
    let value = if value.len() >= 2
        && matches!(value.as_bytes()[0], b'\'' | b'"')
        && value.as_bytes().last() == value.as_bytes().first()
    {
        &value[1..value.len() - 1]
    } else {
        value
    };
    (!value.is_empty()).then_some(value)
}

fn consistent_post_proof(
    mut proofs: impl Iterator<Item = ForumPublishedPost>,
) -> Option<ForumPublishedPost> {
    let proof = proofs.next()?;
    proofs.all(|candidate| candidate == proof).then_some(proof)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_post_acknowledgement_requires_matching_meta_and_link_proof() {
        let html = br#"
            <html>
              <head>
                <title>Please stand by...</title>
                <meta http-equiv="refresh" content="2; url=index.php?showtopic=300433&amp;p=6956800#entry6956800">
              </head>
              <body>
                <p>Thanks. The post has been edited.</p>
                <a href="index.php?showtopic=300433#entry6956800">click here</a>
              </body>
            </html>
        "#;
        let config = EhentaiClientConfig::production()
            .unwrap_or_else(|error| panic!("固定论坛配置应合法: {error:?}"));
        let client = EhentaiClient::with_config(config)
            .unwrap_or_else(|error| panic!("安全论坛客户端应可构造: {error:?}"));
        let base = Url::parse("https://forums.e-hentai.org/index.php")
            .unwrap_or_else(|error| panic!("固定论坛地址应合法: {error:?}"));

        let proof = client
            .post_from_acknowledgement_page(&base, html, 4, "300433", EhentaiOperation::EditPost)
            .unwrap_or_else(|| panic!("两份一致回执应证明固定帖子身份"));

        assert_eq!(proof.thread_id, "300433");
        assert_eq!(proof.post_id, "6956800");
    }

    #[test]
    fn fixed_post_acknowledgement_rejects_conflicting_proofs() {
        let html = br#"
            <html>
              <head>
                <title>Please stand by...</title>
                <meta http-equiv="refresh" content="2; url=index.php?showtopic=300433&amp;p=6956800#entry6956800">
              </head>
              <body>
                <a href="index.php?showtopic=300433#entry6956801">click here</a>
              </body>
            </html>
        "#;
        let config = EhentaiClientConfig::production()
            .unwrap_or_else(|error| panic!("固定论坛配置应合法: {error:?}"));
        let client = EhentaiClient::with_config(config)
            .unwrap_or_else(|error| panic!("安全论坛客户端应可构造: {error:?}"));
        let base = Url::parse("https://forums.e-hentai.org/index.php")
            .unwrap_or_else(|error| panic!("固定论坛地址应合法: {error:?}"));

        assert!(
            client
                .post_from_acknowledgement_page(
                    &base,
                    html,
                    4,
                    "300433",
                    EhentaiOperation::EditPost,
                )
                .is_none()
        );
    }
}
