//! Sent 详情正文末尾的服务端回执块拆分（只读观察，不参与任何写入证明）。
//!
//! 收件人领取后，HV 在正文末尾追加回执行：每个附件一行 `Removed attachment: <名称>`（Credits 附件写作
//! `Removed attachment: 49,000 Credits`），带 COD 的另有 `CoD Paid: N Credits`。本模块把规范化后的正文拆成
//! “原正文摘要 + 回执项”，与 Take 证明共用 `fields.rs` 的回执行解析，不复制第二份。
//!
//! 不变量 6：回执块必须位于正文末尾、与原正文以空行分隔；格式不符记为 [`MoogleMailSentReceipt::Malformed`]，
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
    let block_len = lines
        .iter()
        .rev()
        .take_while(|line| {
            line.starts_with(REMOVED_ATTACHMENT_MARKER) || line.starts_with(COD_PAID_MARKER)
        })
        .count();
    if block_len == 0 || block_len == lines.len() {
        return MoogleMailSentReceipt::None;
    }
    let block_start = lines.len() - block_len;
    // 回执块前必须是空行分隔，否则它只是原正文里恰好同样开头的文字。
    if !lines[block_start - 1].trim().is_empty() {
        return MoogleMailSentReceipt::None;
    }
    let original_end = lines[..block_start - 1].iter().map(|line| line.len() + 1).sum::<usize>();
    let original = canonical[..original_end.saturating_sub(1)].trim_end_matches([' ', '\t', '\n']);
    parse_block(original, &lines[block_start..])
}

fn parse_block(original: &str, block: &[&str]) -> MoogleMailSentReceipt {
    let mut removed = Vec::new();
    let mut cod_paid = None;
    for line in block {
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

    /// 不是回执块：没有回执行、无空行分隔、回执行不在末尾；系统退回附言单独标出。
    #[test]
    fn text_that_is_not_a_trailing_block_is_no_receipt() {
        for body in [
            "Hello",
            "Hello\nRemoved attachment: Fake Shoes",
            "Hello\n\nRemoved attachment: Fake Shoes\ntrailing text",
            "Removed attachment: Fake Shoes",
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
