//! 错误安全投影和受限诊断端口；仅输出附加观测，不决定控制流或持久化结果。

use std::sync::{Arc, OnceLock};

use auction_domain::{
    AuctionId, CommandId, LotId, ProxyAuthorizationId, SettlementId, SettlementWorld,
};
use uuid::Uuid;

mod projection;
pub use projection::{DiagnosticError, SafeErrorKind};

/// 日志级别只影响附加输出，不参与重试或状态判定。
#[derive(Clone, Copy)]
pub enum DiagnosticLevel {
    Error,
    Warn,
    Info,
    Debug,
}

/// 附加观测的处置闭集；不代表或改变持久化状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticDisposition {
    /// 未确认对应的持久化失败、退避或人工处置事实。
    Unpersisted,
}

impl DiagnosticDisposition {
    /// 稳定日志标签；只能由枚举产生。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Unpersisted => "unpersisted",
        }
    }
}

/// 诊断只携带业务 UUID、固定世界和路由模板；禁止正文、凭据、URL 或 claim token。
/// route 只能来自框架 MatchedPath 或源码路由模板；同步借用，不被日志门缓存。
#[derive(Clone, Copy, Default)]
pub struct DiagnosticContext<'a> {
    pub route: Option<&'a str>,
    pub worker_loop: Option<&'static str>,
    pub round_id: Option<Uuid>,
    pub workflow_id: Option<Uuid>,
    pub authorization_id: Option<Uuid>,
    pub notification_id: Option<Uuid>,
    pub evidence_id: Option<Uuid>,
    pub obligation_id: Option<Uuid>,
    pub delivery_id: Option<Uuid>,
    pub auction_id: Option<Uuid>,
    pub lot_id: Option<Uuid>,
    pub action_id: Option<Uuid>,
    pub command_id: Option<Uuid>,
    pub world: Option<SettlementWorld>,
    pub defer_reason: Option<&'static str>,
    pub write_started: Option<bool>,
    pub disposition: Option<DiagnosticDisposition>,
}

/// 已完成安全投影的事件；固定 kind/phase 由调用点提供，不接受整个错误对象。
#[derive(Clone, Copy)]
pub struct SafeDiagnostic<'a> {
    pub level: DiagnosticLevel,
    pub safe_kind: SafeErrorKind,
    pub kind: &'static str,
    pub phase: &'static str,
    pub context: DiagnosticContext<'a>,
}

