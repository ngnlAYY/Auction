//! 出站通知模板客户端：读取全部模板，逐个保存正文或恢复内置默认正文。

use auction_contracts::{
    AdminMailTemplateDto, AdminMailTemplateListResponse, AdminMailTemplateUpdateRequest,
    AdminMailTemplateUpdateResponse, ApiResponse,
};
use gloo_net::http::Method;
use uuid::Uuid;

use super::AdminWriteFailure;
use super::failure::AdminWriteEndpoint;
use super::transport::WriteRequest;
use super::{invalid_request_error, invalid_response_error, response_error, send_get};

/// 单个模板正文的字节上限，与服务端一致。
const MAX_TEMPLATE_BODY_BYTES: usize = 64 * 1024;
const MAX_WATCH_BODY_BYTES: usize = 16 * 1024;
// 镜像 auction-http 的 server/surfaces.rs 中 PUT /api/v1/admin/mail-templates/{template_key}。
const MAX_TEMPLATE_REQUEST_BYTES: usize = 96 * 1024;

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
        && template.body.len() <= MAX_TEMPLATE_BODY_BYTES
        && template.default_body.len() <= MAX_TEMPLATE_BODY_BYTES
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

#[cfg(test)]
type MailTemplateSubmitStub =
    std::rc::Rc<dyn Fn(&str) -> Result<AdminMailTemplateDto, AdminWriteFailure>>;

#[cfg(test)]
impl MailTemplateAttempt {
    pub(crate) fn stub_submit(
        submit: impl Fn(&str) -> Result<AdminMailTemplateDto, AdminWriteFailure> + 'static,
    ) {
        dioxus::prelude::provide_context(std::rc::Rc::new(submit) as MailTemplateSubmitStub);
    }
}

fn encode_template_request(
    command_id: Uuid,
    expected_revision: i64,
    body: Option<String>,
) -> Result<String, String> {
    let command = AdminMailTemplateUpdateRequest {
        command_id: command_id.to_string(),
        expected_revision,
        body,
    };
    let encoded = serde_json::to_string(&command).map_err(invalid_request_error)?;
    if encoded.len() > MAX_TEMPLATE_REQUEST_BYTES {
        return Err("body_too_large".to_owned());
    }
    Ok(encoded)
}

/// 提交保存或恢复默认；只接受与命令一致的已接受结果。
pub(crate) async fn submit_mail_template(
    attempt: &MailTemplateAttempt,
) -> Result<AdminMailTemplateDto, AdminWriteFailure> {
    if attempt.command_id.is_nil()
        || attempt.expected_revision < 0
        || !valid_template_key(&attempt.key)
        || attempt.body.as_deref().is_some_and(str::is_empty)
    {
        return Err(AdminWriteFailure::not_sent("invalid_request"));
    }
    let body = encode_template_request(
        attempt.command_id,
        attempt.expected_revision,
        attempt.body.clone(),
    )
    .map_err(AdminWriteFailure::not_sent)?;
    #[cfg(test)]
    if let Some(submit) = dioxus::prelude::try_consume_context::<MailTemplateSubmitStub>() {
        return submit(&body);
    }
    let url = format!("/api/v1/admin/mail-templates/{}", attempt.key);
    let response = WriteRequest::new(&url, Method::PUT, AdminWriteEndpoint::MailTemplate)
        .send_body(&body)
        .await?;
    let payload: ApiResponse<AdminMailTemplateUpdateResponse> =
        response.json().await.map_err(AdminWriteFailure::invalid_response)?;
    let accepted = payload.data.template;
    // 重放返回的是原结果，版本恰好是期望版本加一；正文与“是否自定义”必须与命令一致。
    if !valid_template_entry(&accepted)
        || accepted.key != attempt.key
        || Some(accepted.revision) != attempt.expected_revision.checked_add(1)
        || accepted.customized != attempt.body.is_some()
        || attempt.body.as_ref().is_some_and(|body| body != &accepted.body)
        || attempt.body.is_none() && accepted.body != accepted.default_body
    {
        return Err(AdminWriteFailure::invalid_response("mail template update"));
    }
    Ok(accepted)
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
    /// 装备拒收原因缺失。
    MissingRejectionReason,
    /// 私信缺少与发送证明绑定的固定句。
    MissingNotificationIdentity,
    /// 被超提醒超过冻结投递正文的字节上限。
    WatchBodyTooLong,
    /// 完整编码后的写请求超出路由的请求体上限。
    RequestTooLarge,
}

impl MailTemplateBodyProblem {
    /// 私信发送证明绑定的固定句，与服务端逐字一致。
    pub(crate) const NOTIFICATION_IDENTITY: &'static str = "Notification: {notification_id}.";
}

