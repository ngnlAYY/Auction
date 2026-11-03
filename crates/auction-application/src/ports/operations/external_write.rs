//! 外部写入的进程生命周期窄端口；不决定租约、幂等、持久状态或重发资格。

use std::{
    future::{Future, poll_fn},
    pin::pin,
    sync::Arc,
    task::{Context, Poll},
};

/// 指标与日志使用的编译期闭集；装备准备和告警发送复用 delivery_send。
#[derive(Debug, Clone, Copy)]
pub enum CriticalWriteKind {
    Background,
    DeliverySend,
    DeliveryTake,
    DeliveryReturn,
    ComposeDiscard,
    Forum,
    ForumPm,
}

/// 单工作项生命周期；实现只管理进程内等待，不执行数据库或外部操作。
pub trait ExternalWriteActivityPort: Send + Sync {
    /// 围栏调用前同步登记交接；论坛在已有持久 claim 下的第一个实际 POST 前登记。
    /// 交接与外写同样计入活动并阻止排空结束；登记不授权 POST，仍须确认持久围栏。
    /// 关闭入口后拒绝首次登记，已经登记的活动继续由原 owner 驱动。
    fn enter_critical(&self) -> bool;
    /// 仅在既有结果端口确认提交后调用；未调用时的 drop 只能记为未收口。
    fn complete(&self);
    /// 只有未进入关键段的工作在关闭时就绪；实现负责注册关闭唤醒。
    fn poll_cancelled(&self, context: &mut Context<'_>) -> Poll<()>;
    /// 同步判断准备期是否已取消，错误收口分支据此避免补写持久失败。
    fn is_cancelled(&self) -> bool;
}

/// 一个叶子工作项的共享活动守卫；默认值保留旧路径，不取消或延长任何业务操作。
#[derive(Clone, Default)]
pub struct ExternalWriteActivity(Option<Arc<dyn ExternalWriteActivityPort>>);
impl std::fmt::Debug for ExternalWriteActivity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExternalWriteActivity").field("enabled", &self.0.is_some()).finish()
    }
}
impl ExternalWriteActivity {
    /// 绑定装配根提供的单工作项活动。
    pub fn new(port: Arc<dyn ExternalWriteActivityPort>) -> Self {
        Self(Some(port))
    }
    /// 是否已注入新排空协议；false 保持旧路径。
    pub fn enabled(&self) -> bool {
        self.0.is_some()
    }
    /// 围栏调用前登记交接；关闭先发生时拒绝登记，调用方不得再发起围栏。
    /// 交接与外写共用活动计数和排空截止，只有围栏确认后才允许 POST。
    pub fn enter_critical(&self) -> bool {
        self.0.as_ref().is_none_or(|port| port.enter_critical())
    }
    /// 既有结果端口确认提交后释放；不会自行写入持久状态。
    pub fn complete(&self) {
        if let Some(port) = &self.0 {
            port.complete();
        }
    }
    /// 准备期取消仅由进程入口产生，已登记交接或进入外写阶段时返回 false。
    pub fn is_cancelled(&self) -> bool {
        self.0.as_ref().is_some_and(|port| port.is_cancelled())
    }
    /// 在同一个 poll 中检查取消和驱动工作；不按阶段快照 abort 父任务，避免误杀关键子项。
    pub async fn run<F: Future>(&self, future: F) -> Option<F::Output> {
        let mut future = pin!(future);
        poll_fn(|context| {
            if self.0.as_ref().is_some_and(|port| port.poll_cancelled(context).is_ready()) {
                return Poll::Ready(None);
            }
            future.as_mut().poll(context).map(Some)
        })
        .await
    }
}

/// 装配根注入逐工作项守卫的工厂。
pub trait ExternalWriteActivityFactory: Send + Sync {
    fn activity(&self, kind: CriticalWriteKind) -> ExternalWriteActivity;
}

/// 可整体回退的生命周期入口；未注入工厂即为旧排空路径。
#[derive(Clone, Default)]
pub struct ExternalWriteLifecycle(Option<Arc<dyn ExternalWriteActivityFactory>>);
impl ExternalWriteLifecycle {
    /// 仅在启用关键段预算时注入工厂。
    pub fn new(factory: Arc<dyn ExternalWriteActivityFactory>) -> Self {
        Self(Some(factory))
    }
    /// 是否已注入新排空协议；false 保持旧路径。
    pub fn enabled(&self) -> bool {
        self.0.is_some()
    }
    /// 为单个叶子工作项建立活动，默认工厂缺失时返回旧路径守卫。
    pub fn activity(&self, kind: CriticalWriteKind) -> ExternalWriteActivity {
        self.0.as_ref().map(|factory| factory.activity(kind)).unwrap_or_default()
    }
    /// 只用于不包含外写子项的 owner；外写必须由自己的活动守卫保护完整 future。
    pub async fn background<F: Future>(&self, future: F) -> Option<F::Output> {
        self.activity(CriticalWriteKind::Background).run(future).await
    }
}
