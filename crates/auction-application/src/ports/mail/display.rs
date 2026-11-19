//! MoogleMail 入站邮件的安全展示投影。

use std::{
    collections::{HashSet, hash_map::RandomState},
    fmt,
    hash::{BuildHasher, Hasher},
};

use auction_domain::valid_text;
use serde::{Deserialize, Serialize};

use crate::ports::mail::processing::{MailEquipmentAttachment, MailMessage};
use crate::ports::settlement::system_returned_settlement_obligation_id;

pub const MAIL_DISPLAY_SCHEMA_VERSION: u8 = 1;
pub const MAIL_DISPLAY_REDACTION_VERSION: u8 = 1;
const MAX_SUBJECT_BYTES: usize = 512;
const MAX_BODY_BYTES: usize = 64 * 1024;
const MAX_JSON_BYTES: usize = 128 * 1024;
const MAX_ATTACHMENT_IDS: usize = 1000;
const LOGIN_KEY_TEXT_LENGTH: usize = 43;
const MANUAL_KEY_MIN_BYTES: usize = 32;
const REDACTED: &str = "[REDACTED]";

/// 可持久化到 Admin 审查详情的 MoogleMail 原始信息安全投影。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MailDisplaySnapshot {
    pub schema_version: u8,
    pub redaction_version: u8,
    pub body: String,
    pub attachment_ids: Vec<String>,
    pub credits: i64,
    pub cod: i64,
}

impl fmt::Debug for MailDisplaySnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MailDisplaySnapshot")
            .field("schema_version", &self.schema_version)
            .field("redaction_version", &self.redaction_version)
            .field("body_bytes", &self.body.len())
            .field("attachment_count", &self.attachment_ids.len())
            .field("credits", &self.credits)
            .field("cod", &self.cod)
            .finish()
    }
}

impl MailDisplaySnapshot {
    /// 从短生命周期邮件 envelope 生成不含装备 key 和认证 key 的展示投影。
    pub fn from_message(message: &MailMessage) -> Option<Self> {
        if message.credits < 0
            || message.cod < 0
            || message.attachments.len() > MAX_ATTACHMENT_IDS
            || message.body.len() > MAX_BODY_BYTES
        {
            return None;
        }
        let mut attachment_ids = message.sorted_attachment_ids();
        attachment_ids.dedup();
        let body = if authentication_subject(&message.subject) {
            "[REDACTED: authentication message]".to_owned()
        } else {
            limit_redacted_text(
                redact_message_text(&message.body, &message.attachments)?,
                MAX_BODY_BYTES,
            )
        };
        let snapshot = Self {
            schema_version: MAIL_DISPLAY_SCHEMA_VERSION,
            redaction_version: MAIL_DISPLAY_REDACTION_VERSION,
            body,
            attachment_ids,
            credits: message.credits,
            cod: message.cod,
        };
        snapshot.validate().then_some(snapshot)
    }

    /// 在入站展示字段与 Admin 投影中遮蔽主题里的认证 key 和装备 key。
    pub fn redact_subject(
        sender: &str,
        subject: &str,
        attachments: &[MailEquipmentAttachment],
    ) -> String {
        // 恢复与领取核验会重读规范系统退件主题。其闭集格式只有固定文案与义务 UUID，
        // 即使 UUID 的一段恰好与附件 key 重合，也须原样保留来源证据。
        if system_returned_settlement_obligation_id(sender, subject).is_some() {
            return subject.to_owned();
        }
        redact_message_text(subject, attachments).map_or_else(
            || REDACTED.to_owned(),
            |text| limit_redacted_text(text, MAX_SUBJECT_BYTES),
        )
    }

    /// 使用原始发件人核验退件，生成历史主题的唯一安全展示值。
    pub fn display_stored_subject(sender: &str, subject: &str) -> String {
        if system_returned_settlement_obligation_id(sender, subject).is_some() {
            return subject.to_owned();
        }
        // 脱敏必须读到完整原文，裁剪会抹掉登录 key 标签后仅由空白组成的尾串。
        let redacted = redact_text(subject, None);
        let trimmed = redacted.trim();
        // 裁剪不能把非规范退件主题变成可被识别为来源证据的主题。
        if !valid_text(trimmed, MAX_SUBJECT_BYTES)
            || system_returned_settlement_obligation_id(sender, trimmed).is_some()
        {
            return REDACTED.to_owned();
        }
        if trimmed.len() == redacted.len() { redacted } else { trimmed.to_owned() }
    }

    /// 对历史展示正文重新应用当前脱敏规则，此时可用的附件信息只有 ID。
    pub fn redact_stored_body(body: &str) -> String {
        limit_redacted_text(redact_text(body, None), MAX_BODY_BYTES)
    }

