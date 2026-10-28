//! 最终发送响应的成功证明和明确失败分类。

use std::str;

use scraper::Html;

use super::common::{
    compose_error, is_css_send_confirmation, response_mail_id, selector, visible_body_text,
    visible_confirmation_text,
};
use super::*;
use crate::clients::mooglemail::parser::optional_unique_element;

impl MoogleMailParser {
    /// 解析最终发送响应的成功提示，并尽可能提取远端邮件 ID。
    /// 返回 None 表示提示页未暴露唯一 mid；提示缺失或解析失败也不能替代独立 Sent 证明。
    pub fn prove_send_completed(
        &self,
        document: &[u8],
        world: MoogleMailWorld,
    ) -> Result<Option<String>, MoogleMailError> {
        let (source, parsed) = parse_response(document, world)?;
        prove_send_completed(source, &parsed, world)
    }

    /// 只从唯一系统提示区的可见协议文字识别明确发送失败。
    /// script/style/noscript/textarea 中的诱饵短语不参与分类；已有 `#mmail_fin` 的页面交给
    /// 成功证明处理，不会被误分类为失败。
    pub fn classify_send_response_failure(
        &self,
        document: &[u8],
        world: MoogleMailWorld,
    ) -> Result<Option<MoogleMailSendResponseFailure>, MoogleMailError> {
        let (source, parsed) = parse_response(document, world)?;
        classify_send_response_failure(source, &parsed, world)
    }

    /// 一次解析发送响应，分别保留明确拒绝和成功提示的原有判定与错误。
    pub(crate) fn inspect_send_response(
        &self,
        document: &[u8],
        world: MoogleMailWorld,
    ) -> (
        Result<Option<MoogleMailSendResponseFailure>, MoogleMailError>,
        Result<Option<String>, MoogleMailError>,
    ) {
        let (source, parsed) = match parse_response(document, world) {
            Ok(value) => value,
            Err(error) => return (Err(error), Err(error)),
        };
        let failure = classify_send_response_failure(source, &parsed, world);
        let confirmation = if matches!(&failure, Ok(Some(_))) {
            Ok(None)
        } else {
            prove_send_completed(source, &parsed, world)
        };
        (failure, confirmation)
    }
}

fn parse_response(
    document: &[u8],
    world: MoogleMailWorld,
) -> Result<(&str, Html), MoogleMailError> {
    let source = str::from_utf8(document)
        .map_err(|_| compose_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
    if source.trim().is_empty() {
        return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
    }
    Ok((source, Html::parse_document(source)))
}

fn prove_send_completed(
    source: &str,
    parsed: &Html,
    world: MoogleMailWorld,
) -> Result<Option<String>, MoogleMailError> {
    let confirmation_selector = selector("#mmail_fin", world)?;
    if let Some(confirmation) = optional_unique_element(
        parsed.select(&confirmation_selector),
        MoogleMailOperation::Compose,
        world,
    )? {
        let phrase = visible_confirmation_text(confirmation, world)?;
        if phrase.to_ascii_lowercase().contains("message has been sent")
            || is_css_send_confirmation(confirmation)
        {
            return Ok(response_mail_id(source));
        }
    }
    Err(compose_error(world, MoogleMailErrorCategory::WriteUnknown))
}

fn classify_send_response_failure(
    source: &str,
    parsed: &Html,
    world: MoogleMailWorld,
) -> Result<Option<MoogleMailSendResponseFailure>, MoogleMailError> {
    let body_selector = selector("body", world)?;
    let confirmation_selector = selector("#mmail_fin", world)?;
    if parsed.select(&body_selector).take(2).count() != 1
        || parsed.select(&confirmation_selector).next().is_some()
    {
        return Ok(None);
    }
    // 所有拒绝提示都只采信系统消息，不把装备名称或其他可控可见文字当作拒绝证明。
    let message_selector = selector("#messagebox_inner", world)?;
    let mut messages = parsed.select(&message_selector);
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
            returned_mail_id: response_mail_id(source),
        }));
    }
    if visible.contains("cannot afford the postage") {
        return Ok(Some(MoogleMailSendResponseFailure::Rejected));
    }
    Ok(None)
}
