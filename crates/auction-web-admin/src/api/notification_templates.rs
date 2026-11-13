//! 出站通知模板客户端：读取全部模板，逐个保存正文或恢复内置默认正文。

use super::{
    AdminApiFailure, ApiResponse, fetch_csrf_token, invalid_request_error, invalid_response_error,
    response_error, response_failure, send_get, send_request,
};
use auction_contracts::{
    AdminMailTemplateDto, AdminMailTemplateListResponse, AdminMailTemplateUpdateRequest,
    AdminMailTemplateUpdateResponse,
};
use gloo_net::http::Request;
use uuid::Uuid;

/// 单个模板正文的字节上限，与服务端一致。
const MAX_TEMPLATE_BODY_BYTES: usize = 64 * 1024;

pub(crate) async fn fetch_mail_templates() -> Result<Vec<AdminMailTemplateDto>, String> {
    let response = send_get("/api/v1/admin/mail-templates").await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<AdminMailTemplateListResponse> =
        response.json().await.map_err(invalid_response_error)?;
    let templates = payload.data.templates;
    if templates.is_empty()
        || !templates.iter().enumerate().all(|(index, template)| {
            valid_template_entry(template)
                && !templates[..index].iter().any(|earlier| earlier.key == template.key)
        })
    {
        return Err(invalid_response_error("mail templates"));
    }
    Ok(templates)
}

/// 响应里的一项是否自洽：键是 snake_case，版本与“是否自定义”对得上，必需占位符都在允许集合里。
fn valid_template_entry(template: &AdminMailTemplateDto) -> bool {
    valid_template_key(&template.key)
        && template.revision >= 0
        && !template.body.is_empty()
        && !template.default_body.is_empty()
        && (template.revision > 0 || !template.customized)
        && (template.customized || (template.usable && template.body == template.default_body))
        && (template.revision > 0) == template.updated_by.is_some()
        && (template.revision > 0) == template.updated_at.is_some()
        && template
            .required_placeholders
            .iter()
            .all(|name| template.allowed_placeholders.contains(name))
}

fn valid_template_key(key: &str) -> bool {
    let mut characters = key.chars();
    key.len() <= 64
        && characters.next().is_some_and(|first| first.is_ascii_lowercase())
        && characters.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        })
}

/// 一次保存或恢复默认的尝试。结果未知时必须整份原样重试：命令 ID、期望版本与正文都在首次
/// 提交时确定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MailTemplateAttempt {
    pub(crate) command_id: Uuid,
    pub(crate) key: String,
    pub(crate) expected_revision: i64,
    /// 新正文；`None` 表示恢复内置默认正文。
    pub(crate) body: Option<String>,
}

/// 提交保存或恢复默认；只接受与命令一致的已接受结果。
pub(crate) async fn submit_mail_template(
    attempt: &MailTemplateAttempt,
) -> Result<AdminMailTemplateDto, AdminApiFailure> {
    if attempt.command_id.is_nil()
        || attempt.expected_revision < 0
        || !valid_template_key(&attempt.key)
        || attempt.body.as_deref().is_some_and(str::is_empty)
    {
        return Err(AdminApiFailure::client("invalid_request"));
    }
    let command = AdminMailTemplateUpdateRequest {
        command_id: attempt.command_id.to_string(),
        expected_revision: attempt.expected_revision,
        body: attempt.body.clone(),
    };
    let url = format!("/api/v1/admin/mail-templates/{}", attempt.key);
    let csrf_token = fetch_csrf_token().await.map_err(AdminApiFailure::client)?;
    let response = send_request(|abort_signal| {
        Request::put(&url)
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .json(&command)
            .map_err(invalid_request_error)
    })
    .await
    .map_err(AdminApiFailure::client)?;
    if !response.ok() {
        return Err(response_failure(response).await);
    }
    let payload: ApiResponse<AdminMailTemplateUpdateResponse> = response
        .json()
        .await
        .map_err(|error| AdminApiFailure::client(invalid_response_error(error)))?;
    let accepted = payload.data.template;
    // 重放返回的是原结果，版本恰好是期望版本加一；正文与“是否自定义”必须与命令一致。
    if !valid_template_entry(&accepted)
        || accepted.key != attempt.key
        || Some(accepted.revision) != attempt.expected_revision.checked_add(1)
        || accepted.customized != attempt.body.is_some()
        || attempt.body.as_ref().is_some_and(|body| body != &accepted.body)
    {
        return Err(AdminApiFailure::client(invalid_response_error("mail template update")));
    }
    Ok(accepted)
}

/// 结果是否未知：网络错误、超时、5xx 或服务端明确的 `outcome_unknown` 都只能原样重试。
pub(crate) fn mail_template_outcome_unknown(failure: &AdminApiFailure) -> bool {
    failure.code == "outcome_unknown" || failure.status.is_none_or(|status| status >= 500)
}

/// 草稿正文不能保存的原因；只做浏览器里的即时提示，服务端仍按同一规则完整校验。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MailTemplateBodyProblem {
    /// 正文为空、首尾有空白、过长或含控制字符。
    Format,
    /// 花括号没有成对出现。
    Unclosed,
    /// 使用了该模板不允许的占位符。
    UnknownPlaceholder,
    /// 缺少必需占位符。
    MissingPlaceholder,
}