    /// 校验展示投影边界和版本闭集。
    pub fn validate(&self) -> bool {
        self.schema_version == MAIL_DISPLAY_SCHEMA_VERSION
            && self.redaction_version == MAIL_DISPLAY_REDACTION_VERSION
            && self.body.len() <= MAX_BODY_BYTES
            && self
                .body
                .chars()
                .all(|character| !character.is_control() || matches!(character, '\n' | '\r' | '\t'))
            && url_key_ranges(&self.body).next().is_none()
            && self.credits >= 0
            && self.cod >= 0
            && self.attachment_ids.len() <= MAX_ATTACHMENT_IDS
            && self.attachment_ids.windows(2).all(|ids| ids[0] < ids[1])
            && self.attachment_ids.iter().all(|id| {
                !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()) && id.len() <= 64
            })
            // PostgreSQL jsonb 的文本序列化会加入分隔空格，预留该开销以免展示字段阻断入站。
            && serde_json::to_vec(self).is_ok_and(|json| {
                json.len().saturating_add(self.attachment_ids.len() + 16) <= MAX_JSON_BYTES
            })
    }
}

fn authentication_subject(subject: &str) -> bool {
    let subject = subject.to_ascii_lowercase();
    ["register", "rebind", "login", "auth", "key"].iter().any(|marker| subject.contains(marker))
}

fn key_fingerprint(key: &[u8], state: &RandomState) -> u64 {
    let mut hasher = state.build_hasher();
    for byte in key {
        hasher.write_u8(byte.to_ascii_lowercase());
    }
    hasher.finish()
}

fn redact_message_text(source: &str, attachments: &[MailEquipmentAttachment]) -> Option<String> {
    if attachments.is_empty() {
        return Some(redact_text(source, None));
    }
    let mut keys = HashSet::with_capacity(attachments.len());
    let hash_state = RandomState::new();
    for attachment in attachments {
        let fingerprint =
            attachment.key.with_bytes(|key| key_fingerprint(key, &hash_state)).ok()?;
        keys.insert(fingerprint);
    }
    Some(redact_text(source, Some((&keys, &hash_state))))
}

fn redact_text(source: &str, attachment_keys: Option<(&HashSet<u64>, &RandomState)>) -> String {
    let mut ranges = url_key_ranges(source)
        .chain(credential_token_ranges(source, attachment_keys.is_none()))
        .chain(labeled_key_ranges(source))
        .collect::<Vec<_>>();
    // 原始 key 不离开同步借用；集合仅保留本次随机盐指纹，碰撞最多额外屏蔽展示文本。
    // 附件 key 固定十字节；一次扫描同时匹配全部 key，成本不随附件数乘倍增长。
    if let Some((keys, hash_state)) = attachment_keys {
        for (start, window) in source.as_bytes().windows(10).enumerate() {
            if keys.contains(&key_fingerprint(window, hash_state))
                && source.is_char_boundary(start)
                && source.is_char_boundary(start + 10)
            {
                ranges.push((start, start + 10));
            }
        }
    }
    ranges.sort_unstable();
    // 同一个 key 可以在正文中重叠出现；先合并区间，避免后续替换破坏旧索引。
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in ranges {
        if let Some(last) = merged.last_mut()
            && start <= last.1
        {
            last.1 = last.1.max(end);
        } else {
            merged.push((start, end));
        }
    }
    let mut redacted = String::with_capacity(source.len());
    let mut offset = 0;
    for (start, end) in merged {
        redacted.push_str(&source[offset..start]);
        redacted.push_str(REDACTED);
        offset = end;
    }
    redacted.push_str(&source[offset..]);
    redacted
}

fn limit_redacted_text(redacted: String, maximum_bytes: usize) -> String {
    // 短凭据替换后仍须满足展示字段上限，避免脱敏阻断入站提交。
    if redacted.len() <= maximum_bytes { redacted } else { REDACTED.to_owned() }
}

fn credential_token_ranges(
    text: &str,
    redact_bare_equipment: bool,
) -> impl Iterator<Item = (usize, usize)> + '_ {
    let mut start = 0;
    text.as_bytes()
        .split(|byte| !byte.is_ascii_alphanumeric() && !matches!(*byte, b'-' | b'_'))
        .filter_map(move |token| {
            let end = start + token.len();
            let range = (start, end);
            start = end + 1;
            // 历史投影没有附件明文，保守遮蔽所有独立的十位十六进制值。
            (token.len() == LOGIN_KEY_TEXT_LENGTH
                || redact_bare_equipment
                    && token.len() == 10
                    && token.iter().all(u8::is_ascii_hexdigit))
            .then_some(range)
        })
}

