//! 原生装备标记按钮：忙碌、冲突与未知结果重放均可见且可用键盘操作。

use super::*;
use uuid::Uuid;

/// 只消费整场 owner 的状态；不会在目录每行发起 GET。
#[component]
pub(crate) fn LotWatchButton(lot_id: String, lot_code: String) -> Element {
    let context = use_context::<WatchContext>();
    let submit = use_context::<WatchSubmit>();
    let session = context.session;
    if !context.identity_matches() {
        let snapshot = session.read();
        return match snapshot.as_ref() {
            Some(Ok(None)) => rsx! {
                Link {
                    class: "public-lot-watch__login",
                    to: login_href_for_current_location(),
                    {t!("watch-login")}
                }
            },
            Some(Err(_)) => rsx! {
                span { class: "public-lot-watch__error", role: "alert", {t!("session-error")} }
            },
            _ => rsx! { span { role: "status", {t!("watch-loading")} } },
        };
    }

    let authoritative = context.authoritative(&lot_id);
    let watched = authoritative.map(|(watched, _)| watched).unwrap_or_else(|| {
        context.cached.read().as_ref().is_some_and(|page| {
            page.watches.iter().any(|watch| watch.lot_id == lot_id && watch.watched)
        })
    });
    let pending = context.commands.read().get(&lot_id).cloned();
    let busy = pending.as_ref().is_some_and(|pending| pending.busy);
    let error = pending.as_ref().and_then(|pending| pending.error.as_ref());
    let uncertain = pending.as_ref().is_some_and(|pending| pending.uncertain);
    let conflict = error.is_some_and(|error| error.status == 409);
    let error_message = if uncertain {
        t!("watch-unknown")
    } else if conflict {
        t!("watch-conflict")
    } else {
        t!("watch-save-error")
    };
    let error_id = format!("public-lot-watch-error-{lot_id}");
    let click_context = context.clone();
    let click_lot = lot_id.clone();
    let retry_context = context.clone();
    let retry_lot = lot_id.clone();
    let mut refresh_context = context.clone();

    rsx! {
        span { class: "public-lot-watch", aria_busy: busy.to_string(),
            button {
                class: "public-lot-watch__toggle",
                r#type: "button",
                aria_pressed: watched.to_string(),
                aria_label: t!("watch-toggle", code: lot_code),
                aria_describedby: if error.is_some() { error_id.clone() } else { String::new() },
                disabled: authoritative.is_none() || pending.is_some(),
                onclick: move |_| {
                    if !click_context.identity_matches()
                        || click_context.commands.read().contains_key(&click_lot)
                    {
                        return;
                    }
                    let Some((watched, version)) = click_context.authoritative(&click_lot) else {
                        return;
                    };
                    submit.0.call((click_lot.clone(), PublicSetLotWatchRequest {
                        command_id: Uuid::new_v4().to_string(),
                        watched: !watched,
                        channel: "mm".to_owned(),
                        expected_version: version,
                    }));
                },
                span { class: "public-lot-watch__icon", aria_hidden: "true",
                    if watched { "★" } else { "☆" }
                }
                span { class: "public-lot-watch__label",
                    if busy { {t!("watch-saving")} }
                    else if watched { {t!("watch-on")} }
                    else { {t!("watch-off")} }
                }
            }
            if error.is_some() {
                span { id: error_id.clone(), class: "public-lot-watch__error", role: "alert", "{error_message}" }
                if uncertain {
                    button {
                        r#type: "button",
                        onclick: move |_| {
                            if !retry_context.identity_matches() { return; }
                            let pending = retry_context.commands.read().get(&retry_lot).cloned();
                            if let Some(pending) = pending
                                && !pending.busy
                                && pending.uncertain
                            {
                                submit.0.call((retry_lot.clone(), pending.payload));
                            }
                        },
                        {t!("watch-retry-command")}
                    }
                } else {
                    button {
                        r#type: "button",
                        onclick: move |_| {
                            if !refresh_context.identity_matches() { return; }
                            {
                                let mut commands = refresh_context.commands.write();
                                // 即使旧刷新事件延迟到达，也不能清除正在提交或结果未知的命令。
                                if commands.get(&lot_id).is_some_and(|pending| pending.busy || pending.uncertain) {
                                    return;
                                }
                                commands.remove(&lot_id);
                            }
                            refresh_context.invalidate();
                        },
                        {t!("watch-refresh")}
                    }
                }
            }
        }
    }
}
