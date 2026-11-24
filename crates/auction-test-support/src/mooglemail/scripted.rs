//! 测试登记的待领取邮件：页面由调用测试从已脱敏的真实语料派生后交给替身，替身只按远端状态在
//! 领取前/后两份页面间切换，保证附件至多被取走一次，并可让一次领取 POST 迟到完成。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::{Method, StatusCode};
use axum::response::{Html, IntoResponse, Response};

use super::{MailDouble, MailState};

/// 测试登记的待领取邮件：领取前/后两份页面由调用方提供，替身只记录远端状态与领取次数。
pub(super) struct ScriptedTake {
    mid: String,
    action: ScriptedAction,
    claimable: String,
    taken: String,
    /// 远端附件是否已被取走。
    removed: bool,
    /// 迟到的领取：POST 已到达但尚未生效，在下一次读取该邮件的响应生成之后才生效。
    deferred: bool,
    /// 下一次领取 POST 迟到完成并延迟响应（客户端据此超时）。
    late_post: Option<Duration>,
    /// 远端真正取走附件的次数；正确的协议下至多一次。
    removals: usize,
    take_posts: usize,
    return_posts: usize,
    returns: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScriptedAction {
    Take,
    Return,
}

impl MailDouble {
    /// 登记一封待领取邮件：读取详情时按远端状态回放领取前或领取后的页面，领取 POST 取走附件
    /// 并回放领取后的页面（已取走时同样回放领取后的页面，不重复取走）。页面由调用方从真实语料
    /// 派生，替身不编造页面。
    pub fn script_take_mail(&self, mid: &str, claimable: String, taken: String) {
        self.state.lock().expect("test state poisoned").scripted = Some(ScriptedTake {
            mid: mid.to_owned(),
            action: ScriptedAction::Take,
            claimable,
            taken,
            removed: false,
            deferred: false,
            late_post: None,
            removals: 0,
            take_posts: 0,
            return_posts: 0,
            returns: 0,
        });
    }

    /// 登记一封待退回邮件：GET 在退回前/后页面间切换，Return POST 只生效一次。
    pub fn script_return_mail(&self, mid: &str, before: String, after: String) {
        self.state.lock().expect("test state poisoned").scripted = Some(ScriptedTake {
            mid: mid.to_owned(),
            action: ScriptedAction::Return,
            claimable: before,
            taken: after,
            removed: false,
            deferred: false,
            late_post: None,
            removals: 0,
            take_posts: 0,
            return_posts: 0,
            returns: 0,
        });
    }

    /// 下一次领取 POST 迟到完成：到达时只登记，`delay` 之后才响应（客户端先超时）；领取在下一次
    /// 读取该邮件的响应生成之后才生效，复现“旧 POST 迟到完成与重排后的新预检交错”。
    pub fn delay_next_take(&self, delay: Duration) {
        if let Some(scripted) = self.state.lock().expect("test state poisoned").scripted.as_mut() {
            scripted.late_post = Some(delay);
        }
    }

    /// 让已登记的迟到领取立即生效，复现“旧 POST 在新预检之前完成”。
    pub fn complete_deferred_take(&self) {
        if let Some(scripted) = self.state.lock().expect("test state poisoned").scripted.as_mut()
            && std::mem::take(&mut scripted.deferred)
            && !scripted.removed
        {
            scripted.removed = true;
            match scripted.action {
                ScriptedAction::Take => scripted.removals += 1,
                ScriptedAction::Return => scripted.returns += 1,
            }
        }
    }

    /// 远端真正取走附件的次数与收到的领取 POST 数。
    pub fn take_counts(&self) -> (usize, usize) {
        let state = self.state.lock().expect("test state poisoned");
        state.scripted.as_ref().map_or((0, 0), |scripted| (scripted.removals, scripted.take_posts))
    }

    /// 远端真正退回的次数与收到的 Return POST 数。
    pub fn return_counts(&self) -> (usize, usize) {
        let state = self.state.lock().expect("test state poisoned");
        state.scripted.as_ref().map_or((0, 0), |scripted| (scripted.returns, scripted.return_posts))
    }
}

/// 已登记的待领取邮件：读取按远端状态回放，迟到的领取在本次读取响应生成之后生效；领取 POST
/// 只取走一次附件。其他 MID 返回 `None`，交给语料回放。
pub(super) async fn respond(
    state: &Arc<Mutex<MailState>>,
    method: &Method,
    mid: Option<&str>,
) -> Option<Response> {
    let late = {
        let mut current = state.lock().expect("test state poisoned");
        let scripted =
            current.scripted.as_mut().filter(|scripted| Some(scripted.mid.as_str()) == mid)?;
        if *method == Method::GET {
            let page =
                if scripted.removed { scripted.taken.clone() } else { scripted.claimable.clone() };
            if std::mem::take(&mut scripted.deferred) && !scripted.removed {
                scripted.removed = true;
                match scripted.action {
                    ScriptedAction::Take => scripted.removals += 1,
                    ScriptedAction::Return => scripted.returns += 1,
                }
            }
            return Some(Html(page).into_response());
        }
        if *method != Method::POST {
            return Some(StatusCode::METHOD_NOT_ALLOWED.into_response());
        }
        match scripted.action {
            ScriptedAction::Take => scripted.take_posts += 1,
            ScriptedAction::Return => scripted.return_posts += 1,
        }
        match scripted.late_post.take() {
            Some(delay) => {
                scripted.deferred = !scripted.removed;
                Some((delay, scripted.taken.clone()))
            }
            None => {
                if !scripted.removed {
                    scripted.removed = true;
                    match scripted.action {
                        ScriptedAction::Take => scripted.removals += 1,
                        ScriptedAction::Return => scripted.returns += 1,
                    }
                }
                return Some(Html(scripted.taken.clone()).into_response());
            }
        }
    };
    let (delay, page) = late?;
    tokio::time::sleep(delay).await;
    Some(Html(page).into_response())
}
