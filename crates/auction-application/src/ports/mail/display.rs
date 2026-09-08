//! MoogleMail 入站邮件的安全展示投影。

use std::{
    collections::{HashSet, hash_map::RandomState},
    fmt,
    hash::{BuildHasher, Hasher},
};

use serde::{Deserialize, Serialize};

use super::mail_processing::MailMessage;

pub const MAIL_DISPLAY_SCHEMA_VERSION: u8 = 1;
pub const MAIL_DISPLAY_REDACTION_VERSION: u8 = 1;
const MAX_BODY_BYTES: usize = 64 * 1024;
const MAX_JSON_BYTES: usize = 128 * 1024;
const MAX_ATTACHMENT_IDS: usize = 1000;

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
            let mut keys = HashSet::with_capacity(message.attachments.len());
            let hash_state = RandomState::new();
            for attachment in &message.attachments {
                let fingerprint =
                    attachment.key.with_bytes(|key| key_fingerprint(key, &hash_state)).ok()?;
                keys.insert(fingerprint);
            }
            redact_body(&message.body, &keys, &hash_state)
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

    /// 校验展示投影边界和版本闭集。
    pub fn validate(&self) -> bool {
        self.schema_version == MAIL_DISPLAY_SCHEMA_VERSION
            && self.redaction_version == MAIL_DISPLAY_REDACTION_VERSION
            && self.body.len() <= MAX_BODY_BYTES
            && self
                .body
                .chars()
                .all(|character| !character.is_control() || matches!(character, '\n' | '\r' | '\t'))
            && url_key_ranges(&self.body).is_empty()
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

fn redact_body(source: &str, keys: &HashSet<u64>, hash_state: &RandomState) -> String {
    let mut ranges = url_key_ranges(source);
    // 原始 key 不离开同步借用；集合仅保留本次随机盐指纹，碰撞最多额外屏蔽正文。
    // 附件 key 固定十字节；一次扫描同时匹配全部 key，成本不随附件数乘倍增长。
    for (start, window) in source.as_bytes().windows(10).enumerate() {
        if keys.contains(&key_fingerprint(window, hash_state))
            && source.is_char_boundary(start)
            && source.is_char_boundary(start + 10)
        {
            ranges.push((start, start + 10));
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
        redacted.push_str("[REDACTED]");
        offset = end;
    }
    redacted.push_str(&source[offset..]);
    redacted
}

fn url_key_ranges(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut ranges = Vec::new();
    for slash in 0..bytes.len() {
        if bytes[slash] != b'/' || slash + 11 > bytes.len() {
            continue;
        }
        let end = slash + 11;
        if bytes[slash + 1..end].iter().all(u8::is_ascii_hexdigit)
            && bytes.get(end).is_none_or(|byte| !byte.is_ascii_hexdigit())
            && text.is_char_boundary(slash + 1)
            && text.is_char_boundary(end)
        {
            ranges.push((slash + 1, end));
        }
    }
    ranges
}
