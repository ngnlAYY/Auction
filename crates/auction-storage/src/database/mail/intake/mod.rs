//! PostgreSQL MoogleMail 拍卖入站 owner 事务；应用层只交已验证的公开装备事实和密钥信封。
//! 命令账本、投稿、装备、密钥信封、pending lots、预览版本、审计和 realtime 事件必须同事务完成。

use std::future::Future;

use auction_application::core::CompetitionTransactionError;
use auction_application::mail::{
    MailAuctionAcceptance, MailAuctionIntakeLookup, MailAuctionIntakeRecord,
    MailAuctionIntakeResult, MailAuctionIntakeTransaction, MailAuctionIntakeWrite,
    SubmissionReceiptFacts,
};

use uuid::Uuid;

use crate::database::auction::competition::{PostgresCompetitionTransaction, map_sql_error};

mod acceptance;
mod command;
mod receipt;
mod record;

#[allow(clippy::manual_async_fn)]
impl MailAuctionIntakeTransaction for PostgresCompetitionTransaction {
    fn load_mail_auction_acceptance(
        &mut self,
        lookup: MailAuctionIntakeLookup,
    ) -> impl Future<Output = Result<Option<MailAuctionAcceptance>, CompetitionTransactionError>> + Send
    {
        async move { acceptance::load_accepted(self, lookup).await }
    }

    fn add_mail_auction_intake(
        &mut self,
        write: MailAuctionIntakeWrite,
    ) -> impl Future<Output = Result<MailAuctionIntakeResult, CompetitionTransactionError>> + Send
    {
        async move { command::add_mail_auction_intake_inner(self, write).await }
    }

    fn load_submission_receipt(
        &mut self,
        submission_id: Uuid,
    ) -> impl Future<Output = Result<Option<SubmissionReceiptFacts>, CompetitionTransactionError>> + Send
    {
        async move { receipt::load_submission_receipt(self, submission_id).await }
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
