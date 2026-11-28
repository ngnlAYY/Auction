//! 最终发送响应的成功证明和明确失败分类。

use std::str;

use scraper::Html;

use super::model::MoogleMailSendResponseFailure;
use super::page_text::{
    compose_error, compose_selector, is_css_send_confirmation, response_mail_id, visible_body_text,
    visible_confirmation_text,
};
use crate::clients::mooglemail::parser::optional_unique_element;
use crate::clients::mooglemail::{
    MoogleMailError, MoogleMailErrorCategory, MoogleMailOperation, MoogleMailParser,
    MoogleMailWorld,
};
use crate::safe_diagnostics;

impl MoogleMailParser {
    /// 一次解析最终发送响应，返回（明确失败分类，成功证明）。
    ///
    /// 明确失败只从唯一系统提示区的可见协议文字识别，script/style/noscript/textarea 中的诱饵
    /// 短语不参与分类；已有 `#mmail_fin` 的页面交给成功证明，不会被误分类为失败。识别出明确
    /// 失败时不再做成功证明，第二项为 `Ok(None)`，调用方须先看第一项。
    /// 成功证明的 `Ok(None)` 表示提示页未暴露唯一 mid；提示缺失或解析失败也不能替代独立 Sent
    /// 证明。生产路径只经发送器调用，crate 外的直接调用方仅限测试。
    pub fn inspect_send_response(
        &self,
        document: &[u8],
        world: MoogleMailWorld,
    ) -> (
        Result<Option<MoogleMailSendResponseFailure>, MoogleMailError>,
        Result<Option<String>, MoogleMailError>,
    ) {
        let parsed = match parse_response(document, world) {
            Ok(value) => value,
            Err(error) => return (Err(error), Err(error)),
        };
        let failure = classify_send_response_failure(&parsed, world);
        let confirmation = if matches!(&failure, Ok(Some(_))) {
            Ok(None)
        } else {
            prove_send_completed(&parsed, world)
        };
        (failure, confirmation)
    }
}

fn parse_response(document: &[u8], world: MoogleMailWorld) -> Result<Html, MoogleMailError> {
    let source = str::from_utf8(document).map_err(safe_diagnostics::warn_map(
        "mooglemail_send",
        "decode_send_document",
        || compose_error(world, MoogleMailErrorCategory::ProtocolDrift),
    ))?;
    if source.trim().is_empty() {
        return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    Ok(Html::parse_document(source))
}

fn prove_send_completed(
    parsed: &Html,
    world: MoogleMailWorld,
) -> Result<Option<String>, MoogleMailError> {
    let confirmation_selector = compose_selector!("#mmail_fin", world)?;
    if let Some(confirmation) = optional_unique_element(
        parsed.select(confirmation_selector),
        MoogleMailOperation::Compose,
        world,
    )? {
        let phrase = visible_confirmation_text(confirmation, world)?;
        if phrase.to_ascii_lowercase().contains("message has been sent")
            || is_css_send_confirmation(confirmation)
        {
            return Ok(response_mail_id(&confirmation.html()));
        }
    }
    Err(compose_error(world, MoogleMailErrorCategory::WriteUnknown))
}

fn classify_send_response_failure(
    parsed: &Html,
    world: MoogleMailWorld,
) -> Result<Option<MoogleMailSendResponseFailure>, MoogleMailError> {
    let body_selector = compose_selector!("body", world)?;
    let confirmation_selector = compose_selector!("#mmail_fin", world)?;
    if parsed.select(body_selector).take(2).count() != 1
        || parsed.select(confirmation_selector).next().is_some()
    {
        return Ok(None);
    }
    // 所有拒绝提示都只采信系统消息，不把装备名称或其他可控可见文字当作拒绝证明。
    let message_selector = compose_selector!("#messagebox_inner", world)?;
    let mut messages = parsed.select(message_selector);
    let Some(message) = messages.next() else { return Ok(None) };
    if messages.next().is_some() {
        return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    let visible = visible_body_text(message);
    if visible.eq_ignore_ascii_case(
        "Messaging yourself must be the ultimate form of social withdrawal, kupo! Seek help, kupo!",
    ) {
        return Ok(Some(MoogleMailSendResponseFailure::RecipientIsSender));
    }
    let visible = visible.to_ascii_lowercase();
    const RECIPIENT_NOT_FOUND_PHRASES: &[&str] = &[
        "recipient not found",
        "recipient does not exist",
        "recipient doesn't exist",
        "no such recipient",
        "player not found",
        "user not found",
        "no such user",
        "could not find recipient",
    ];
    if RECIPIENT_NOT_FOUND_PHRASES.iter().any(|phrase| visible.contains(phrase)) {
        return Ok(Some(MoogleMailSendResponseFailure::RecipientNotFound {
            returned_mail_id: response_mail_id(&message.html()),
        }));
    }
    if visible.contains("cannot afford the postage") {
        return Ok(Some(MoogleMailSendResponseFailure::Rejected));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use scraper::Html;

    use super::{classify_send_response_failure, prove_send_completed};
    use crate::clients::mooglemail::{
        MoogleMailErrorCategory, MoogleMailSendResponseFailure, MoogleMailWorld,
    };

    /// `inspect_send_response` 识别出明确拒绝后跳过成功证明；这里直接调用两个判定，保留
    /// “成功证明拒绝失败页”的独立断言，防止拒绝页被当作发送成功。
    #[test]
    fn rejection_pages_are_never_proven_sent() {
        let world = MoogleMailWorld::Main;
        for (page, expected) in [
            (
                "<div id=\"messagebox_inner\">You cannot afford the postage</div>",
                MoogleMailSendResponseFailure::Rejected,
            ),
            (
                "<div id=\"messagebox_inner\">Recipient not found</div>",
                MoogleMailSendResponseFailure::RecipientNotFound { returned_mail_id: None },
            ),
            (
                "<div id=\"messagebox_inner\">Messaging yourself must be the ultimate form of \
                 social withdrawal, kupo! Seek help, kupo!</div>",
                MoogleMailSendResponseFailure::RecipientIsSender,
            ),
        ] {
            let parsed = Html::parse_document(page);
            assert_eq!(
                classify_send_response_failure(&parsed, world).expect("拒绝页可分类"),
                Some(expected)
            );
            let error = prove_send_completed(&parsed, world).expect_err("拒绝页不得证明成功");
            assert_eq!(error.category, MoogleMailErrorCategory::WriteUnknown);
        }
    }
}
