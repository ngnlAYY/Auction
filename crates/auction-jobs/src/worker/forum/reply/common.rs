//! 论坛命令的完成、稳定 ID、错误分类和请求辅助。

use super::*;

pub(super) async fn finish(
    runtime: &ForumReplyRuntime,
    command_id: Uuid,
    status: &str,
    error_code: &str,
    detail: &str,
) -> Result<(), ApplicationError> {
    runtime
        .store
        .finish_command(command_id, status.to_owned(), error_code.to_owned(), detail.to_owned())
        .await
}

pub(super) fn forum_command_id(
    auction_id: auction_domain::AuctionId,
    thread_id: &str,
    post_id: &str,
    line_number: i32,
    lot_code: &str,
    command_kind: &str,
) -> Uuid {
    let name = format!(
        "auction/forum-command/v2/{auction_id}/{thread_id}/{post_id}/{line_number}/{lot_code}/{command_kind}"
    );
    Uuid::new_v5(&Uuid::NAMESPACE_URL, name.as_bytes())
}

pub(super) fn is_retryable(error: &ApplicationError) -> bool {
    matches!(
        error,
        ApplicationError::Repository(_)
            | ApplicationError::CompetitionTransaction(
                CompetitionTransactionError::Failed(_)
                    | CompetitionTransactionError::Conflict
                    | CompetitionTransactionError::SerializationConflict,
            )
    )
}

pub(super) fn bounded_detail(value: String) -> String {
    let detail: String = value.chars().take(512).collect();
    if detail.is_empty() { "论坛竞价命令被拒绝".to_owned() } else { detail }
}

pub(super) fn repository_error(message: &str) -> ApplicationError {
    auction_application::core::RepositoryError::Failed(message.to_owned()).into()
}
