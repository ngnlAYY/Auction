//! 新鲜 MID 表单解析；姓名来自远端当前映射，不能接受数据库旧姓名或任意隐藏动作。

use std::collections::BTreeMap;

use auction_application::forum::{ForumPmError, ForumPmMessage};
use scraper::Html;
use url::Url;

use super::protocol::read_link;
use super::protocol::{selector, sender_identity};

pub(super) struct PmForm {
    pub(super) action: Url,
    pub(super) referer: Url,
    pub(super) values: Vec<(String, String)>,
}

pub(super) fn parse(
    source: &str,
    target: &Url,
    message: &ForumPmMessage,
) -> Result<(String, PmForm), ForumPmError> {
    let document = Html::parse_document(source);
    let sender = sender_identity(&document, target)?;
    let forms = selector("form")?;
    let mut matches = document
        .select(&forms)
        .filter(|form| form.attr("name") == Some("REPLIER") || form.attr("id") == Some("REPLIER"));
    let form = matches.next().ok_or(ForumPmError::Protocol)?;
    if matches.next().is_some()
        || !form.attr("method").is_some_and(|v| v.eq_ignore_ascii_case("post"))
        || form
            .attr("enctype")
            .is_some_and(|v| !v.eq_ignore_ascii_case("application/x-www-form-urlencoded"))
        || form.attr("target").is_some_and(|v| !v.is_empty() && v != "_self")
    {
        return Err(ForumPmError::Protocol);
    }
    let (action, pairs) =
        read_link(target, form.attr("action").ok_or(ForumPmError::Protocol)?, &["act"])?;
    if pairs.len() != 1 || pairs.get("act").map(String::as_str) != Some("msg") {
        return Err(ForumPmError::Protocol);
    }
    let mut fields = BTreeMap::new();
    let controls = selector("input[name], select[name], textarea[name], button[name]")?;
    for control in form.select(&controls) {
        let name = control.attr("name").ok_or(ForumPmError::Protocol)?;
        let value = control.attr("value").unwrap_or_default();
        let kind = control.attr("type").unwrap_or("text");
        if control.attr("disabled").is_some()
            || control.attr("formaction").is_some()
            || control.attr("form").is_some()
        {
            return Err(ForumPmError::Protocol);
        }
        // 无名普通按钮只操作编辑器，不会进入手工构造的 POST；未知提交/隐藏动作仍拒绝。
        if name.is_empty()
            && matches!(control.value().name(), "input" | "button")
            && kind.eq_ignore_ascii_case("button")
        {
            continue;
        }
        if fields.len() >= 24 || !super::super::super::forms::valid_form_field(name, value) {
            return Err(ForumPmError::Protocol);
        }
        let valid = match name {
            "act" | "CODE" | "MODE" | "post_key" | "auth_key" | "OID" | "removeattachid" => {
                control.value().name() == "input" && kind.eq_ignore_ascii_case("hidden")
            }
            "entered_name" | "msg_title" => {
                control.value().name() == "input" && kind.eq_ignore_ascii_case("text")
            }
            "Post" => {
                control.value().name() == "textarea" && control.text().all(|v| v.trim().is_empty())
            }
            "from_contact" => {
                control.value().name() == "select"
                    && control.select(&selector("option[value='-']")?).next().is_some()
            }
            "ffont" | "fsize" => editor_select(control)?,
            "add_sent" | "add_tracking" => {
                control.value().name() == "input" && kind.eq_ignore_ascii_case("checkbox")
            }
            "submit" | "preview" | "save" => kind.eq_ignore_ascii_case("submit"),
            _ => false,
        };
        if !valid || fields.insert(name.to_owned(), value.to_owned()).is_some() {
            return Err(ForumPmError::Protocol);
        }
    }
    for (name, expected) in [("act", "Msg"), ("CODE", "04"), ("MODE", "01")] {
        if fields.get(name).map(String::as_str) != Some(expected) {
            return Err(ForumPmError::Protocol);
        }
    }
    for name in ["post_key", "auth_key"] {
        if fields.get(name).is_none_or(std::string::String::is_empty) {
            return Err(ForumPmError::Protocol);
        }
    }
    for name in ["OID", "removeattachid"] {
        if !fields.get(name).is_some_and(|v| v.is_empty() || v == "0") {
            return Err(ForumPmError::Protocol);
        }
    }
    if ["Post", "msg_title", "from_contact"].iter().any(|name| !fields.contains_key(*name))
        || fields
            .get("entered_name")
            .is_none_or(|v| v.trim().is_empty() || v.len() > 256 || v.chars().any(char::is_control))
        || fields.get("msg_title").is_some_and(|v| !v.is_empty())
        || fields.get("submit").map(String::as_str) != Some("Send Message")
    {
        return Err(ForumPmError::InvalidRecipient);
    }
    let mut values = Vec::new();
    for name in
        ["act", "CODE", "MODE", "post_key", "auth_key", "OID", "removeattachid", "entered_name"]
    {
        values.push((name.to_owned(), fields.remove(name).ok_or(ForumPmError::Protocol)?));
    }
    values.extend([
        ("from_contact".to_owned(), "-".to_owned()),
        ("msg_title".to_owned(), message.title.clone()),
        ("Post".to_owned(), message.body.clone()),
        ("add_sent".to_owned(), "yes".to_owned()),
        ("submit".to_owned(), "Send Message".to_owned()),
    ]);
    // 不复制字体/字号、add_tracking、preview、save 或未知操作；表单凭据没有 Debug/持久化表示。
    Ok((sender, PmForm { action, referer: target.clone(), values }))
}

/// 编辑器字体/字号只接受单选且默认首项为 0；它们不是路由字段，不发送到远端。
fn editor_select(control: scraper::ElementRef<'_>) -> Result<bool, ForumPmError> {
    if control.value().name() != "select" || control.attr("multiple").is_some() {
        return Ok(false);
    }
    let options = selector("option")?;
    let mut options = control.select(&options);
    let Some(default) = options.next() else {
        return Ok(false);
    };
    Ok(default.attr("value") == Some("0")
        && default.attr("disabled").is_none()
        && options.all(|option| option.attr("selected").is_none()))
}
