//! 只提交当前登录页的封闭字段集合；未知隐藏控件、验证码和额外验证均失败关闭。

use auction_application::core::diagnostics::DiagnosticContext;

use crate::safe_diagnostics;

use auction_application::admin::{AdminCredentialError, AdminCredentialLogin};
use auction_application::core::ApplicationError;
use reqwest::StatusCode;
use scraper::{Html, Selector};
use url::Url;

use super::{SecretBuffer, unavailable};

const HIDDEN_NAMES: [&str; 3] = ["referer", "b", "bt"];
const MAXIMUM_FIELD_BYTES: usize = 4096;

pub(super) struct LoginForm {
    hidden: [SecretBuffer; 3],
    submit_url: Url,
}

impl LoginForm {
    pub(super) fn encode(
        self,
        login: &AdminCredentialLogin,
    ) -> Result<(Url, Vec<u8>), ApplicationError> {
        let password = std::str::from_utf8(login.password()).map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "credential_login",
                "encode_password",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            unavailable()
        })?;
        let field_bytes = self.hidden.iter().map(|field| field.0.len()).sum::<usize>()
            + login.username().len()
            + password.len();
        // 按最坏百分号编码长度预留，避免扩容在旧分配中留下密码副本。
        let mut encoded =
            url::form_urlencoded::Serializer::new(String::with_capacity(field_bytes * 3 + 128));
        for (name, value) in HIDDEN_NAMES.iter().zip(&self.hidden) {
            let value = std::str::from_utf8(&value.0).map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "credential_login",
                    "encode_hidden_fields",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                unavailable()
            })?;
            encoded.append_pair(name, value);
        }
        encoded.append_pair("UserName", login.username());
        encoded.append_pair("PassWord", password);
        encoded.append_pair("CookieDate", "1");
        // Privacy 明确不勾选；submit 不是认证字段，不发送；不执行页面脚本。
        Ok((self.submit_url, encoded.finish().into_bytes()))
    }
}

