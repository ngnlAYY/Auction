//! 在代理真实表单上复核草稿与授权读取失败的反馈。

use std::cell::Cell;

use auction_web_shared::i18n::{DEFAULT_PUBLIC_LOCALE, build_bundle};
use auction_web_shared::use_confirm_command;
use dioxus::dioxus_core::{AttributeValue, Mutation, Mutations};
use dioxus::prelude::*;
use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;
use uuid::Uuid;

use super::{ProxyForm, ProxyFormInput};
use crate::api::{OwnedProxy, PublicSession};
use crate::surface::commands::pending::{ProxyConfirmCommand, PublicCommandIdentity};
use crate::surface::commands::session::use_command_session_provider;
use crate::surface::router::PUBLIC_LOCALE_CATALOG;
use crate::surface::{PublicCommandOwner, PublicSessionResource};

type ReadResult = Option<Result<Option<OwnedProxy>, String>>;
type Draft = (Signal<String>, Signal<String>, Signal<Option<String>>);

thread_local! {
    static READ: Cell<Option<Signal<ReadResult>>> = const { Cell::new(None) };
    static DRAFT: Cell<Option<Draft>> = const { Cell::new(None) };
}

pub(super) fn observe_draft(
    amount: Signal<String>,
    mode: Signal<String>,
    validation: Signal<Option<String>>,
) {
    DRAFT.set(Some((amount, mode, validation)));
}

#[component]
fn ProxyApp() -> Element {
    try_use_init_i18n_from_bundle(|| {
        build_bundle(
            &DEFAULT_PUBLIC_LOCALE,
            PUBLIC_LOCALE_CATALOG.default_ftl,
            &DEFAULT_PUBLIC_LOCALE,
            None,
        )
    })
    .expect("加载公开端英文译文");
    let user = Uuid::from_u128(1);
    let user_id = user.to_string();
    let owner = use_signal(|| Some(user_id.clone()));
    use_context_provider(|| PublicCommandOwner(owner));
    use_context_provider(|| PublicCommandIdentity(user));
    let session: PublicSessionResource = use_resource(move || {
        let user_id = user.to_string();
        async move {
            Ok(Some(PublicSession {
                authenticated: true,
                user_id,
                username: "alice".to_owned(),
                expires_at: None,
                absolute_expires_at: None,
            }))
        }
    });
    use_context_provider(|| session);
    use_command_session_provider(Some(user_id));
    let refresh_generation = use_context_provider(|| Signal::new(0_u64));
    let read = use_signal(|| Some(Ok(None)));
    READ.set(Some(read));
    let input = ProxyFormInput {
        lot_id: "test-lot".to_owned(),
        can_bid: true,
        current_price: 100_000,
        next_bid: 105_000,
        lot_status: "live".to_owned(),
        allow_slow: true,
        default_mode: Some("fast".to_owned()),
        leading: false,
        blocked_reason: String::new(),
        proxy: read(),
        refresh_generation,
        saved_authorization: use_signal(|| None),
        create_command: ProxyConfirmCommand(use_confirm_command()),
        lower_command: ProxyConfirmCommand(use_confirm_command()),
        on_close: use_callback(|()| {}),
    };
    rsx! { ProxyForm { input } }
}

fn settle(dom: &mut VirtualDom) -> Vec<Mutation> {
    let mut mutations = Mutations::default();
    for _ in 0..8 {
        dom.process_events();
        dom.render_immediate(&mut mutations);
    }
    mutations.edits
}

fn read_result(dom: &mut VirtualDom, result: ReadResult) -> Vec<Mutation> {
    let mut signal = READ.get().expect("读取状态已挂载");
    dom.in_runtime(|| signal.set(result));
    settle(dom)
}

#[test]
fn failed_recheck_leaves_the_selected_mode_amount_and_validation_in_the_same_form() {
    let mut dom = VirtualDom::new(ProxyApp);
    dom.rebuild_in_place();
    settle(&mut dom);
    let (mut amount, mut mode, mut validation) = DRAFT.get().expect("表单已挂载");
    dom.in_runtime(|| {
        amount.set("1K".to_owned());
        mode.set("slow".to_owned());
        validation.set(Some("先前校验提示".to_owned()));
    });
    settle(&mut dom);

    let failed = read_result(&mut dom, Some(Err("network_error".to_owned())));
    assert!(failed.iter().any(|edit| matches!(edit,
        Mutation::CreateTextNode { value, .. } if value.contains("Proxy authorization is unavailable")
    )));
    assert!(
        failed.iter().any(|edit| matches!(
            edit,
            Mutation::SetAttribute { name: "disabled", value: AttributeValue::Bool(true), .. }
        )),
        "失败时保留表单并禁用它的提交"
    );

    let retrying = read_result(&mut dom, None);
    assert!(
        !retrying.iter().any(|edit| matches!(
            edit,
            Mutation::SetAttribute { name: "disabled", value: AttributeValue::Bool(false), .. }
        )),
        "授权确认前保持禁用"
    );
    let recovered = read_result(&mut dom, Some(Ok(None)));
    assert!(recovered.iter().any(|edit| matches!(
        edit,
        Mutation::SetAttribute { name: "disabled", value: AttributeValue::Bool(false), .. }
    )));
    assert!(
        !recovered.iter().any(|edit| matches!(edit,
            Mutation::NewEventListener { name, .. } if name == "input" || name == "change"
        )),
        "原输入与选项应保持挂载"
    );
    let (shown_amount, shown_mode, shown_validation) = DRAFT.get().expect("表单仍然存在");
    assert_eq!((shown_amount, shown_mode, shown_validation), (amount, mode, validation));
    assert_eq!((shown_amount(), shown_mode()), ("1K".to_owned(), "slow".to_owned()));
    assert_eq!(shown_validation().as_deref(), Some("先前校验提示"));
}