fn labeled_key_ranges(text: &str) -> impl Iterator<Item = (usize, usize)> + '_ {
    let mut line_offset = 0;
    text.split_inclusive(['\r', '\n']).filter_map(move |line| {
        let offset = line_offset;
        line_offset += line.len();
        let line = line.trim_end_matches(['\r', '\n']);
        let value = labeled_key_value(line)?;
        Some((offset + line.len() - value.len(), offset + line.len()))
    })
}

fn labeled_key_value(line: &str) -> Option<&str> {
    let line = line.trim_start();
    // 单独的 key 只在行首或有赋值分隔符时视作标签，以免遮蔽普通行文。
    line.as_bytes().windows(3).enumerate().find_map(|(start, marker)| {
        if !marker.eq_ignore_ascii_case(b"key") {
            return None;
        }
        let before = &line[..start];
        if before.chars().next_back().is_some_and(label_word_character) {
            return None;
        }
        let tail = &line[start + 3..];
        let prefix = before.trim_end();
        if ends_with_login_label(prefix) && !tail.starts_with(char::is_alphanumeric) {
            // 明确的 login key / 登录 key 标签后，只要到行尾还有任何字符（含空白与标点），
            // 就遮蔽整段原文，不裁剪、不找分隔符、不设长度门槛。
            return (!tail.is_empty()).then_some(tail);
        }
        let value = tail.trim_start();
        if let Some(value) = value.strip_prefix([':', '：', '=']) {
            let value = value.trim_start();
            return (!value.is_empty()).then_some(value);
        }
        (prefix.is_empty() && tail.len() > value.len() && value.len() >= MANUAL_KEY_MIN_BYTES)
            .then_some(value)
    })
}

fn ends_with_login_label(prefix: &str) -> bool {
    ["login", "登录"].into_iter().any(|label| {
        let Some(start) = prefix.len().checked_sub(label.len()) else {
            return false;
        };
        prefix.get(start..).is_some_and(|tail| {
            tail.eq_ignore_ascii_case(label)
                && !prefix[..start].chars().next_back().is_some_and(label_word_character)
        })
    })
}

fn label_word_character(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '_' | '-')
}

fn url_key_ranges(text: &str) -> impl Iterator<Item = (usize, usize)> + '_ {
    let bytes = text.as_bytes();
    bytes.iter().enumerate().filter_map(move |(slash, byte)| {
        if *byte != b'/' || slash + 11 > bytes.len() {
            return None;
        }
        let end = slash + 11;
        (bytes[slash + 1..end].iter().all(u8::is_ascii_hexdigit)
            && bytes.get(end).is_none_or(|byte| !byte.is_ascii_hexdigit())
            && text.is_char_boundary(slash + 1)
            && text.is_char_boundary(end))
        .then_some((slash + 1, end))
    })
}

#[cfg(test)]
mod tests {
    use auction_domain::{EquipmentKey, SettlementObligationKind};
    use auction_time::Timestamp;
    use uuid::Uuid;

    use super::MailDisplaySnapshot;
    use crate::ports::mail::processing::{MailEquipmentAttachment, MailMessage};
    use crate::ports::settlement::{SYSTEM_RETURN_SENDER, system_return_matches_obligation};

    #[test]
    fn message_text_redacts_login_urls_and_known_attachment_keys() {
        let text = "凭据(AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA) https://example.invalid/equip/123/0123abcdef 3D2F1E0C9B";
        let safe = "凭据([REDACTED]) https://example.invalid/equip/123/[REDACTED] [REDACTED]";
        let mut message = MailMessage {
            mailbox_key: "main".to_owned(),
            external_mail_id: "9001".to_owned(),
            world_id: "main".to_owned(),
            received_at: Timestamp::from_timestamp(1_800_000_000, 0).unwrap(),
            sender: "Player".to_owned(),
            subject: text.to_owned(),
            body: text.to_owned(),
            credits: 0,
            cod: 0,
            attachments: vec![MailEquipmentAttachment {
                external_equipment_id: "123".to_owned(),
                key: EquipmentKey::new("3d2f1e0c9b").unwrap(),
            }],
            returnable: false,
            claimable: true,
            content_digest: [1; 32],
        };
        assert_eq!(MailDisplaySnapshot::from_message(&message).unwrap().body, safe);
        assert_eq!(
            MailDisplaySnapshot::redact_subject(
                &message.sender,
                &message.subject,
                &message.attachments
            ),
            safe,
        );
        let manual = "补充说明：登录 key 多字节 夹杂 空格 与标点! 这不是可通过形态识别的值";
        message.subject = manual.to_owned();
        message.body = format!("正常正文\n{manual}\n保留的下一行");
        assert_eq!(
            MailDisplaySnapshot::redact_subject(
                &message.sender,
                &message.subject,
                &message.attachments
            ),
            "补充说明：登录 key[REDACTED]",
        );
        // 主题本身没有认证标记时，正文仍须遮蔽带标签的手工 key。
        message.subject = "拍卖问题".to_owned();
        assert_eq!(
            MailDisplaySnapshot::from_message(&message).unwrap().body,
            "正常正文\n补充说明：登录 key[REDACTED]\n保留的下一行",
        );
        message.body = format!(
            "补充说明：登录 key {}\n补充说明：登录 key \n补充说明：登录 key",
            " ".repeat(32)
        );
        assert_eq!(
            MailDisplaySnapshot::from_message(&message).unwrap().body,
            "补充说明：登录 key[REDACTED]\n补充说明：登录 key[REDACTED]\n补充说明：登录 key",
        );
        message.subject = "rekey".to_owned();
        assert_eq!(
            MailDisplaySnapshot::from_message(&message).unwrap().body,
            "[REDACTED: authentication message]"
        );
    }

