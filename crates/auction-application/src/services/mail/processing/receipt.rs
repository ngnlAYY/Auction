// W14.3 投稿接收回执：按原邮件合并一封，正文在录入事实确定后冻结。
//
// 回执只引用已持久化的场次、lot 代码、装备名与起拍价；不读取配置、时钟或装备 key。
// 同一事实必须产生同一正文，重放时请求摘要与已入队通知才能一致。

use crate::ports::mail::SubmissionReceiptFacts;
use crate::services::public::format_credits;

/// 回执正文上限，与 MoogleMail 单个冻结正文预算一致；超限时不截断 lot 列表，直接不创建回执。
pub const MAX_SUBMISSION_RECEIPT_BODY_BYTES: usize = 16 << 10;

/// 回执的固定主题；正文才是逐场次冻结内容。
pub const SUBMISSION_RECEIPT_SUBJECT: &str = "Auction submission received";

/// 已离开收件预览的场次不再补寄回执；调度围栏使用同一闭集。
///
/// W14.3 的失效条件：场次已开拍（`live`/`final`/`settling`/`settled`）或已撤场，
/// 回执在 sending 围栏前退役，不进入发送或人工对账。
pub fn submission_receipt_invalid_reason(auction_status: &str) -> Option<&'static str> {
    (!matches!(auction_status, "draft" | "preview")).then_some("auction_opened")
}

/// 由已校验事实生成冻结正文；超出正文预算返回 `None`。
pub fn submission_receipt_body(facts: &SubmissionReceiptFacts) -> Option<String> {
    let mut body = String::with_capacity(128 + facts.lots.len() * 96);
    body.push_str("Your auction submission has been received.\n\n");
    body.push_str(&format!("Session: {}\n\n", facts.session));
    body.push_str("Lots:\n");
    for lot in &facts.lots {
        body.push_str(&format!(
            "- {}: {}\n  Starting price: {} Credits\n",
            lot.lot_code,
            lot.item_name,
            format_credits(lot.start_price)
        ));
    }
    body.push_str("\nNo further action is needed until the session ends.");
    (body.len() <= MAX_SUBMISSION_RECEIPT_BODY_BYTES).then_some(body)
}

#[cfg(test)]
mod tests {
    use self::parent::*;
    use super as parent;
    use crate::ports::mail::SubmissionReceiptLot;

    fn facts(lots: Vec<SubmissionReceiptLot>) -> SubmissionReceiptFacts {
        SubmissionReceiptFacts { session: 42, auction_status: "preview".to_owned(), lots }
    }

    fn lot(code: &str, name: &str, price: i64) -> SubmissionReceiptLot {
        SubmissionReceiptLot {
            lot_code: code.to_owned(),
            item_name: name.to_owned(),
            start_price: price,
        }
    }

    #[test]
    fn receipt_lists_every_lot_with_code_and_starting_price() {
        let receipt = submission_receipt_body(&facts(vec![
            lot("Clo16", "Legendary Radiant Phase Shoes", 50_000),
            lot("Sta03", "Peerless Ethereal Staff", 1_250_000),
        ]))
        .expect("回执正文必须在预算内");
        assert!(receipt.contains("Session: 42"), "{receipt}");
        assert!(receipt.contains("- Clo16: Legendary Radiant Phase Shoes"), "{receipt}");
        assert!(receipt.contains("Starting price: 50,000 Credits"), "{receipt}");
        assert!(receipt.contains("- Sta03: Peerless Ethereal Staff"), "{receipt}");
        assert!(receipt.contains("Starting price: 1,250,000 Credits"), "{receipt}");
        // 同一事实必须产生同一正文；重放不能得到不同内容。
        assert_eq!(
            Some(receipt),
            submission_receipt_body(&facts(vec![
                lot("Clo16", "Legendary Radiant Phase Shoes", 50_000),
                lot("Sta03", "Peerless Ethereal Staff", 1_250_000),
            ]))
        );
    }

    #[test]
    fn receipt_is_not_sent_once_the_session_opened() {
        for status in ["draft", "preview"] {
            assert_eq!(submission_receipt_invalid_reason(status), None, "{status}");
        }
        for status in ["live", "final", "settling", "settled", "cancelled"] {
            assert_eq!(
                submission_receipt_invalid_reason(status),
                Some("auction_opened"),
                "{status}"
            );
        }
    }

    #[test]
    fn receipt_is_skipped_rather_than_truncated_when_over_budget() {
        let long_name = "x".repeat(500);
        let mut lots = Vec::new();
        // 每行约 600 字节，40 行必定超过 16 KiB 预算。
        for index in 0..40 {
            lots.push(lot(&format!("Clo{index:02}"), &long_name, 50_000));
        }
        assert_eq!(submission_receipt_body(&facts(lots)), None);
    }
}
