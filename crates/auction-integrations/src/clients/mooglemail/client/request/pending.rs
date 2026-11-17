//! 两段式读取的发出边界。排队阶段（请求门、在途锁、账号忙与 owner 复核）在调用方 future 内执行，
//! 被取消或预算到期即撤票退出、不会补发；返回的 [`PendingRead`] 停在发出边界。发出之后放弃等待只
//! 丢弃结果：请求照常走完，在途锁持有到远端回应或客户端超时。Worker 自己的读取
//! （[`MoogleMailClient::get`]）排队结束即发出，调用方放弃时剩余部分交给独立任务；候选凭据在线
//! 检查先在发出边界复核自己的放行条件，再把发出交给带着检查名额的独立任务。

use super::QueuedRead;
use super::body::DiagnosticResponse;
use crate::clients::mooglemail::client::MoogleMailClient;
use crate::clients::mooglemail::protocol::MoogleMailCookie;
use crate::clients::mooglemail::{MoogleMailError, MoogleMailTarget, MoogleMailWorld};

/// 已排到发出边界、尚未发出的读取：持有在途锁与构造好的请求。丢弃即放弃发出并释放在途锁。
pub(crate) struct PendingRead {
    client: MoogleMailClient,
    target: MoogleMailTarget,
    cookie: MoogleMailCookie,
    queued: QueuedRead,
}

impl MoogleMailClient {
    /// 以候选 Cookie 排队读取指定世界的余额页（`credential_check`），停在发出边界。
    pub(crate) async fn queue_credential_check(
        &self,
        world: MoogleMailWorld,
        cookie: &MoogleMailCookie,
    ) -> Result<PendingRead, MoogleMailError> {
        let target = self.credential_check(world)?;
        self.queue_pending(target, cookie).await
    }

    /// 排队到发出边界；排队失败按普通读取的口径留证与记诊断。
    pub(super) async fn queue_pending(
        &self,
        target: MoogleMailTarget,
        cookie: &MoogleMailCookie,
    ) -> Result<PendingRead, MoogleMailError> {
        match self.queue_read(&target, cookie).await {
            Ok(queued) => {
                Ok(PendingRead { client: self.clone(), target, cookie: cookie.clone(), queued })
            }
            Err(error) => {
                Err(self.report_read_failure(&target, &DiagnosticResponse::default(), error).await)
            }
        }
    }
}

impl PendingRead {
    /// 发出请求并读完回应，结果分类与诊断同普通读取。剩余部分有界：HTTP 交换受整次请求超时约束，
    /// 回应后的 owner 复核与失败留证各有上限。调用方放弃等待时须把它交给独立任务走完，不能丢弃。
    pub(crate) async fn send(self) -> Result<Vec<u8>, MoogleMailError> {
        let Self { client, target, cookie, queued } = self;
        let mut response = DiagnosticResponse::default();
        match client.send_read(queued, &target, &cookie, &mut response).await {
            Ok(body) => Ok(body),
            Err(error) => Err(client.report_read_failure(&target, &response, error).await),
        }
    }
}
