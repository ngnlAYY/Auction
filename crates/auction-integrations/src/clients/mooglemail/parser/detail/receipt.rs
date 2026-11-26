//! Sent 详情正文末尾的服务端回执块拆分（只读观察，不参与任何写入证明）。
//!
//! 收件人领取后，HV 在正文末尾追加回执行：每个附件一行 `Removed attachment: <名称>`（Credits 附件写作
//! `Removed attachment: 49,000 Credits`），带 COD 的另有 `CoD Paid: N Credits`；已签收的原生格式里 CoD 行在前、与附件行之间隔一个空行
//! （`CoD Paid: N Credits\n\nRemoved attachment: <名称>`），所以回执块允许回执行之间夹空行（块内空行忽略）。本模块把规范化后的正文拆成
//! “原正文摘要 + 回执项”，与 Take 证明共用 `fields.rs` 的回执行解析，不复制第二份。
//!
//! 不变量 6：回执块必须位于正文末尾（只含回执行与空行）、与原正文至少隔一个空行；格式不符记为 [`MoogleMailSentReceipt::Malformed`]，
//! 只影响寄售领取判定，绝不让原有 Sent 解析失败。正文中间出现回执样式的行不算回执（原正文摘要比对自然失败）。

use sha2::{Digest, Sha256};

use crate::clients::mooglemail::parser::{
    MoogleMailSentReceipt, REMOVED_ATTACHMENT_MARKER, RETURNED_SUFFIX_PREFIX, canonical_mail_body,
    removed_marker_line, take_cod_receipt_amount,
};

const COD_PAID_MARKER: &str = "CoD Paid:";

/// 拆出末尾回执块，并判断正文是否含系统退回附言；中间产物（规范化正文）用后清零。
pub(crate) fn split_sent_receipt(body: &str) -> (MoogleMailSentReceipt, bool) {
    let mut canonical = canonical_mail_body(body);
    let returned_notice = canonical.contains(RETURNED_SUFFIX_PREFIX);
    let receipt = split_canonical(&canonical);
    let mut bytes = std::mem::take(&mut canonical).into_bytes();
    bytes.fill(0);
    (receipt, returned_notice)
}

fn split_canonical(canonical: &str) -> MoogleMailSentReceipt {
    let lines: Vec<&str> = canonical.split('\n').collect();
    // 从末尾向前收集回执行或空行：已签收的原生格式在 CoD 行与附件行之间隔一个空行。
    let tail_len = lines
        .iter()
        .rev()
        .take_while(|line| {
            line.trim().is_empty()
                || line.starts_with(REMOVED_ATTACHMENT_MARKER)
                || line.starts_with(COD_PAID_MARKER)
        })
        .count();
    let tail = &lines[lines.len() - tail_len..];
    // 块开头的空行是与原正文的分隔，必须至少有一个；块内至少一条回执行。
    let separator = tail.iter().take_while(|line| line.trim().is_empty()).count();
    if separator == 0 || separator == tail.len() || separator == lines.len() {
        return MoogleMailSentReceipt::None;
    }
    let block_start = lines.len() - tail_len + separator;
    let original_end =
        lines[..block_start - separator].iter().map(|line| line.len() + 1).sum::<usize>();
    let original = canonical[..original_end.saturating_sub(1)].trim_end_matches([' ', '\t', '\n']);
    parse_block(original, &lines[block_start..])
}

fn parse_block(original: &str, block: &[&str]) -> MoogleMailSentReceipt {
    let mut removed = Vec::new();
    let mut cod_paid = None;
    for line in block {
        if line.trim().is_empty() {
            continue;
        }
        if line.starts_with(REMOVED_ATTACHMENT_MARKER) {
            let Some(marker) = removed_marker_line(line) else {
                return MoogleMailSentReceipt::Malformed;
            };
            if removed.contains(&marker) {
                return MoogleMailSentReceipt::Malformed;
            }
            removed.push(marker);
        } else {
            let Some(amount) = take_cod_receipt_amount(line) else {
                return MoogleMailSentReceipt::Malformed;
            };
            if cod_paid.replace(amount).is_some() {
                return MoogleMailSentReceipt::Malformed;
            }
        }
    }
    MoogleMailSentReceipt::Parsed {
        original_body_digest: Sha256::digest(original.as_bytes()).into(),
        removed,
        cod_paid,
    }
}

