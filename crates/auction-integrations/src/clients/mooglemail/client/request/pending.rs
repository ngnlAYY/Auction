//! 发出边界交给调用方的两段式读取，只供候选凭据在线检查使用。排队阶段（请求门、在途锁、账号忙
//! 与 owner 复核）在调用方 future 内执行，被取消或预算到期即撤票退出、不会补发；返回的
//! [`PendingRead`] 停在发出边界，调用方复核自己的放行条件后再发出。发出后放弃等待只丢弃结果：
//! 请求照常走完，在途锁持有到远端回应或客户端超时。

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
    /// 以候选 Cookie 排队读取指定世界的余额页（`credential_check`），停在发出边界。排队失败按
    /// 普通读取的口径留证与记诊断。
    pub(crate) async fn queue_credential_check(
        &self,
        world: MoogleMailWorld,
        cookie: &MoogleMailCookie,
    ) -> Result<PendingRead, MoogleMailError> {
        let target = self.credential_check(world)?;
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
    /// 发出请求并读完回应，结果分类与诊断同普通读取。调用方应在独立任务中执行：放弃等待只丢弃
    /// 结果，已发出的请求照常走完。
    pub(crate) async fn send(self) -> Result<Vec<u8>, MoogleMailError> {
        let Self { client, target, cookie, queued } = self;
        let mut response = DiagnosticResponse::default();
        match client.send_read(queued, &target, &cookie, &mut response).await {
            Ok(body) => Ok(body),
            Err(error) => Err(client.report_read_failure(&target, &response, error).await),
        }
    }
}
