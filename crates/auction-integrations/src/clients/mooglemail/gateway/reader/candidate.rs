//! 候选凭据的固定只读探测：Admin 在线检查经 Worker 执行时，用本 gateway 与邮件循环共用的
//! 客户端读取 Item Shop 余额页。只判断 Cookie 当次能否读取，不证明账号身份，也不改变任何
//! 运行凭据、会话或账号忙状态。

use chrono::Utc;

use super::MoogleMailGateway;
use crate::clients::mooglemail::error::MoogleMailError;
use crate::clients::mooglemail::protocol::{MoogleMailCookie, MoogleMailWorld};

impl MoogleMailGateway {
    /// 用调用方解密的候选 Cookie 读取指定世界的余额页。请求共享本账号的请求门、在途锁与
    /// 运行 owner 复核，因而不会与 Worker 自己的读写重叠；账号忙窗口内不发网络，成功也不结束
    /// 当前账号的忙碌期。余额只用于确认页面结构，不是身份证据，结果不保留。
    pub(crate) async fn probe_candidate_cookie(
        &self,
        world: MoogleMailWorld,
        cookie: &MoogleMailCookie,
    ) -> Result<(), MoogleMailError> {
        let target = self.client.credential_check(world)?;
        let body = self.client.get(&target, cookie).await?;
        self.parser.parse_balance(&body, world, Utc::now()).map(|_| ())
    }
}