#[cfg(test)]
mod tests {
    use auction_domain::consignment::RemovedMarker;
    use sha2::{Digest, Sha256};

    use super::split_sent_receipt;
    use crate::clients::mooglemail::parser::MoogleMailSentReceipt;

    fn digest(text: &str) -> [u8; 32] {
        Sha256::digest(text.as_bytes()).into()
    }

    fn marker(name: &str, eid: Option<&str>) -> RemovedMarker {
        RemovedMarker { name: name.to_owned(), canonical_eid: eid.map(str::to_owned) }
    }

    /// 空行分隔的末尾回执块：拆出原正文摘要、装备 / Credits 回执与 CoD Paid；CRLF 同样规范。
    #[test]
    fn trailing_block_splits_into_original_digest_and_receipts() {
        let body = "Hello\r\n\r\nSecond paragraph\n\nRemoved attachment: [42] Fake Shoes\n\
                    Removed attachment: 49,000 Credits\nCoD Paid: 49,000 Credits\n";
        let (receipt, returned) = split_sent_receipt(body);
        assert!(!returned);
        assert_eq!(
            receipt,
            MoogleMailSentReceipt::Parsed {
                original_body_digest: digest("Hello\n\nSecond paragraph"),
                removed: vec![marker("Fake Shoes", Some("42")), marker("49,000 Credits", None)],
                cod_paid: Some(49_000),
            }
        );
    }

    /// 已签收的原生格式：CoD 行在前、空行、附件行；原正文摘要只含原正文。
    #[test]
    fn cod_line_then_blank_then_attachment_is_one_block() {
        let body = "Hello\n\nSecond paragraph\n\nCoD Paid: 100 Credits\n\nRemoved attachment: [42] Fake Shoes";
        assert_eq!(
            split_sent_receipt(body).0,
            MoogleMailSentReceipt::Parsed {
                original_body_digest: digest("Hello\n\nSecond paragraph"),
                removed: vec![marker("Fake Shoes", Some("42"))],
                cod_paid: Some(100),
            }
        );
    }

    /// 不是回执块：没有回执行、无空行分隔、回执行不在末尾；系统退回附言单独标出。
    #[test]
    fn text_that_is_not_a_trailing_block_is_no_receipt() {
        for body in [
            "Hello",
            "Hello\nRemoved attachment: Fake Shoes",
            "Hello\n\nRemoved attachment: Fake Shoes\ntrailing text",
            "Removed attachment: Fake Shoes",
            "Hello\n\nRemoved attachment: Fake Shoes\n\ntrailing text",
        ] {
            assert_eq!(split_sent_receipt(body).0, MoogleMailSentReceipt::None, "{body:?}");
        }
        let (_, returned) =
            split_sent_receipt("Hello\n\nThis message was returned from Fake, kupo!");
        assert!(returned);
    }

    /// 格式不符记为 malformed：CoD Paid 两行、金额格式不符、名称为空、marker 重复。
    #[test]
    fn broken_receipt_lines_are_malformed() {
        for block in [
            "CoD Paid: 5 Credits\nCoD Paid: 5 Credits",
            "CoD Paid: five Credits",
            "CoD Paid:5 Credits",
            "Removed attachment:   ",
            "Removed attachment: A\nRemoved attachment: A",
        ] {
            let body = format!("Hello\n\n{block}");
            assert_eq!(split_sent_receipt(&body).0, MoogleMailSentReceipt::Malformed, "{block:?}");
        }
    }
}
