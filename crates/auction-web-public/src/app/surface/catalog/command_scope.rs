//! 页面级竞价弹窗宿主；读取分支与目录阶段只更新能力，不拥有已提交命令。

use super::*;
use auction_web_shared::{Dialog, use_confirm_command};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CatalogAction {
    Bid,
    Proxy,
}

/// 这里只保存弹窗展示输入；UUID、冻结负载和未知结果仍由共享 ConfirmCommand 唯一管理。
#[derive(Clone, PartialEq)]
pub(super) struct CommandDialogInput {
    pub lot_id: String,
    pub lot_code: String,
    pub action: CatalogAction,
    pub can_bid: bool,
    pub current_price: i64,
    pub next_bid: i64,
    pub lot_status: String,
    pub allow_slow: bool,
    pub ordinary_blocked: bool,
    pub leading: bool,
    pub blocked_reason: String,
    pub proxy: Option<Result<Option<crate::api::OwnedProxy>, String>>,
}

#[derive(Clone, Copy)]
pub(super) struct CommandDialogContext(pub Signal<Option<CommandDialogInput>>);

/// 身份由外壳明确确认；暂时 GET 错误或 Pending 不改变宿主身份。
#[component]
pub(crate) fn CatalogCommandScope(children: Element) -> Element {
    let owner = use_context::<PublicCommandOwner>().0;
    let user_id = owner();
    // 单元素 keyed 列表建立真正的身份生命周期；静态子组件的 key 不会替换 scope。
    rsx! {
        for (user_id, children) in [(user_id, children)] {
            IdentityCommandScope { key: "{user_id:?}", user_id, children }
        }
    }
}

#[component]
fn IdentityCommandScope(user_id: Option<String>, children: Element) -> Element {
    let dialog = use_signal(|| None::<CommandDialogInput>);
    use_context_provider(|| CommandDialogContext(dialog));
    let pending_watches = watches::use_watch_command_owner(user_id.clone());
    let user_id = user_id
        .as_deref()
        .and_then(|value| uuid::Uuid::parse_str(value).ok())
        .unwrap_or_else(uuid::Uuid::nil);
    use_context_provider(|| commands::PublicCommandIdentity(user_id));
    let input = dialog.read().clone();
    rsx! {
        {children}
        {pending_watches}
        if let Some(input) = input {
            // 一个页面只有一个模态弹窗；关闭才卸载，后台 GET/分页/阶段切换均不替换它。
            CatalogCommandDialog { input, dialog }
        }
    }
}

#[component]
fn CatalogCommandDialog(
    input: CommandDialogInput,
    mut dialog: Signal<Option<CommandDialogInput>>,
) -> Element {
    let refresh_generation = use_context::<Signal<u64>>();
    let session = use_context::<PublicSessionResource>();
    let user_id = use_context::<commands::PublicCommandIdentity>().0;
    let authenticated = *session.state().read() == UseResourceState::Ready
        && session.read().as_ref().is_some_and(|snapshot| {
            matches!(snapshot, Ok(Some(identity)) if identity.authenticated
                && uuid::Uuid::parse_str(&identity.user_id).ok() == Some(user_id))
        });
    let proxy_success = use_signal(|| None::<Box<ProxyCommandAuthorization>>);
    // 成功投影与命令状态都由稳定宿主持有；表单替换不能取消任务或丢失结果。
    let bid_success = use_signal(|| None::<commands::BidSuccess>);
    let bid_command = commands::BidConfirmCommand(use_confirm_command());
    let proxy_create_command = commands::ProxyConfirmCommand(use_confirm_command());
    let proxy_lower_command = commands::ProxyConfirmCommand(use_confirm_command());
    let busy = (bid_command.submitting)()
        || (proxy_create_command.submitting)()
        || (proxy_lower_command.submitting)();
    let close = EventHandler::new(move |_: ()| {
        if !(bid_command.submitting)()
            && !(proxy_create_command.submitting)()
            && !(proxy_lower_command.submitting)()
        {
            // 显式关闭沿用原有取消契约；自动重读绝不走这个路径。
            dialog.set(None);
        }
    });
    let title = match input.action {
        CatalogAction::Bid => t!("bid-dialog-title", code: input.lot_code.clone()),
        CatalogAction::Proxy => t!("proxy-dialog-title", code: input.lot_code.clone()),
    };
    let can_bid = input.can_bid && authenticated;

    rsx! {
        // 遮罩不关闭；busy 禁用 Escape/关闭，焦点管理仍由 PublicShell 统一提供。
        Dialog {
            title,
            close_label: t!("catalog-close"),
            busy,
            on_close: close,
            if input.action == CatalogAction::Bid {
                if (bid_command.uncertain)() {
                    commands::BidRetry {
                        lot_id: input.lot_id,
                        lot_code: input.lot_code,
                        command: bid_command,
                        success: bid_success,
                        on_close: close,
                    }
                } else {
                    commands::BidForm { input: commands::BidFormInput {
                        lot_id: input.lot_id,
                        lot_code: input.lot_code,
                        current_price: input.current_price,
                        next_bid: input.next_bid,
                        lot_status: input.lot_status,
                        can_bid,
                        blocked: input.ordinary_blocked,
                        blocked_reason: input.blocked_reason,
                        command: bid_command,
                        success: bid_success,
                        on_close: close,
                    } }
                }
            } else {
                {commands::proxy_form(commands::ProxyFormInput {
                    lot_id: input.lot_id,
                    can_bid,
                    current_price: input.current_price,
                    next_bid: input.next_bid,
                    lot_status: input.lot_status,
                    allow_slow: input.allow_slow,
                    leading: input.leading,
                    blocked_reason: input.blocked_reason,
                    proxy: input.proxy,
                    refresh_generation,
                    saved_authorization: proxy_success,
                    create_command: proxy_create_command,
                    lower_command: proxy_lower_command,
                    on_close: close,
                })}
            }
        }
    }
}
