//! MoogleMail 解析结果的安全数据模型。

use super::*;

pub(crate) const MAX_INBOX_ROWS_PER_PAGE: usize = 1000;
pub(crate) const MAX_SENT_ROWS_PER_PAGE: usize = MAX_INBOX_ROWS_PER_PAGE;
pub(crate) const MAX_ATTACHMENT_ENTRIES: usize = 1000;
pub(crate) const MAX_SUMMARY_SUBJECT_BYTES: usize = 512;
pub(crate) const MAX_MESSAGE_TEXT_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const INBOX_TIME_FORMAT: &str = "%Y-%m-%d %H:%M";

mod balance;
mod inventory;
mod message;
mod sent;

pub use balance::*;
pub use inventory::*;
pub use message::*;
pub use sent::*;