/// 按正文格式、占位符及模板专属要求检查草稿；旧的不可用正文也可读取并修正。
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
    let problem = match template.key.as_str() {
        "auction_equipment_rejected"
            if !seen.contains(&"reason_code") && !seen.contains(&"reason_text") =>
        {
            Some(MailTemplateBodyProblem::MissingRejectionReason)
        }
        "lot_watch_pm" if !body.contains(MailTemplateBodyProblem::NOTIFICATION_IDENTITY) => {
            Some(MailTemplateBodyProblem::MissingNotificationIdentity)
        }
        "lot_watch" | "lot_watch_pm" if body.len() > MAX_WATCH_BODY_BYTES => {
            Some(MailTemplateBodyProblem::WatchBodyTooLong)
        }
        _ => None,
    };
    problem.or_else(|| {
        // UUID 的规范文本固定为 36 字节。确认前用等长值预检，HTTP 发送复用同一个编码器。
        match encode_template_request(Uuid::nil(), template.revision, Some(body.to_owned())) {
            Ok(_) => None,
            Err(code) if code == "body_too_large" => Some(MailTemplateBodyProblem::RequestTooLarge),
            Err(_error) => Some(MailTemplateBodyProblem::Format),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{
        AdminMailTemplateDto, MailTemplateBodyProblem, mail_template_body_problem,
        valid_template_entry,
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
    fn template_specific_content_is_required() {
        let mut rejected = payout();
        rejected.key = "auction_equipment_rejected".to_owned();
        rejected.required_placeholders.clear();
        rejected.allowed_placeholders = vec!["reason_code".to_owned(), "reason_text".to_owned()];
        assert!(mail_template_body_problem(&rejected, "Rejected").is_some());
        for body in ["{reason_code}", "{reason_text}"] {
            assert_eq!(mail_template_body_problem(&rejected, body), None);
        }

        let mut pm = payout();
        pm.key = "lot_watch_pm".to_owned();
        pm.required_placeholders = vec!["item_name".into(), "lot_code".into(), "amount".into()];
        pm.allowed_placeholders = pm.required_placeholders.clone();
        pm.allowed_placeholders.push("notification_id".into());
        assert!(
            mail_template_body_problem(&pm, "{item_name} {lot_code} {amount} {notification_id}")
                .is_some()
        );
        assert_eq!(
            mail_template_body_problem(
                &pm,
                "{item_name} {lot_code} {amount}\nNotification: {notification_id}."
            ),
            None
        );
    }

    #[test]
    fn notification_template_limit_counts_utf8_bytes() {
        let mut template = payout();
        template.key = "lot_watch".to_owned();
        let prefix = "{item_name} {amount} {reference} ";
        let remaining = (16 << 10) - prefix.len();
        let limit = format!("{prefix}{}{}", "界".repeat(remaining / 3), "x".repeat(remaining % 3));
        assert_eq!(mail_template_body_problem(&template, &limit), None);
        assert!(mail_template_body_problem(&template, &(limit + "x")).is_some());
    }

    #[test]
    fn template_check_counts_json_escapes_without_reducing_the_body_limit() {
        let mut template = payout();
        template.key = "auction_no_attachments".into();
        template.required_placeholders.clear();
        template.allowed_placeholders.clear();
        assert!(mail_template_body_problem(&template, &"\"".repeat(50_000)).is_some());
        assert_eq!(mail_template_body_problem(&template, &"x".repeat(64 * 1024)), None);
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
    fn mail_template_read_and_command_contract_fixtures() {
        use super::{
            AdminMailTemplateListResponse, AdminMailTemplateUpdateResponse, ApiResponse,
            valid_template_entry,
        };
        use crate::api::contract_fixtures::{admin_fixture, decode};

        let payload: ApiResponse<AdminMailTemplateListResponse> =
            decode(admin_fixture!("mail-templates"), "/data", "templates");
        assert!(payload.data.templates.iter().all(valid_template_entry));
        let payout = payload
            .data
            .templates
            .iter()
            .find(|entry| entry.key == "settlement_seller_payout")
            .expect("结算模板");
        assert!(payout.customized);
        assert_eq!(payout.revision, 1);

        let command: ApiResponse<AdminMailTemplateUpdateResponse> =
            decode(admin_fixture!("mail-template-command"), "/data/template", "revision");
        let accepted = command.data.template;
        assert!(valid_template_entry(&accepted));
        assert_eq!(accepted.key, "settlement_seller_payout");
        assert_eq!(accepted.revision, 2);
        assert!(!accepted.customized);
        assert_eq!(accepted.body, accepted.default_body);
    }
}
