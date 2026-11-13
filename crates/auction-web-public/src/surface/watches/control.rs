//! 原生装备标记按钮：忙碌、冲突与未知结果重放均可见且可用键盘操作。

use super::*;
use uuid::Uuid;

/// 目录行编号左侧的“已标记”记号：与标记按钮读取同一份整场状态，不发请求、不可操作。
/// 未标记、匿名或不在标记作用域内时不渲染；位置由目录样式预留，出现与消失不推动编号。
#[component]
pub(crate) fn LotWatchMark(lot_id: String) -> Element {
    let watched = try_use_context::<WatchContext>()
        .is_some_and(|context| context.identity_matches() && context.displayed(&lot_id));
    if !watched {
        return rsx! {};
    }
    rsx! {
        span {
            class: "public-lot-watch-mark",
            role: "img",
            aria_label: t!("watch-on"),
            title: t!("watch-on"),
            svg {
                view_box: "0 0 24 24",
                fill: "currentColor",
                stroke: "currentColor",
                stroke_width: "1.5",
                stroke_linecap: "round",
                stroke_linejoin: "round",
                "aria-hidden": "true",
                path { d: "M6 3h12a1 1 0 0 1 1 1v17l-7-4-7 4V4a1 1 0 0 1 1-1z" }
            }
        }
    }
}

/// 只消费整场 owner 的状态；不会在目录每行发起 GET。
#[component]
pub(crate) fn LotWatchButton(lot_id: String, lot_code: String, icon_only: Option<bool>) -> Element {
    let icon_only = icon_only.unwrap_or(false);
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
                    aria_label: t!("watch-login"),
                    if icon_only {
                        span { class: "public-lot-watch__icon", "aria-hidden": "true",
                            svg {
                                view_box: "0 0 24 24",
                                fill: "none",
                                stroke: "currentColor",
                                stroke_width: "1.5",
                                stroke_linecap: "round",
                                stroke_linejoin: "round",
                                path { d: "M6 3h12a1 1 0 0 1 1 1v17l-7-4-7 4V4a1 1 0 0 1 1-1z" }
                            }
                        }
                    } else {
                        {t!("watch-login")}
                    }
                }
            },
            Some(Err(_)) => rsx! {
                span { class: "public-lot-watch__error", role: "alert", {t!("session-error")} }
            },
            _ => rsx! {},
        };
    }

    let authoritative = context.authoritative(&lot_id);
    let pending = context.commands.read().get(&lot_id).cloned();
    let watched = context.displayed(&lot_id);
    let busy = pending.as_ref().is_some_and(|pending| pending.busy);
    let error = pending.as_ref().and_then(|pending| pending.error.as_ref());
    let uncertain = pending.as_ref().is_some_and(|pending| pending.uncertain);
    let conflict = error.is_some_and(|error| error.status == 409 && error.code == "watch_conflict");
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
                    submit.0.call((click_context.auction_id.clone(), click_lot.clone(), PublicSetLotWatchRequest {
                        command_id: Uuid::new_v4().to_string(),
                        watched: !watched,
                        channel: "mm".to_owned(),
                        expected_version: version,
                    }));
                },
                span { class: "public-lot-watch__icon", aria_hidden: "true",
                    svg {
                        view_box: "0 0 24 24",
                        fill: if watched { "currentColor" } else { "none" },
                        stroke: "currentColor",
                        stroke_width: "1.5",
                        stroke_linecap: "round",
                        stroke_linejoin: "round",
                        path { d: "M6 3h12a1 1 0 0 1 1 1v17l-7-4-7 4V4a1 1 0 0 1 1-1z" }
                    }
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
                                submit.0.call((retry_context.auction_id.clone(), retry_lot.clone(), pending.payload));
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
