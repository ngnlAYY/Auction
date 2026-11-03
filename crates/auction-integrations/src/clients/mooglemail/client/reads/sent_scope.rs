//! 临时发送前上下文的身份绑定；允许自身 POST 推进读取修订，不允许换凭据或 owner。

use std::sync::Arc;

use super::{MoogleMailReadRound, ReadScope, ScopeKey, read_unavailable};
use crate::clients::mooglemail::{MoogleMailClient, MoogleMailCookie, MoogleMailError};

/// 不持有发送前正文或 epoch；同一 client 的 endpoint 不可变，独立 client 的 scope 不共享。
pub(crate) struct MoogleMailSentScope {
    key: ScopeKey,
    scope: Arc<ReadScope>,
}

impl MoogleMailReadRound {
    pub(crate) fn sent_scope(&self) -> MoogleMailSentScope {
        MoogleMailSentScope { key: self.key, scope: Arc::clone(&self.scope) }
    }
}

impl MoogleMailClient {
    pub(crate) fn current_sent_scope(
        &self,
        cookie: &MoogleMailCookie,
        world: crate::clients::mooglemail::MoogleMailWorld,
    ) -> Result<MoogleMailSentScope, MoogleMailError> {
        let (key, scope) = self.read_scope(cookie, world)?;
        Ok(MoogleMailSentScope { key, scope })
    }

    /// 检查原凭据、world、运行代次与客户端身份；不把旧读取 epoch 提升为写授权。
    pub(crate) fn check_sent_scope(
        &self,
        cookie: &MoogleMailCookie,
        scope: &MoogleMailSentScope,
    ) -> Result<(), MoogleMailError> {
        let (key, current) = self.read_scope(cookie, scope.key.world)?;
        if key != scope.key || !Arc::ptr_eq(&current, &scope.scope) {
            return Err(read_unavailable(scope.key.world));
        }
        Ok(())
    }
}
