//! 周期读取的单次刷新与到期命令编排；定时器由外层负责。

use crate::ports::runtime_switches::{
    RuntimeSwitchCommand, RuntimeSwitchError, RuntimeSwitchOperation, RuntimeSwitchStore,
    RuntimeSwitches,
};
use std::{sync::Arc, time::Instant};
use uuid::Uuid;

/// 共享刷新用例；到期回落由快照期限决定，追加收口失败不延长覆盖。
pub struct RuntimeSwitchRefreshService {
    store: Arc<dyn RuntimeSwitchStore>,
    switches: RuntimeSwitches,
}

impl RuntimeSwitchRefreshService {
    /// 注入窄数据库端口与进程快照。
    pub fn new(store: Arc<dyn RuntimeSwitchStore>, switches: RuntimeSwitches) -> Self {
        Self { store, switches }
    }

    /// 一个读取周期；数据库返回每名称最新事实，不过滤已到期 set。
    pub async fn refresh(&self) -> Result<(), RuntimeSwitchError> {
        let started = Instant::now();
        self.switches.install(self.store.read().await?, started)
    }

    /// Worker 周期追加到期事实；原设置命令稳定派生身份，可安全重试未知提交。
    pub async fn expire(&self) -> Result<(), RuntimeSwitchError> {
        let read = self.store.read().await?;
        for event in read.events {
            event.validate()?;
            if event.operation != RuntimeSwitchOperation::Set
                || !event.expires_at.is_some_and(|until| until <= read.database_time)
            {
                continue;
            }
            let command = RuntimeSwitchCommand {
                command_id: Uuid::new_v5(&event.command_id, b"runtime-switch-expire-v1"),
                name: event.name,
                expected_revision: event.revision,
                operation: RuntimeSwitchOperation::Expire,
                value: None,
                expires_at: None,
                expired_command_id: Some(event.command_id),
                actor_id: "runtime-switch-expirer".to_owned(),
                operator_identity: None,
                entrypoint: "system".to_owned(),
                reason: "override expired".to_owned(),
            };
            match self.store.write(command).await {
                Ok(_) | Err(RuntimeSwitchError::Conflict) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}
