//! PostgreSQL MoogleMail 拍卖入站 owner 事务。
//!
//! 应用层只把已验证的公开装备事实和密钥信封交给这里。父模块保留事务端口实现；
//! 入站写入和审计/realtime 记录分别归档到子模块，但命令账本、投稿、装备、密钥信封、
//! pending lots、预览版本、审计和 realtime 事件仍必须在同一事务中完成。

use std::future::Future;

use auction_application::{
    CompetitionTransactionError, MailAuctionIntakeRecord, MailAuctionIntakeResult,
    MailAuctionIntakeTransaction, MailAuctionIntakeWrite,
};

use super::competition::{PostgresCompetitionTransaction, map_sql_error};

mod intake;
mod record;

#[allow(clippy::manual_async_fn)]
impl MailAuctionIntakeTransaction for PostgresCompetitionTransaction {
    fn add_mail_auction_intake(
        &mut self,
        write: MailAuctionIntakeWrite,
    ) -> impl Future<Output = Result<MailAuctionIntakeResult, CompetitionTransactionError>> + Send
    {
        async move { intake::add_mail_auction_intake_inner(self, write).await }
    }

    fn append_mail_auction_record(
        &mut self,
        record: MailAuctionIntakeRecord,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send {
        async move { record::append_mail_auction_record_inner(self, record).await }
    }
}

fn failed(message: &str) -> CompetitionTransactionError {
    CompetitionTransactionError::Failed(message.to_owned())
}
