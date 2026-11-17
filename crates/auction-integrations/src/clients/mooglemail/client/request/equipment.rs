//! 装备页读取借用的同账号请求许可：与本客户端的邮件请求共用在途锁、账号忙熔断与外部账号
//! 请求指标（`operation=equipment`），但不经请求门；装备页之间的节奏仍由装备来源适配器自身的
//! 最小间隔决定。保护的不变量：同一 Cookie 的物理请求不重叠；只有来源明确无效才能退件，
//! 战斗页与限流只能让读取暂时不可用。许可不借用客户端，装备页请求发出后被放弃时可随剩余部分
//! 交给独立任务，在途锁不随调用方提前释放。

use std::sync::Arc;
use std::time::Instant;

use auction_application::external_metrics::{
    ExternalMetricsObserver, MailRequestCategory, MailSignal,
};
use reqwest::StatusCode;

use super::super::MoogleMailClient;
use super::super::account_busy::MoogleMailAccountBusy;
use super::super::security::{is_hentaiverse_battle_response, is_state_lock_limited};
use crate::clients::gate::GateClass;
use crate::clients::metrics::{MailRequestGuard, mail_signal};
use crate::clients::mooglemail::{MoogleMailOperation, MoogleMailWorld};
use crate::clients::operation_lock::OperationLockGuard;

/// 装备页响应反映的账号临时状态；都不表示来源无效，调用方只能按暂时不可用延期，不能退件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EquipmentReadHold {
    /// 战斗页替代了装备页：角色正被玩家占用。
    AccountBusy,
    /// 远端限流：429 或 HV 状态锁限流正文（同一账号请求重叠）。
    RateLimited,
}

/// 与邮件读取同一口径识别装备页响应中的账号临时状态；只看状态码与正文，不改变熔断状态。
pub(crate) fn equipment_read_hold(status: StatusCode, body: &[u8]) -> Option<EquipmentReadHold> {
    if is_hentaiverse_battle_response(body) {
        Some(EquipmentReadHold::AccountBusy)
    } else if status == StatusCode::TOO_MANY_REQUESTS || is_state_lock_limited(body) {
        Some(EquipmentReadHold::RateLimited)
    } else {
        None
    }
}

/// 持有期间占用同账号在途锁；Drop 释放锁，已开始而未收口的请求指标按 canceled 记录。
pub(crate) struct EquipmentReadPermit {
    account_busy: Arc<MoogleMailAccountBusy>,
    external_metrics: Arc<dyn ExternalMetricsObserver>,
    world: MoogleMailWorld,
    request: Option<MailRequestGuard>,
    hold: Option<MailRequestCategory>,
    _in_flight: OperationLockGuard,
}

impl MoogleMailClient {
    /// 取得装备页读取的同账号许可；返回 `None` 表示账号忙，调用方不得发出网络请求。
    ///
    /// 装备页是纯只读观测，按后台档排在已登记的前台请求之后。熔断窗口内一律拒绝且不占用
    /// 窗口唯一的探测额度：装备页是否在战斗中被替换尚未证实，它的成功不能证明账号已退出战斗，
    /// 因此成功读取也不结束忙碌期。
    pub(crate) async fn equipment_read_permit(
        &self,
        world: MoogleMailWorld,
    ) -> Option<EquipmentReadPermit> {
        // 已知的熔断拒绝先于排队，不在锁上空等。
        if !self.account_busy.permits_unprobed_read(world, Instant::now()) {
            return None;
        }
        let in_flight = self.in_flight.lock(GateClass::Background).await;
        // 排队期间在途请求可能已打开账号忙窗口：发出网络前复核。
        if !self.account_busy.permits_unprobed_read(world, Instant::now()) {
            return None;
        }
        Some(EquipmentReadPermit {
            account_busy: Arc::clone(&self.account_busy),
            external_metrics: Arc::clone(&self.external_metrics),
            world,
            request: None,
            hold: None,
            _in_flight: in_flight,
        })
    }
}

impl EquipmentReadPermit {
    /// 网络请求即将发出：开始计入外部账号请求指标。
    pub(crate) fn start_request(&mut self) {
        self.request = Some(MailRequestGuard::start(
            Arc::clone(&self.external_metrics),
            self.world,
            MoogleMailOperation::Equipment,
        ));
    }

    /// 记录响应反映的账号临时状态。战斗页立即打开同一账号忙窗口：此时仍持有在途锁，排队请求
    /// 取得锁后的复核一定能看到新窗口；熔断只限制请求节奏，凭据仍有效。
    pub(crate) fn record_hold(&mut self, hold: EquipmentReadHold) {
        let (signal, category) = match hold {
            EquipmentReadHold::AccountBusy => {
                self.account_busy.record_busy(self.world, Instant::now());
                (MailSignal::AccountBusy, MailRequestCategory::AccountBusy)
            }
            EquipmentReadHold::RateLimited => {
                (MailSignal::RateLimited, MailRequestCategory::RateLimited)
            }
        };
        mail_signal(
            self.external_metrics.as_ref(),
            self.world,
            MoogleMailOperation::Equipment,
            signal,
        );
        self.hold = Some(category);
    }

    /// 收口请求指标并释放在途锁；记录过临时状态的请求以该状态收口。
    pub(crate) fn finish(mut self, category: MailRequestCategory) {
        if let Some(request) = self.request.take() {
            request.finish(self.hold.unwrap_or(category));
        }
    }
}

#[cfg(test)]
mod tests {
    use reqwest::StatusCode;

    use super::{EquipmentReadHold, equipment_read_hold};

    #[test]
    fn battle_and_rate_limits_are_holds_not_source_failures() {
        let battle = br#"<html><body><div id="battle_main">loading</div></body></html>"#;
        assert_eq!(
            equipment_read_hold(StatusCode::OK, battle),
            Some(EquipmentReadHold::AccountBusy)
        );
        assert_eq!(
            equipment_read_hold(StatusCode::OK, b"state lock limiter in effect"),
            Some(EquipmentReadHold::RateLimited)
        );
        assert_eq!(
            equipment_read_hold(StatusCode::TOO_MANY_REQUESTS, b""),
            Some(EquipmentReadHold::RateLimited)
        );
        // 正常装备页里出现同名文字不是账号状态。
        assert_eq!(
            equipment_read_hold(
                StatusCode::OK,
                br#"<html><body><div class="showequip"><p>battle_main</p></div></body></html>"#
            ),
            None
        );
        assert_eq!(equipment_read_hold(StatusCode::INTERNAL_SERVER_ERROR, b"<html></html>"), None);
    }
}