pub(super) fn parse(
    body: &[u8],
    page_url: &Url,
    submit_url: &Url,
) -> Result<LoginForm, ApplicationError> {
    let source = std::str::from_utf8(body).map_err(|diagnostic_cause| {
        safe_diagnostics::warn(
            "credential_login",
            "decode_login_page",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        unavailable()
    })?;
    let document = Html::parse_document(source);
    if challenge(&document)? {
        return Err(AdminCredentialError::LoginChallenge.into());
    }
    let forms = selector("form")?;
    let fields = selector("input, select, textarea, button")?;
    let mut candidates = document.select(&forms).filter(|form| {
        form.select(&fields)
            .any(|field| matches!(field.attr("name"), Some("UserName" | "PassWord")))
    });
    let form = candidates.next().ok_or_else(unavailable)?;
    let action = form
        .attr("action")
        .filter(|value| value.len() <= MAXIMUM_FIELD_BYTES)
        .and_then(|value| page_url.join(value).ok())
        .ok_or_else(unavailable)?;
    if candidates.next().is_some()
        || !form.attr("method").is_some_and(|value| value.eq_ignore_ascii_case("post"))
        || form
            .attr("enctype")
            .is_some_and(|value| !value.eq_ignore_ascii_case("application/x-www-form-urlencoded"))
        || !safe_login_target(&action, submit_url)
    {
        return Err(unavailable());
    }
    let mut hidden: [Option<SecretBuffer>; 3] = [None, None, None];
    let mut username = false;
    let mut password = false;
    let mut cookie_date = false;
    let mut privacy = false;
    for (index, field) in form.select(&fields).enumerate() {
        if index >= 24 || field.attr("disabled").is_some() || field.attr("form").is_some() {
            return Err(unavailable());
        }
        let name = field.attr("name").unwrap_or_default();
        let tag = field.value().name();
        let kind = field.attr("type").unwrap_or(if tag == "button" { "submit" } else { "text" });
        if matches!(name, "" | "submit")
            && matches!(tag, "input" | "button")
            && kind.eq_ignore_ascii_case("submit")
        {
            continue;
        }
        if tag != "input" {
            return Err(unavailable());
        }
        if let Some(index) = HIDDEN_NAMES.iter().position(|candidate| *candidate == name) {
            // HTML 未声明 value 时该控件的当前值就是空字符串，仍以本次表单为准。
            let value = field.attr("value").unwrap_or_default();
            if hidden[index].is_some()
                || !kind.eq_ignore_ascii_case("hidden")
                || value.len() > MAXIMUM_FIELD_BYTES
                || value.chars().any(char::is_control)
                || (index == 0 && !safe_referer(page_url, value))
            {
                return Err(unavailable());
            }
            hidden[index] = Some(SecretBuffer(value.as_bytes().to_vec()));
            continue;
        }
        let (seen, expected_kind) = match name {
            "UserName" => (&mut username, "text"),
            "PassWord" => (&mut password, "password"),
            "CookieDate" => (&mut cookie_date, "checkbox"),
            "Privacy" => (&mut privacy, "checkbox"),
            _ => return Err(unavailable()),
        };
        if *seen
            || !kind.eq_ignore_ascii_case(expected_kind)
            || (matches!(name, "CookieDate" | "Privacy") && field.attr("value") != Some("1"))
        {
            return Err(unavailable());
        }
        *seen = true;
    }
    if !username || !password || !cookie_date {
        return Err(unavailable());
    }
    let [referer, b, bt] = hidden;
    Ok(LoginForm {
        hidden: [
            referer.ok_or_else(unavailable)?,
            b.ok_or_else(unavailable)?,
            bt.ok_or_else(unavailable)?,
        ],
        submit_url: action,
    })
}

pub(super) fn check_response(
    body: &[u8],
    status: StatusCode,
    submitted: bool,
) -> Result<(), ApplicationError> {
    let source = std::str::from_utf8(body).map_err(|diagnostic_cause| {
        safe_diagnostics::warn(
            "credential_login",
            "decode_login_response",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        unavailable()
    })?;
    let document = Html::parse_document(source);
    if challenge(&document)? {
        return Err(AdminCredentialError::LoginChallenge.into());
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        return Err(AdminCredentialError::LoginBusy.into());
    }
    if submitted && (status == StatusCode::UNAUTHORIZED || rejected(&document)?) {
        return Err(AdminCredentialError::LoginRejected.into());
    }
    if status != StatusCode::OK {
        return Err(unavailable());
    }
    Ok(())
}

fn safe_referer(page_url: &Url, value: &str) -> bool {
    let Ok(url) = page_url.join(value) else { return false };
    url.origin() == page_url.origin()
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/index.php"
        && login_query(&url, "00", true)
        && url.fragment().is_none()
}

/// 无既有 Cookie 的真实登录页会把短时 session 放在 action 的 s 参数中；不能丢弃或接受其它动作。
fn safe_login_target(url: &Url, expected: &Url) -> bool {
    url.origin() == expected.origin()
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == expected.path()
        && url.fragment().is_none()
        && login_query(url, "01", false)
}

/// referer 允许站点自身产生的双重 HTML 转义 act 名称；按表单原文传回，不二次解码或跟随。
fn login_query(url: &Url, code: &str, referer: bool) -> bool {
    let mut seen = [false; 3];
    for (name, value) in url.query_pairs() {
        let index = match name.as_ref() {
            "act" if value == "Login" => 0,
            "amp;act" if referer && value == "Login" => 0,
            "CODE" if value == code => 1,
            "s" if value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) => 2,
            _ => return false,
        };
        if seen[index] {
            return false;
        }
        seen[index] = true;
    }
    if referer { seen[0] == seen[1] } else { seen[0] && seen[1] }
}

fn challenge(document: &Html) -> Result<bool, ApplicationError> {
    let controls = selector("[name], [id], [class], iframe[src], [autocomplete]")?;
    for element in document.select(&controls) {
        for attribute in ["name", "id", "class", "src"] {
            if element.attr(attribute).is_some_and(|value| {
                ["captcha", "turnstile", "cf-chl-", "challenge-form", "challenge-stage"]
                    .iter()
                    .any(|needle| contains_ascii(value, needle))
                    || matches!(value, "otp" | "totp" | "security_code")
            }) {
                return Ok(true);
            }
        }
        if element.attr("autocomplete") == Some("one-time-code") {
            return Ok(true);
        }
    }
    let titles = selector("title")?;
    Ok(document.select(&titles).any(|title| {
        title.text().any(|text| {
            contains_ascii(text, "just a moment") || contains_ascii(text, "attention required")
        })
    }))
}

fn rejected(document: &Html) -> Result<bool, ApplicationError> {
    // 只认系统错误区域中的明确拒绝文案；缺少 Cookie、任意 HTTP 200 或普通正文都不是密码错误证据。
    let errors =
        selector(".errorwrap, .error, #error, #error-message, .errormessage, .tablepad, .red")?;
    Ok(document.select(&errors).any(|element| {
        element.text().any(|text| {
            [
                "password was wrong",
                "incorrect password",
                "invalid username or password",
                "username or password is incorrect",
                "could not find a member called",
            ]
            .iter()
            .any(|needle| contains_ascii(text, needle))
        })
    }))
}

fn contains_ascii(value: &str, needle: &str) -> bool {
    value
        .as_bytes()
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

fn selector(value: &str) -> Result<Selector, ApplicationError> {
    Selector::parse(value).map_err(|diagnostic_cause| {
        safe_diagnostics::warn(
            "credential_login",
            "build_selector",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        unavailable()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use auction_application::admin::AdminCredentialLogin;

    fn page_url() -> Url {
        Url::parse("https://forums.e-hentai.org/index.php").unwrap()
    }

    fn session() -> String {
        "0123456789abcdef0123456789abcdef".to_owned()
    }

    fn login_page(method: &str, action: &str, fields: &str) -> String {
        format!(
            r#"<html><head><title>Login</title></head><body><form method="{method}" action="{action}">{fields}</form></body></html>"#
        )
    }

    fn valid_fields() -> &'static str {
        r#"<input type="hidden" name="referer" value="/index.php?act=Login&amp;CODE=00">
<input type="hidden" name="b" value="">
<input type="hidden" name="bt" value="">
<input type="text" name="UserName">
<input type="password" name="PassWord">
<input type="checkbox" name="CookieDate" value="1">"#
    }

    fn login() -> AdminCredentialLogin {
        AdminCredentialLogin::new("fixture-user".to_owned(), "fixture-password".to_owned())
            .unwrap_or_else(|error| panic!("测试凭据必须合法: {error:?}"))
    }

    #[test]
    fn parses_the_closed_login_form_and_encodes_only_known_fields() {
        let action = format!("index.php?act=Login&CODE=01&s={}", session());
        let body = login_page("POST", &action, valid_fields());
        let form = parse(body.as_bytes(), &page_url(), &page_url())
            .unwrap_or_else(|error| panic!("真实登录页结构必须可解析: {error:?}"));

        let (url, encoded) = form
            .encode(&login())
            .unwrap_or_else(|error| panic!("合法登录表单必须可编码: {error:?}"));
        let expected = page_url()
            .join(&action)
            .unwrap_or_else(|error| panic!("测试动作必须可解析: {error:?}"));
        assert_eq!(url, expected);
        let encoded = String::from_utf8(encoded)
            .unwrap_or_else(|error| panic!("表单编码必须是 UTF-8: {error:?}"));
        assert!(encoded.contains("UserName=fixture-user"));
        assert!(encoded.contains("PassWord=fixture-password"));
        assert!(encoded.contains("CookieDate=1"));
        assert!(encoded.contains("referer=%2Findex.php%3Fact%3DLogin%26CODE%3D00"));
        // Privacy 明确不提交，页面 submit 按钮也不作为认证字段。
        assert!(!encoded.contains("Privacy"));
        assert!(!encoded.contains("submit"));
    }

    #[test]
    fn rejects_challenge_pages_wrong_actions_and_extra_or_duplicate_fields() {
        let action = format!("index.php?act=Login&CODE=01&s={}", session());
        let challenge = r#"<html><head><title>Just a moment...</title></head><body><form method="post" action="index.php?act=Login&CODE=01"><input type="text" name="UserName"><input type="password" name="PassWord"><input type="checkbox" name="CookieDate" value="1"></form></body></html>"#;
        assert!(parse(challenge.as_bytes(), &page_url(), &page_url()).is_err());

        let wrong_code = login_page("POST", "index.php?act=Login&CODE=02", valid_fields());
        assert!(parse(wrong_code.as_bytes(), &page_url(), &page_url()).is_err());

        let get_form = login_page("GET", &action, valid_fields());
        assert!(parse(get_form.as_bytes(), &page_url(), &page_url()).is_err());

        let missing_referer = login_page(
            "POST",
            &action,
            r#"<input type="hidden" name="b" value=""><input type="hidden" name="bt" value=""><input type="text" name="UserName"><input type="password" name="PassWord"><input type="checkbox" name="CookieDate" value="1">"#,
        );
        assert!(parse(missing_referer.as_bytes(), &page_url(), &page_url()).is_err());

        let unknown_field = login_page(
            "POST",
            &action,
            &format!("{}<input type=\"hidden\" name=\"extra\" value=\"123456\">", valid_fields()),
        );
        assert!(parse(unknown_field.as_bytes(), &page_url(), &page_url()).is_err());

        let duplicate = format!(
            r#"<html><body><form method="post" action="{action}">{}</form><form method="post" action="{action}">{}</form></body></html>"#,
            valid_fields(),
            valid_fields()
        );
        assert!(parse(duplicate.as_bytes(), &page_url(), &page_url()).is_err());
    }

    #[test]
    fn login_response_requires_explicit_rejection_or_challenge_evidence() {
        // 普通 200 登录页本身不能证明密码错误。
        assert!(
            check_response(
                br#"<html><body><div class="tablepad">Welcome back</div></body></html>"#,
                StatusCode::OK,
                true
            )
            .is_ok()
        );
        assert_eq!(
            check_response(b"", StatusCode::TOO_MANY_REQUESTS, true).unwrap_err().to_string(),
            AdminCredentialError::LoginBusy.to_string()
        );
        assert_eq!(
            check_response(b"", StatusCode::UNAUTHORIZED, true).unwrap_err().to_string(),
            AdminCredentialError::LoginRejected.to_string()
        );
        assert!(
            check_response(
                br#"<html><body><div class="errorwrap">Password was wrong</div></body></html>"#,
                StatusCode::OK,
                true
            )
            .is_err()
        );
        assert!(check_response(b"", StatusCode::OK, true).is_ok());
    }
}
