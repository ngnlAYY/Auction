//! 通过真实认证外壳驱动退出决定和用户页的一次性 Key 交付。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use auction_contracts::AdminUserPaginationResponse;
use dioxus::core::DynamicNode;
use dioxus::history::MemoryHistory;
use dioxus::prelude::*;

use super::{NOW, SessionFixture, logout_ticket, read_ticket};
use crate::api::AdminUserPage;
use crate::surface::auctions::editor::tests::{
    RecordRead, button, click_button, has_live_text, init_i18n, mount_writer, settle_writer,
};
use crate::surface::auth::effects::TestSession;
use crate::surface::auth::session::{Effect, Event, LogoutOutcome, Ticket};
use crate::surface::realtime::TestPage;
use crate::surface::router::Route;
use crate::surface::users::UsersPageFixture;

struct Shell {
    page: TestPage,
    fixture: Rc<SessionFixture>,
    session: Rc<Cell<Option<TestSession>>>,
    effects: Rc<RefCell<Vec<Effect>>>,
}

impl Shell {
    fn mount(route: Route) -> Self {
        let fixture = Rc::new(SessionFixture::new("current-cookie", 900));
        let session = Rc::new(Cell::new(None));
        let effects = Rc::new(RefCell::new(Vec::new()));
        let (page, _) = mount_writer({
            let fixture = fixture.clone();
            let session = session.clone();
            let effects = effects.clone();
            move || {
                init_i18n();
                use_context_provider(|| {
                    Rc::new(MemoryHistory::with_initial_path(route.clone())) as Rc<dyn History>
                });
                use_context_provider(|| {
                    Rc::new(UsersPageFixture {
                        fetch: Box::new(|_, page, page_size| AdminUserPage {
                            users: Vec::new(),
                            pagination: AdminUserPaginationResponse { page, page_size, total: 0 },
                        }),
                        retry_correction: Cell::new(None),
                    })
                });
                let mut state = use_signal(|| fixture.authenticated());
                let mut view = use_signal(|| state.peek().view());
                let fixture = fixture.clone();
                let effects = effects.clone();
                let send = use_callback(move |event| {
                    let before = state.peek().clone();
                    let (next, produced) = fixture.apply(before, event, NOW);
                    effects.borrow_mut().extend(produced);
                    view.set(next.view());
                    state.set(next);
                });
                let projection = TestSession(view, send);
                session.set(Some(projection));
                use_context_provider(|| projection);
                rsx! { Router::<Route> {} }
            }
        });
        Self { page, fixture, session, effects }
    }

    fn send(&mut self, event: Event) -> Vec<Effect> {
        self.session.get().unwrap().1.call(event);
        settle_writer(&mut self.page);
        self.effects.take()
    }

    fn confirm(&mut self, ticket: Ticket) {
        assert!(self.send(Event::ReadSending(ticket)).is_empty());
        let effects = self.send(self.fixture.read_ok(ticket));
        assert!(
            !effects.iter().any(|effect| matches!(effect, Effect::Read(_) | Effect::Logout(..)))
        );
    }

    fn enter_logout_recovery(&mut self) -> Ticket {
        click_button(&mut self.page, "退出登录");
        let ticket = logout_ticket(&self.effects.take());
        self.send(Event::LogoutSending(ticket));
        let effects = self.send(Event::LogoutFinished(
            ticket,
            self.fixture.cookie.clone(),
            LogoutOutcome::Unknown("request_timeout".into()),
        ));
        let review = read_ticket(&effects);
        assert!(button(&self.page, "继续退出").unwrap().1);
        assert!(button(&self.page, "留在后台").unwrap().1);
        self.confirm(review);
        assert!(has_live_text(&self.page, "退出尚未确认，此标签页的写入已关闭。"));
        assert!(!self.session.get().unwrap().0.peek().can_write);
        ticket
    }
}

