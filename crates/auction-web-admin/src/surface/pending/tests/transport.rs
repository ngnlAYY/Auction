//! 用原始传输结果验证业务写入的未决边界与页面可见的响应状态。

use std::{cell::RefCell, collections::VecDeque, rc::Rc};

use auction_web_shared::TransportError;
use dioxus::prelude::{ReadableExt, ScopeId, VirtualDom};

use super::{
    CommandHarness, MAIL_TEMPLATE_ENTITY, rejections, send_mail_template,
    stub_mail_template_transport, test_page,
};
use crate::api::{AdminWriteFailure, WriteDispatchStub, admin_fixture};
use crate::surface::pending::PendingCommands;

const PREFLIGHT_ERRORS: [&str; 3] = ["network_error", "request_timeout", "mail_template_conflict"];

fn mount_commands() -> (VirtualDom, PendingCommands) {
    let control = Rc::new(CommandHarness::default());
    let mut dom = VirtualDom::new(test_page).with_root_context(control.clone());
    dom.rebuild_in_place();
    (dom, control.pending.get().unwrap())
}

fn submit_observed(commands: PendingCommands) -> Rc<RefCell<Vec<AdminWriteFailure>>> {
    let failures = Rc::new(RefCell::new(Vec::new()));
    let observed = failures.clone();
    assert!(commands.submit(
        MAIL_TEMPLATE_ENTITY.to_owned(),
        "保存模板".to_owned(),
        move |command_id| {
            let observed = observed.clone();
            async move {
                let result = send_mail_template(command_id).await;
                if let Err(failure) = &result {
                    observed.borrow_mut().push(failure.clone());
                }
                result
            }
        },
        |()| panic!("拒绝不能交付成功回执"),
        |_| {},
    ));
    failures
}

#[test]
fn write_preflight_errors_do_not_dispatch_or_hold_a_new_command() {
    for code in PREFLIGHT_ERRORS {
        let (mut dom, commands) = mount_commands();
        let reviews = Rc::new(RefCell::new(VecDeque::from([code])));
        let failures = dom.in_scope(ScopeId::APP, || {
            let reviews = reviews.clone();
            stub_mail_template_transport(
                move || Err(reviews.borrow_mut().pop_front().unwrap().to_owned()),
                |_| panic!("写前复核失败后不得派发业务写"),
            );
            submit_observed(commands)
        });
        dom.process_events();
        assert!(reviews.borrow().is_empty(), "{code}");
        assert_eq!(failures.borrow().as_slice(), &[AdminWriteFailure::not_sent(code)]);
        assert_eq!(failures.borrow()[0].observed_response_status(), None, "{code}");
        assert!(!commands.contains(MAIL_TEMPLATE_ENTITY), "{code}");
    }
}

#[test]
fn dispatched_write_failure_keeps_the_command_across_preflight_errors_and_retries() {
    let (mut dom, commands) = mount_commands();
    let reviews = Rc::new(RefCell::new(VecDeque::from([
        Ok("csrf-fixture"),
        Err(PREFLIGHT_ERRORS[0]),
        Err(PREFLIGHT_ERRORS[1]),
        Err(PREFLIGHT_ERRORS[2]),
        Ok("csrf-fixture"),
    ])));
    let requests = Rc::new(RefCell::new(Vec::new()));
    let failures = dom.in_scope(ScopeId::APP, || {
        let reviews = reviews.clone();
        let requests = requests.clone();
        stub_mail_template_transport(
            move || {
                reviews.borrow_mut().pop_front().unwrap().map(str::to_owned).map_err(str::to_owned)
            },
            move |body| {
                let mut requests = requests.borrow_mut();
                requests.push(body.to_vec());
                if requests.len() == 1 {
                    WriteDispatchStub::Failed { built: true, error: TransportError::Network(None) }
                } else {
                    WriteDispatchStub::Response {
                        status: 409,
                        body: Some(admin_fixture!("error-mail-template-conflict").as_bytes()),
                    }
                }
            },
        );
        submit_observed(commands)
    });
    dom.process_events();
    assert_eq!(reviews.borrow().len(), 4);
    assert_eq!(requests.borrow().len(), 1);
    assert_eq!(
        failures.borrow().as_slice(),
        &[AdminWriteFailure::outcome_unknown("network_error")]
    );
    let command_id = commands.host.entries.peek()[0].identity.command_id;
    assert!(commands.host.entries.peek()[0].unknown);

    for (index, code) in PREFLIGHT_ERRORS.into_iter().enumerate() {
        dom.in_scope(ScopeId::APP, || commands.host.retry(command_id));
        dom.process_events();
        assert_eq!(reviews.borrow().len(), 3 - index, "{code}");
        assert_eq!(requests.borrow().len(), 1, "{code}");
        assert_eq!(failures.borrow().last(), Some(&AdminWriteFailure::not_sent(code)), "{code}");
        let entries = commands.host.entries.peek();
        assert_eq!(entries[0].identity.command_id, command_id, "{code}");
        assert!(entries[0].unknown, "{code}");
        assert!(rejections(commands).peek().is_empty(), "{code}");
    }

    dom.in_scope(ScopeId::APP, || commands.host.retry(command_id));
    dom.process_events();
    assert!(reviews.borrow().is_empty());
    let requests = requests.borrow();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1], requests[0]);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&requests[0]).unwrap(),
        serde_json::json!({
            "command_id": command_id,
            "expected_revision": 5,
            "body": "首次确认的正文",
        }),
    );
    assert!(!commands.contains(MAIL_TEMPLATE_ENTITY));
    let notices = rejections(commands);
    let notices = notices.peek();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].identity.command_id, command_id);
    assert_eq!(
        notices[0].result.as_ref().err().map(String::as_str),
        Some("mail_template_conflict")
    );
}

#[test]
fn interrupted_and_malformed_409_keep_pending_with_distinct_observed_statuses() {
    let (mut dom, commands) = mount_commands();
    let failures = dom.in_scope(ScopeId::APP, || {
        let bodies = RefCell::new(VecDeque::from([None, Some(b"{".as_slice())]));
        stub_mail_template_transport(
            || Ok("csrf-fixture".to_owned()),
            move |_| WriteDispatchStub::Response {
                status: 409,
                body: bodies.borrow_mut().pop_front().unwrap(),
            },
        );
        submit_observed(commands)
    });
    dom.process_events();
    let command_id = commands.host.entries.peek()[0].identity.command_id;
    for (index, status, code) in [(0, None, "network_error"), (1, Some(409), "http_409")] {
        if index == 1 {
            dom.in_scope(ScopeId::APP, || commands.host.retry(command_id));
            dom.process_events();
        }
        let failures = failures.borrow();
        let failure = &failures[index];
        assert!(matches!(failure, AdminWriteFailure::OutcomeUnknown { .. }));
        assert_eq!(failure.observed_response_status(), status);
        assert_eq!(failure.display_code(), code);
        let entries = commands.host.entries.peek();
        assert_eq!(entries[0].identity.command_id, command_id);
        assert!(entries[0].unknown);
        assert!(!commands.is_busy(MAIL_TEMPLATE_ENTITY));
        assert!(rejections(commands).peek().is_empty());
    }
}
