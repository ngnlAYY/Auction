//! 竞价命令的传输替身与回执分类回归。

use std::cell::{Cell, RefCell};
use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::rc::{Rc, Weak};
use std::task::{Poll, Waker};

use dioxus::prelude::EventHandler;
use serde_json::{Value, json};

use super::request::outcome_unknown;
use crate::api::{CommandError, TransportError};

pub(in crate::surface) mod test_support {
    pub(in crate::surface) use super::{
        ABANDON_CONFIRMATION, BID_SUBMIT, FAIL_BUILD, RECOVERY, Request, enable_requests, requests,
    };
}

thread_local! {
    static REQUESTS: RefCell<Vec<Request>> = const { RefCell::new(Vec::new()) };
    pub(in crate::surface) static FAIL_BUILD: Cell<bool> = const { Cell::new(false) };
    pub(in crate::surface) static BID_SUBMIT: Cell<Option<EventHandler<()>>> = const { Cell::new(None) };
    pub(in crate::surface) static RECOVERY: Cell<Option<(EventHandler<()>, EventHandler<()>)>> = const { Cell::new(None) };
    pub(in crate::surface) static ABANDON_CONFIRMATION: Cell<Option<(EventHandler<()>, EventHandler<()>)>> = const { Cell::new(None) };
}

pub(in crate::surface) fn enable_requests() {
    REQUESTS.set(Vec::new());
    CommandError::intercept_test_requests(send);
    FAIL_BUILD.set(false);
    BID_SUBMIT.set(None);
    RECOVERY.set(None);
    ABANDON_CONFIRMATION.set(None);
}

pub(in crate::surface) fn requests() -> Vec<Request> {
    REQUESTS.with_borrow(Clone::clone)
}

type WireResponse = Result<(u16, Option<String>), TransportError>;
type WireFuture = Pin<Box<dyn Future<Output = WireResponse>>>;

#[derive(Default)]
struct Reply {
    result: Option<WireResponse>,
    waker: Option<Waker>,
}

#[derive(Clone)]
pub(in crate::surface) struct Request {
    pub url: String,
    pub user: String,
    pub body: Vec<u8>,
    reply: Weak<RefCell<Reply>>,
}

impl Request {
    pub(in crate::surface) fn in_flight(&self) -> bool {
        self.reply.strong_count() > 0
    }

    pub(in crate::surface) fn complete(&self, result: Result<(), CommandError>) {
        let result = match result {
            Ok(()) => Ok((200, Some(self.success_body()))),
            Err(error) if error.status == 0 => Err(TransportError::Network(None)),
            Err(error) => Ok((
                error.status,
                Some(
                    json!({
                        "code": error.code,
                        "message": "替身拒绝",
                        "data": null,
                    })
                    .to_string(),
                ),
            )),
        };
        let reply = self.reply.upgrade().expect("请求应仍在途");
        let waker = {
            let mut reply = reply.borrow_mut();
            reply.result = Some(result);
            reply.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    fn success_body(&self) -> String {
        let path = self.url.split('/').collect::<Vec<_>>();
        let lot_id = path[4];
        let bid = json!({
            "lot_id": lot_id,
            "leading": true,
            "current_price": 105_000,
            "effective_end_at": "2026-11-18T12:00:00Z",
            "state_version": 2,
        });
        let data = if path[5] == "bids" {
            json!({ "bid": bid })
        } else {
            let payload: Value = serde_json::from_slice(&self.body).unwrap();
            let maximum = payload["maximum_amount"].as_i64().unwrap();
            let mode = payload["mode"].as_str().unwrap_or("slow");
            let lowered = path.len() > 6;
            let initial = if lowered { 250_000 } else { maximum };
            let mut transitions = vec![json!({
                "transition_number": 1,
                "kind": "created",
                "old_maximum_amount": null, "new_maximum_amount": initial,
                "old_mode": null, "new_mode": mode,
                "old_status": null, "new_status": "active",
                "reason": null, "resulting_state_version": 1,
                "created_at": "2026-11-18T12:00:00Z",
            })];
            if lowered {
                transitions.push(json!({
                    "transition_number": 2,
                    "kind": "lowered",
                    "old_maximum_amount": initial, "new_maximum_amount": maximum,
                    "old_mode": mode, "new_mode": mode,
                    "old_status": "active", "new_status": "active",
                    "reason": null, "resulting_state_version": 2,
                    "created_at": "2026-11-18T12:00:01Z",
                }));
            }
            let mut authorization = json!({
                "authorization_id": path.get(6).copied().unwrap_or("018f4c8e-7b2a-4c3d-8e4f-000000000004"),
                "lot_id": lot_id,
                "authorization_sequence": 1,
                "usage_number": 1,
                "mode": mode,
                "maximum_amount": maximum,
                "quota": { "used": 1, "limit": 3 },
                "transitions": transitions,
                "status": "active",
                "state_version": transitions.len(),
            });
            if !lowered {
                authorization["bid"] = bid;
            }
            json!({ "authorization": authorization })
        };
        json!({ "code": "ok", "message": "替身回执", "data": data }).to_string()
    }
}

fn send(url: &str, user: &str, body: &[u8]) -> Result<WireFuture, TransportError> {
    if FAIL_BUILD.replace(false) {
        return Err(TransportError::Network(None));
    }
    let reply = Rc::new(RefCell::new(Reply::default()));
    REQUESTS.with_borrow_mut(|requests| {
        requests.push(Request {
            url: url.to_owned(),
            user: user.to_owned(),
            body: body.to_vec(),
            reply: Rc::downgrade(&reply),
        });
    });
    Ok(Box::pin(poll_fn(move |context| {
        let mut reply = reply.borrow_mut();
        match reply.result.take() {
            Some(result) => Poll::Ready(result),
            None => {
                reply.waker = Some(context.waker().clone());
                Poll::Pending
            }
        }
    })))
}

pub(super) fn observe_recovery(
    in_dialog: bool,
    retry: EventHandler<()>,
    abandon: EventHandler<()>,
) {
    if !in_dialog {
        RECOVERY.set(Some((retry, abandon)));
    }
}

pub(super) fn observe_confirmation(on_close: EventHandler<()>, on_confirm: EventHandler<()>) {
    ABANDON_CONFIRMATION.set(Some((on_close, on_confirm)));
}

#[test]
fn a_retry_rejection_only_settles_if_the_ledger_was_consulted() {
    for (status, code, initial, retry) in [
        (409, "bid_too_low", false, false),
        (409, "auction_not_open", false, false),
        (409, "proxy_maximum_not_lower", false, false),
        (409, "command_conflict", false, true),
        (401, "unauthenticated", false, true),
        (409, "session_changed", false, true),
        (429, "rate_limited", false, true),
        (404, "not_found", false, true),
        (0, "request_timeout", true, true),
        (200, "invalid_response", true, true),
        (500, "temporarily_unavailable", true, true),
    ] {
        let error = CommandError { status, code: code.to_owned() };
        assert_eq!(outcome_unknown(&error, false), initial, "{error:?}");
        assert_eq!(outcome_unknown(&error, true), retry, "{error:?}");
    }
}