    #[test]
    fn subject_redaction_preserves_only_canonical_return_proof() {
        let obligation_id = Uuid::parse_str("5b1c2a9e-1f0d-4c6b-9a7e-3d2f1e0c9b8a").unwrap();
        let subject = format!("[RTS] Auction payment request [{obligation_id}]");
        let attachments = [MailEquipmentAttachment {
            external_equipment_id: "123".to_owned(),
            key: EquipmentKey::new("3d2f1e0c9b").unwrap(),
        }];
        let safe =
            MailDisplaySnapshot::redact_subject(SYSTEM_RETURN_SENDER, &subject, &attachments);
        assert_eq!(safe, subject);
        assert!(system_return_matches_obligation(
            SYSTEM_RETURN_SENDER,
            &safe,
            SettlementObligationKind::WinnerPaymentCarrierCod,
            obligation_id
        ));

        let extended = format!("{subject} https://example.invalid/equip/123/0123abcdef");
        let safe =
            MailDisplaySnapshot::redact_subject(SYSTEM_RETURN_SENDER, &extended, &attachments);
        assert!(!safe.contains("0123abcdef"));
        assert!(!system_return_matches_obligation(
            SYSTEM_RETURN_SENDER,
            &safe,
            SettlementObligationKind::WinnerPaymentCarrierCod,
            obligation_id
        ));
    }

    #[test]
    fn stored_subject_normalizes_only_after_redaction() {
        let display = MailDisplaySnapshot::display_stored_subject;
        assert_eq!(display("Player", "拍卖问题 \u{3000}"), "拍卖问题");
        assert_eq!(display("Player", "补充说明：登录 key   "), "补充说明：登录 key[REDACTED]");
        assert_eq!(display("Player", "补充说明：登录 key"), "补充说明：登录 key");
        assert_eq!(display("Player", "\t\u{3000}"), "[REDACTED]");
        assert_eq!(display("Player", "多行\n主题"), "[REDACTED]");
    }

    #[test]
    fn stored_subject_checks_length_after_trimming_redacted_text() {
        let text = "x".repeat(491);
        let padded = format!("          {text} login key ");
        assert_eq!(padded.len(), 512);
        let expected = format!("{text} login key[REDACTED]");
        assert_eq!(expected.len(), 511);
        let safe = MailDisplaySnapshot::display_stored_subject("Player", &padded);
        assert_eq!(safe, expected);
        assert_eq!(MailDisplaySnapshot::redact_subject("Player", &safe, &[]), expected);
        assert_eq!(MailDisplaySnapshot::redact_subject("Player", &padded, &[]), "[REDACTED]");

        let still_too_long = format!("{text}xx login key ");
        assert_eq!(
            MailDisplaySnapshot::display_stored_subject("Player", &still_too_long),
            "[REDACTED]",
        );
    }

    #[test]
    fn stored_subject_does_not_reconstruct_canonical_return_proof() {
        let display = MailDisplaySnapshot::display_stored_subject;
        let canonical = "[RTS] Auction payment request [5b1c2a9e-1f0d-4c6b-9a7e-3d2f1e0c9b8a]";
        let padded = format!("{canonical} ");
        assert_eq!(display(SYSTEM_RETURN_SENDER, canonical), canonical);
        assert_eq!(display(SYSTEM_RETURN_SENDER, &padded), "[REDACTED]");
        assert_eq!(display("MoogleMail ", &padded), canonical);
    }
}