#[test]
fn admin_shell_continue_after_unknown_logout_sends_one_new_delete() {
    let mut shell = Shell::mount(Route::Status {});
    let first = shell.enter_logout_recovery();
    assert!(has_live_text(&shell.page, "服务状态"));
    click_button(&mut shell.page, "继续退出");
    let effects = shell.effects.take();
    let retry = logout_ticket(&effects);
    assert_ne!(retry, first);
    assert!(!effects.iter().any(|effect| matches!(effect, Effect::Read(_))));
    assert!(button(&shell.page, "继续退出").unwrap().1);
    assert!(button(&shell.page, "留在后台").unwrap().1);
}

#[test]
fn admin_shell_stay_after_unknown_logout_reads_before_restoring_writes() {
    let mut shell = Shell::mount(Route::Status {});
    shell.enter_logout_recovery();
    assert!(has_live_text(&shell.page, "服务状态"));
    click_button(&mut shell.page, "留在后台");
    let effects = shell.effects.take();
    let stay = read_ticket(&effects);
    assert!(!effects.iter().any(|effect| matches!(effect, Effect::Logout(..))));
    assert!(button(&shell.page, "继续退出").unwrap().1);
    assert!(button(&shell.page, "留在后台").unwrap().1);
    assert!(button(&shell.page, "退出登录").is_none());
    assert!(!shell.session.get().unwrap().0.peek().can_write);

    shell.confirm(stay);
    assert!(shell.session.get().unwrap().0.peek().can_write);
    assert!(button(&shell.page, "继续退出").is_none());
    assert!(button(&shell.page, "留在后台").is_none());
    assert!(!button(&shell.page, "退出登录").unwrap().1);
}

#[test]
fn users_page_requires_manual_open_for_key_delivered_while_hidden() {
    let mut shell = Shell::mount(Route::Users {});
    assert!(has_live_text(&shell.page, "用户与 Key"));
    let dom = shell.page.dom();
    let source = users_scope(dom, dom.base_scope().root_node()).expect("用户页已挂载");
    // 在真实用户页的来源上提交命令，让已裁定结果经过生产的成功交付回调。
    let response = Rc::new(RecordRead::new());
    dom.in_scope(source, || {
        let response = response.clone();
        assert!(UsersPageFixture::submit_generated_key(move |_| {
            let response = response.clone();
            async move { response.fetch().await.map(Some) }
        }));
    });
    settle_writer(&mut shell.page);
    assert_eq!(response.started(), 1);

    let epoch = shell.session.get().unwrap().0.peek().epoch;
    let effects = shell.send(Event::WriteReviewIdentity {
        epoch,
        session: SessionFixture::new("replacement-cookie", 900).session,
    });
    let ticket = read_ticket(&effects);
    let key = "用户来源页的测试 Key";
    response.respond(Ok(key.to_owned()));
    settle_writer(&mut shell.page);
    assert!(!has_live_text(&shell.page, key));
    assert!(button(&shell.page, "查看生成的 Key").unwrap().1);

    shell.confirm(ticket);
    assert!(has_live_text(&shell.page, "用户与 Key"));
    assert!(!has_live_text(&shell.page, key), "同代恢复后明文必须等到手动查看");
    click_button(&mut shell.page, "查看生成的 Key");
    assert!(has_live_text(&shell.page, key));
    click_button(&mut shell.page, "关闭");
    assert!(!has_live_text(&shell.page, key));
    assert!(button(&shell.page, "查看生成的 Key").is_none());
}

fn users_scope(dom: &VirtualDom, vnode: &VNode) -> Option<ScopeId> {
    vnode.dynamic_nodes.iter().enumerate().find_map(|(index, node)| match node {
        DynamicNode::Component(component) => {
            let scope = component.mounted_scope(index, vnode, dom)?;
            if component.name.rsplit("::").next() == Some("Users") {
                Some(scope.id())
            } else {
                users_scope(dom, scope.try_root_node()?)
            }
        }
        DynamicNode::Fragment(children) => {
            children.iter().find_map(|child| users_scope(dom, child))
        }
        _ => None,
    })
}
