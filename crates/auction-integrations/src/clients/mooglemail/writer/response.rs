//! 最终发送响应的成功证明和明确失败分类。

use std::str;

use scraper::Html;

use super::common::{
    compose_error, response_mail_id, selector, visible_body_text, visible_confirmation_text,
};
use super::*;

impl MoogleMailParser {
    /// 证明最终发送响应成功，并尽可能提取远端邮件 ID。
    ///
    /// 返回 None 只表示成功页面没有暴露唯一 mid，不表示发送失败；调用方仍应保存整个
    /// outbox 的 request digest 并进入后续 sent/reconciliation 流程。
    pub fn prove_send_completed(
        &self,
        document: &[u8],
        world: MoogleMailWorld,
    ) -> Result<Option<String>, MoogleMailError> {
        let source = str::from_utf8(document)
            .map_err(|_| compose_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
        if source.trim().is_empty() {
            return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
        let parsed = Html::parse_document(source);
        let confirmation_selector = selector("#mmail_fin", world)?;
        let form_selector = selector("form#mailform", world)?;
        let confirmations = parsed.select(&confirmation_selector).collect::<Vec<_>>();
        let forms = parsed.select(&form_selector).count();
        if confirmations.len() > 1 {
            return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
        if confirmations.len() == 1 && forms == 0 {
            let phrase = visible_confirmation_text(confirmations[0], world)?;
            if !phrase.to_ascii_lowercase().contains("message has been sent") {
                return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
            }
            return Ok(response_mail_id(source));
        }

        let state = self.parse_compose_state(document, world)?;
        if !state.is_clean() {
            return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
        Ok(response_mail_id(source))
    }

    /// 只从可见文字识别旧版冻结的明确发送失败。
    ///
    /// script/style/noscript/textarea 中的诱饵短语不参与分类；已有 `#mmail_fin` 的页面交给
    /// 成功证明处理，不会被误分类为失败。
    pub fn classify_send_response_failure(
        &self,
        document: &[u8],
        world: MoogleMailWorld,
    ) -> Result<Option<MoogleMailSendResponseFailure>, MoogleMailError> {
        let source = str::from_utf8(document)
            .map_err(|_| compose_error(world, MoogleMailErrorCategory::ProtocolDrift))?;
        if source.trim().is_empty() {
            return Err(compose_error(world, MoogleMailErrorCategory::ProtocolDrift));
        }
        let parsed = Html::parse_document(source);
        let body_selector = selector("body", world)?;
        let confirmation_selector = selector("#mmail_fin", world)?;
        let bodies = parsed.select(&body_selector).collect::<Vec<_>>();
        if bodies.len() != 1 || parsed.select(&confirmation_selector).next().is_some() {
            return Ok(None);
        }
        let visible = visible_body_text(bodies[0]).to_ascii_lowercase();
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
}
