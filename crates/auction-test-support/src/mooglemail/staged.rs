//! 附 Credits 的 compose 暂存回放（显式开启）：一次 `attach_add` + `select_pane=credits` 的写 POST 之后，
//! 按真实 compose 页回放“已暂存该笔 Credits 的草稿”。页面由已脱敏语料派生，不编造新结构：
//! - 基底是采集的 compose 页（compose-main），只把本次 POST 的收件人、主题与正文填回原有输入框；
//! - 附件区沿用采集的 Sent 详情 3125364 里同一 DOM（`mmail_attachcount` / `mmail_attachlist`）；
//! - 另带一个值为 0 的 `#newcod` 输入框：compose 解析器对已暂存草稿要求它存在（尚未设置 CoD）。
//!
//! 不开启时替身对写 POST 的行为不变。

use std::collections::BTreeMap;

use super::fixtures;
use super::{MailDouble, MailState};

/// 采集的 compose 页里空附件区的起点与终点（帮助文字整块）。
const EMPTY_INFO_START: &str = "<div id=\"mmail_attachinfo\">";
const EMPTY_INFO_END: &str = "</div>\n\t\t</div>";

impl MailDouble {
    /// 开启附 Credits 的 compose 暂存回放（见模块说明）；默认关闭。
    pub fn stage_credits_on_attach(&self) {
        self.state.lock().expect("test state poisoned").stage_credits = true;
    }
}

/// 开启回放且本次写 POST 是附 Credits 的 attach_add 时，返回暂存后的 compose 页；否则 `None`，
/// 由调用方按原路由处理。
pub(super) fn respond(state: &MailState, form: &BTreeMap<String, String>) -> Option<String> {
    if !state.stage_credits
        || form.get("action").map(String::as_str) != Some("attach_add")
        || form.get("select_pane").map(String::as_str) != Some("credits")
    {
        return None;
    }
    let credits = form.get("select_count")?.parse::<u64>().ok()?;
    let page = fixtures::COMPOSE_MAIN;
    let start = page.find(EMPTY_INFO_START)?;
    let end = start + page[start..].find(EMPTY_INFO_END)? + EMPTY_INFO_END.len();
    let attached = format!(
        "<div id=\"mmail_attachinfo\"><div id=\"mmail_attachcount\"><div class=\"fc4 fac fcb\"><div>1 / 10 \
         items attached</div></div></div><div id=\"mmail_attachlist\"><div><div>{} Credits</div><div></div>\
         </div></div><input type=\"text\" id=\"newcod\" value=\"0\" /></div>",
        grouped(credits)
    );
    let field = |name: &str| escape(form.get(name).map_or("", String::as_str));
    let html = format!("{}{}{}", &page[..start], attached, &page[end..])
        .replacen(
            "name=\"message_to_name\" size=\"50\" maxlength=\"50\" value=\"\"",
            &format!(
                "name=\"message_to_name\" size=\"50\" maxlength=\"50\" value=\"{}\"",
                field("message_to_name")
            ),
            1,
        )
        .replacen(
            "name=\"message_subject\" size=\"50\" maxlength=\"50\" value=\"\"",
            &format!(
                "name=\"message_subject\" size=\"50\" maxlength=\"50\" value=\"{}\"",
                field("message_subject")
            ),
            1,
        )
        .replacen(
            "<textarea name=\"message_body\"></textarea>",
            &format!("<textarea name=\"message_body\">{}</textarea>", field("message_body")),
            1,
        );
    Some(html)
}

/// 页面上的千分位金额（如 `49,000`）。
fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

fn escape(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}