/// 按模板的占位符规则检查草稿正文。各模板的专属要求（如私信必须保留身份句）由服务端判断。
pub(crate) fn mail_template_body_problem(
    template: &AdminMailTemplateDto,
    body: &str,
) -> Option<MailTemplateBodyProblem> {
    if body.is_empty()
        || body != body.trim()
        || body.len() > MAX_TEMPLATE_BODY_BYTES
        || body
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Some(MailTemplateBodyProblem::Format);
    }
    let mut cursor = 0;
    let mut seen = Vec::new();
    while let Some(relative_start) = body[cursor..].find('{') {
        let start = cursor + relative_start;
        if body[cursor..start].contains('}') {
            return Some(MailTemplateBodyProblem::Unclosed);
        }
        let Some(relative_end) = body[start + 1..].find('}') else {
            return Some(MailTemplateBodyProblem::Unclosed);
        };
        let end = start + 1 + relative_end;
        let placeholder = &body[start + 1..end];
        if !template.allowed_placeholders.iter().any(|name| name == placeholder) {
            return Some(MailTemplateBodyProblem::UnknownPlaceholder);
        }
        seen.push(placeholder);
        cursor = end + 1;
    }
    if body[cursor..].contains('}') {
        return Some(MailTemplateBodyProblem::Unclosed);
    }
    if template.required_placeholders.iter().any(|name| !seen.contains(&name.as_str())) {
        return Some(MailTemplateBodyProblem::MissingPlaceholder);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{
        AdminApiFailure, AdminMailTemplateDto, MailTemplateBodyProblem, mail_template_body_problem,
        mail_template_outcome_unknown, valid_template_entry,
    };

    const DEFAULT_BODY: &str =
        "Item: {item_name}\nAmount: {amount} Credits\nReference: {reference}";

    fn payout() -> AdminMailTemplateDto {
        AdminMailTemplateDto {
            key: "settlement_seller_payout".to_owned(),
            body: DEFAULT_BODY.to_owned(),
            default_body: DEFAULT_BODY.to_owned(),
            customized: false,
            usable: true,
            revision: 0,
            updated_by: None,
            updated_at: None,
            required_placeholders: vec!["amount".to_owned(), "reference".to_owned()],
            allowed_placeholders: vec![
                "amount".to_owned(),
                "reference".to_owned(),
                "item_name".to_owned(),
            ],
        }
    }

    #[test]
    fn draft_is_checked_against_the_template_placeholders() {
        let template = payout();
        assert_eq!(mail_template_body_problem(&template, DEFAULT_BODY), None);
        // 可选占位符可以不用。
        assert_eq!(mail_template_body_problem(&template, "{amount} / {reference}"), None);
        assert_eq!(
            mail_template_body_problem(&template, "Payout: {amount} Credits"),
            Some(MailTemplateBodyProblem::MissingPlaceholder)
        );
        assert_eq!(
            mail_template_body_problem(&template, "{amount} {reference} {login_key}"),
            Some(MailTemplateBodyProblem::UnknownPlaceholder)
        );
        assert_eq!(
            mail_template_body_problem(&template, "{amount} {reference"),
            Some(MailTemplateBodyProblem::Unclosed)
        );
        assert_eq!(
            mail_template_body_problem(&template, "{amount} {reference} }"),
            Some(MailTemplateBodyProblem::Unclosed)
        );
        for body in
            ["", " {amount} {reference}", "{amount} {reference}\n", "{amount}\u{7}{reference}"]
        {
            assert_eq!(
                mail_template_body_problem(&template, body),
                Some(MailTemplateBodyProblem::Format),
                "{body:?}"
            );
        }
    }

    #[test]
    fn response_entries_must_be_self_consistent() {
        assert!(valid_template_entry(&payout()));
        // 没有修订却声称自定义；默认状态的正文与默认正文不一致；必需占位符不在允许集合里。
        let mut customized = payout();
        customized.customized = true;
        assert!(!valid_template_entry(&customized));
        let mut drifted = payout();
        drifted.body = "something else".to_owned();
        assert!(!valid_template_entry(&drifted));
        let mut open_set = payout();
        open_set.required_placeholders.push("login_key".to_owned());
        assert!(!valid_template_entry(&open_set));
        let mut bad_key = payout();
        bad_key.key = "../config".to_owned();
        assert!(!valid_template_entry(&bad_key));
    }

    #[test]
    fn only_unknown_outcomes_are_retried_verbatim() {
        assert!(mail_template_outcome_unknown(&AdminApiFailure::client("network_error")));
        assert!(mail_template_outcome_unknown(&AdminApiFailure {
            status: Some(503),
            code: "outcome_unknown".to_owned(),
        }));
        // 明确的拒绝与冲突不是未知结果：修正输入或刷新后用新命令提交。
        for (status, code) in [(422, "invalid_mail_template"), (409, "mail_template_conflict")] {
            assert!(!mail_template_outcome_unknown(&AdminApiFailure {
                status: Some(status),
                code: code.to_owned(),
            }));
        }
    }
}