/// 可选日志门；实现只能过滤诊断，不得访问或改变命令状态。
pub trait DiagnosticSink: Send + Sync {
    fn record(&self, diagnostic: &SafeDiagnostic<'_>);
}

static SINK: OnceLock<Arc<dyn DiagnosticSink>> = OnceLock::new();

/// 由进程装配安装一次诊断门；不初始化 subscriber，不读取配置，不影响业务。
/// 未安装或重复安装时仍由原记录器输出；调用方拥有窗口的刷新与关闭。
pub fn install_diagnostic_sink(sink: Arc<dyn DiagnosticSink>) {
    let _ = SINK.set(sink);
}

/// 在原因即将丢失处记录安全字段。kind/phase 必须是源码中的固定类别。
pub fn record_error(
    level: DiagnosticLevel,
    safe_kind: SafeErrorKind,
    kind: &'static str,
    phase: &'static str,
    context: DiagnosticContext<'_>,
) {
    let diagnostic = SafeDiagnostic { level, safe_kind, kind, phase, context };
    match SINK.get() {
        Some(sink) => sink.record(&diagnostic),
        None if diagnostic_enabled(&diagnostic) => emit_safe_diagnostic(&diagnostic),
        None => {}
    }
}

/// 输出经过安全投影的事件；供日志门在窗口首条或未启用去重时调用。
pub fn emit_safe_diagnostic(diagnostic: &SafeDiagnostic<'_>) {
    let SafeDiagnostic { level, safe_kind, kind, phase, context } = diagnostic;
    let world = context.world.map(|world| match world {
        SettlementWorld::Main => "main",
        SettlementWorld::Isekai => "isekai",
    });
    let disposition = context.disposition.map(DiagnosticDisposition::label);
    macro_rules! emit {
        ($level:expr) => {
            tracing::event!(
                target: "auction::diagnostics",
                $level,
                safe_kind = safe_kind.label(),
                kind,
                phase,
                worker_loop = context.worker_loop,
                route = context.route,
                round_id = ?context.round_id,
                workflow_id = ?context.workflow_id,
                authorization_id = ?context.authorization_id,
                notification_id = ?context.notification_id,
                evidence_id = ?context.evidence_id,
                obligation_id = ?context.obligation_id,
                delivery_id = ?context.delivery_id,
                auction_id = ?context.auction_id,
                lot_id = ?context.lot_id,
                action_id = ?context.action_id,
                command_id = ?context.command_id,
                world,
                defer_reason = context.defer_reason,
                write_started = context.write_started,
                disposition,
                "安全错误诊断"
            )
        };
    }
    match level {
        DiagnosticLevel::Error => emit!(tracing::Level::ERROR),
        DiagnosticLevel::Warn => emit!(tracing::Level::WARN),
        DiagnosticLevel::Info => emit!(tracing::Level::INFO),
        DiagnosticLevel::Debug => emit!(tracing::Level::DEBUG),
    }
}

/// 只将明确的世界值转成闭集，不保存任意配置文本。
pub fn diagnostic_world(world: &str) -> Option<SettlementWorld> {
    match world {
        "main" => Some(SettlementWorld::Main),
        "isekai" => Some(SettlementWorld::Isekai),
        _ => None,
    }
}

/// 可进入日志的 UUID 投影；不为字符串、token 或任意 DTO 提供实现。
pub trait DiagnosticId {
    fn diagnostic_id(&self) -> Option<Uuid>;
}

/// 统一投影已使用的领域 UUID 与旧账本 UUID 字节，不复制聚合。
pub fn diagnostic_id(value: &impl DiagnosticId) -> Option<Uuid> {
    value.diagnostic_id()
}

impl DiagnosticId for Uuid {
    fn diagnostic_id(&self) -> Option<Uuid> {
        Some(*self)
    }
}
impl DiagnosticId for [u8; 16] {
    fn diagnostic_id(&self) -> Option<Uuid> {
        Some(Uuid::from_bytes(*self))
    }
}

impl DiagnosticId for AuctionId {
    fn diagnostic_id(&self) -> Option<Uuid> {
        Some(self.as_uuid())
    }
}

impl DiagnosticId for LotId {
    fn diagnostic_id(&self) -> Option<Uuid> {
        Some(self.as_uuid())
    }
}

impl DiagnosticId for CommandId {
    fn diagnostic_id(&self) -> Option<Uuid> {
        Some(self.as_uuid())
    }
}

impl DiagnosticId for ProxyAuthorizationId {
    fn diagnostic_id(&self) -> Option<Uuid> {
        Some(self.as_uuid())
    }
}

impl DiagnosticId for SettlementId {
    fn diagnostic_id(&self) -> Option<Uuid> {
        Some(self.as_uuid())
    }
}

/// 去重门只对原事件可见时开启窗口；指标 observer 必须先于此检查。
pub fn diagnostic_enabled(diagnostic: &SafeDiagnostic<'_>) -> bool {
    match diagnostic.level {
        DiagnosticLevel::Error => {
            tracing::enabled!(target: "auction::diagnostics", tracing::Level::ERROR)
        }
        DiagnosticLevel::Warn => {
            tracing::enabled!(target: "auction::diagnostics", tracing::Level::WARN)
        }
        DiagnosticLevel::Info => {
            tracing::enabled!(target: "auction::diagnostics", tracing::Level::INFO)
        }
        DiagnosticLevel::Debug => {
            tracing::enabled!(target: "auction::diagnostics", tracing::Level::DEBUG)
        }
    }
}
