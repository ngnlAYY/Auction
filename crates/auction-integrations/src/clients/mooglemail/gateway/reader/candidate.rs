//! 候选凭据的固定只读探测：Admin 在线检查经 Worker 执行时，用本 gateway 与邮件循环共用的
//! 客户端读取 Item Shop 余额页。只判断 Cookie 当次能否读取，不证明账号身份，也不改变任何
//! 运行凭据、会话或账号忙状态。探测分两段：排队到发出边界由调用方等待，发出交给调用方的独立
//! 任务走完。

use auction_time::Timestamp;

use super::MoogleMailGateway;
use crate::clients::mooglemail::client::PendingRead;
use crate::clients::mooglemail::error::MoogleMailError;
use crate::clients::mooglemail::parser::MoogleMailParser;
use crate::clients::mooglemail::protocol::{MoogleMailCookie, MoogleMailWorld};

/// 停在发出边界的候选探测：已取得在途锁并完成 owner 复核。丢弃即放弃发出并释放在途锁。
pub(crate) struct CandidateProbe {
    read: PendingRead,
    parser: MoogleMailParser,
    world: MoogleMailWorld,
}

impl MoogleMailGateway {
    /// 用调用方解密的候选 Cookie 排队读取指定世界的余额页，停在发出边界。请求共享本账号的
    /// 请求门、在途锁与运行 owner 复核，因而不会与 Worker 自己的读写重叠；账号忙窗口内不发
    /// 网络。排队在调用方 future 内，取消即撤票退出。
    pub(crate) async fn queue_candidate_probe(
        &self,
        world: MoogleMailWorld,
        cookie: &MoogleMailCookie,
    ) -> Result<CandidateProbe, MoogleMailError> {
        let read = self.client.queue_credential_check(world, cookie).await?;
        Ok(CandidateProbe { read, parser: self.parser, world })
    }
}

impl CandidateProbe {
    /// 发出探测并确认余额页结构；成功不结束当前账号的忙碌期，余额不是身份证据，结果不保留。
    /// 调用方应在独立任务中执行：放弃等待不影响已发出的请求走完并持有在途锁到远端回应或客户端超时。
    pub(crate) async fn send(self) -> Result<(), MoogleMailError> {
        let body = self.read.send().await?;
        self.parser.parse_balance(&body, self.world, Timestamp::now()).map(|_| ())
    }
}
