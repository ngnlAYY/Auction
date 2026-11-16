//! 系统退件正文的只读解析。MoogleMail 把原邮件正文原样带回，并在末尾追加
//! `\n\nThis message was returned from <原收件人>, kupo!`；补寄正文的最后一行是版本化派发引用。
//! 这里只拆出原正文的 canonical 摘要、原收件人与派发引用原文，不保存正文本身。

use auction_domain::DISPATCH_REFERENCE_LINE_PREFIX;
use sha2::{Digest, Sha256};

/// 系统退回附加在原正文之后的固定前缀（换行已规范为 LF）。
const RETURNED_SUFFIX_PREFIX: &str = "\n\nThis message was returned from ";

/// 系统退回后缀的结尾。
const RETURNED_SUFFIX_END: &str = ", kupo!";

/// 原收件人名称与派发引用原文的长度上限；超限即不是可信的系统后缀或引用。
const MAX_RETURNED_FROM_BYTES: usize = 256;
const MAX_DISPATCH_REFERENCE_BYTES: usize = 128;

/// 系统退件正文拆出的安全事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReturnedSettlementBody {
    /// 系统后缀写明的原收件人（退回者）名称。
    pub returned_from: String,
    /// 原正文按发送证明同一规则规范化后的 SHA-256：CRLF 归一为 LF，去掉尾部空格、制表符与换行。
    pub body_digest: [u8; 32],
    /// 原正文末行以固定前缀开头时的引用原文（未校验格式，由消费方解析并失败关闭）。
    pub dispatch_reference: Option<String>,
}

/// 解析系统退件正文；没有完整系统后缀、原正文为空或原收件人名称不合法时返回 `None`。
pub fn parse_returned_settlement_body(body: &str) -> Option<ReturnedSettlementBody> {
    let normalized = body.replace("\r\n", "\n");
    let trimmed = normalized.trim_end_matches([' ', '\t', '\n']);
    let without_end = trimmed.strip_suffix(RETURNED_SUFFIX_END)?;
    let (original, returned_from) = without_end.rsplit_once(RETURNED_SUFFIX_PREFIX)?;
    if returned_from.is_empty()
        || returned_from != returned_from.trim()
        || returned_from.len() > MAX_RETURNED_FROM_BYTES
        || returned_from.chars().any(char::is_control)
    {
        return None;
    }
    let original = original.trim_end_matches([' ', '\t', '\n']);
    if original.is_empty() {
        return None;
    }
    let last_line = original.rsplit_once('\n').map_or(original, |(_, line)| line);
    let dispatch_reference = last_line
        .strip_prefix(DISPATCH_REFERENCE_LINE_PREFIX)
        .filter(|value| value.len() <= MAX_DISPATCH_REFERENCE_BYTES)
        .map(str::to_owned);
    Some(ReturnedSettlementBody {
        returned_from: returned_from.to_owned(),
        body_digest: returned_mail_body_digest(original),
        dispatch_reference,
    })
}

/// 与发送围栏冻结的 `body_digest` 同一规范：CRLF 归一为 LF、去掉尾部空格/制表符/换行后取
/// SHA-256。退件原正文据此与原派发的发送事实比较。
pub fn returned_mail_body_digest(body: &str) -> [u8; 32] {
    let normalized = body.replace("\r\n", "\n");
    Sha256::digest(normalized.trim_end_matches([' ', '\t', '\n']).as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::{parse_returned_settlement_body, returned_mail_body_digest};

    const ORIGINAL: &str = "Your auction payout is attached.\n\nItem: Rapier\nAmount: 1,000 Credits\n\nPlease collect the attached Credits.\n\nReference: 5b1c2a9e-1f0d-4c6b-9a7e-3d2f1e0c9b8a";

    /// 首寄退件：拆出原收件人与原正文摘要，没有派发引用。
    #[test]
    fn first_send_return_has_no_dispatch_reference() {
        let body = format!("{ORIGINAL}\n\nThis message was returned from Seller Name, kupo!");
        let parsed = parse_returned_settlement_body(&body).expect("系统后缀完整");
        assert_eq!(parsed.returned_from, "Seller Name");
        assert_eq!(parsed.body_digest, returned_mail_body_digest(ORIGINAL));
        assert_eq!(parsed.dispatch_reference, None);
    }

    /// 补寄退件：末行派发引用原文被拆出；CRLF 与尾部空白不改变摘要。
    #[test]
    fn resend_return_exposes_the_dispatch_reference_line() {
        let original =
            format!("{ORIGINAL}\n\nDispatch reference: v1/2/0d6c1d0c-63c4-5d0b-9a4f-1d7d2f8e9a01");
        let body = format!("{original}\n\nThis message was returned from Seller, kupo!\n")
            .replace('\n', "\r\n");
        let parsed = parse_returned_settlement_body(&body).expect("系统后缀完整");
        assert_eq!(
            parsed.dispatch_reference.as_deref(),
            Some("v1/2/0d6c1d0c-63c4-5d0b-9a4f-1d7d2f8e9a01")
        );
        assert_eq!(parsed.body_digest, returned_mail_body_digest(&original));
        assert_eq!(parsed.returned_from, "Seller");
    }

    /// 缺系统后缀、原正文为空或名称含换行都不是可信的系统退件正文。
    #[test]
    fn incomplete_suffixes_are_rejected() {
        for body in [
            ORIGINAL.to_owned(),
            format!("{ORIGINAL}\n\nThis message was returned from , kupo!"),
            format!("{ORIGINAL}\n\nThis message was returned from Seller"),
            "\n\nThis message was returned from Seller, kupo!".to_owned(),
            format!("{ORIGINAL}\nThis message was returned from Seller, kupo!"),
            format!("{ORIGINAL}\n\nThis message was returned from  Seller, kupo!"),
        ] {
            assert_eq!(parse_returned_settlement_body(&body), None, "{body:?}");
        }
    }
}
