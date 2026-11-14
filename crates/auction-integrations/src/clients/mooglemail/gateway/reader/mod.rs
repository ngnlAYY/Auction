//! MoogleMail 收件、Sent 和余额的有界读取流程。
//!
//! 入口只保留 gateway 状态与子模块声明；构造、窗口选择、增量扫描、收件读取、
//! Sent 证据、恢复扫描与只读快照分别在 `gateway/reader/` 下的子模块实现。
//! 所有子模块共用同一 fetch 预算、只读凭据会话与页数上限，错误映射与日志字段
//! 保持不记录凭据材料。

use std::{
    collections::HashMap,
    sync::{Arc, Mutex as StdMutex},
};

use auction_application::mail::{
    MailPollClaim, MailPollProgressObserver, MailPollReadJournal, MailScanCheckpoint,
    MailScanObserver, MailScanStream,
};

use super::credentials::MoogleMailCredentialSet;
use super::model::MoogleMailGatewayConfig;
use crate::clients::mooglemail::client::MoogleMailClient;
use crate::clients::mooglemail::parser::MoogleMailParser;
use crate::clients::mooglemail::protocol::MoogleMailWorld;

mod error;
mod gateway;
mod inbox;
mod recovery_scan;
mod scan;
mod sent;
mod sent_hints;
mod snapshot;
mod window;

type ReadJournalContext<'a> = (&'a dyn MailPollReadJournal, &'a MailPollClaim);

/// MoogleMail 收件箱读取 gateway。
pub struct MoogleMailGateway {
    pub(super) client: Arc<MoogleMailClient>,
    pub(super) credentials: MoogleMailCredentialSet,
    pub(super) config: MoogleMailGatewayConfig,
    pub(super) parser: MoogleMailParser,
    pub(super) profile_reader:
        Option<Arc<dyn auction_application::forum::ForumTargetProfileReader>>,
    progress_observer: Option<Arc<dyn MailPollProgressObserver>>,
    /// 进程内每日校验检查点；重启只会多做一次完整扫描，永远不会跳过未提交游标之后的行。
    scan_checkpoints: StdMutex<HashMap<(MoogleMailWorld, MailScanStream), MailScanCheckpoint>>,
    scan_observer: Arc<dyn MailScanObserver>,
    /// 已知 MID 的 Sent 页码提示；只决定翻页起点，不参与证明。
    sent_page_hints: sent_hints::SentPageHints,
}
