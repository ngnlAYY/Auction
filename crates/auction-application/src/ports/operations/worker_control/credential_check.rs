//! Admin 邮件凭据在线检查经 Worker control 代为执行的契约。
//!
//! Admin 只提交凭据 ID 与期望状态版本；Worker 按 ID 读取并解密该凭据的数据库信封，前后复核
//! 版本，用与本进程邮件循环共用请求门、在途锁和账号忙熔断的客户端执行固定只读探测。Worker
//! 不可用时 Admin 得到“校验暂不可用”，不得回退为 Admin 进程直连外部站点。

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ports::admin::{
    AdminCredentialCheckObservation, AdminCredentialCheckStatus, AdminCredentialError,
    AdminCredentialWorld,
};
use crate::ports::core::ApplicationFuture;

/// 一次转发检查的总预算（秒），control server 中间件与 client 共用。须小于 Admin HTTP 默认
/// 15 秒的请求预算；Worker 内单个世界的探测另有更短的固定预算。
pub const WORKER_MAIL_CREDENTIAL_CHECK_TIMEOUT_SECONDS: u64 = 12;

/// Admin 发往 Worker 的检查请求；只描述凭据身份与期望版本，不含 Cookie 或目标 URL。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerMailCredentialCheckRequest {
    pub credential_id: Uuid,
    pub expected_state_version: i64,
}

impl WorkerMailCredentialCheckRequest {
    /// 凭据 ID 非空且期望版本为正。
    pub fn validate(&self) -> bool {
        !self.credential_id.is_nil() && self.expected_state_version > 0
    }
}

/// 单个世界的观测投影；只含稳定的世界码与状态码，不含账号、余额、Cookie 或页面正文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerMailCredentialCheckEntry {
    pub world: String,
    pub status: String,
}

/// Worker 返回的检查结果；回显凭据 ID 与状态版本，供 Admin 确认结果绑定本次请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerMailCredentialCheckResponse {
    pub credential_id: Uuid,
    pub state_version: i64,
    pub checks: Vec<WorkerMailCredentialCheckEntry>,
}

impl WorkerMailCredentialCheckResponse {
    /// 由 Worker 侧观测建立传输投影。
    pub fn new(
        credential_id: Uuid,
        state_version: i64,
        checks: &[AdminCredentialCheckObservation],
    ) -> Self {
        Self {
            credential_id,
            state_version,
            checks: checks
                .iter()
                .map(|check| WorkerMailCredentialCheckEntry {
                    world: check.world.as_str().to_owned(),
                    status: check.status.as_str().to_owned(),
                })
                .collect(),
        }
    }

    /// 还原为应用观测。结果未绑定本次请求、为空、超过两个世界或含未知取值时返回 `None`，
    /// 调用方按协议不兼容处理，不猜测部分结果。
    pub fn into_observations(
        self,
        request: &WorkerMailCredentialCheckRequest,
    ) -> Option<Vec<AdminCredentialCheckObservation>> {
        if self.credential_id != request.credential_id
            || self.state_version != request.expected_state_version
            || self.checks.is_empty()
            || self.checks.len() > 2
        {
            return None;
        }
        let observations = self
            .checks
            .into_iter()
            .map(|check| {
                Some(AdminCredentialCheckObservation {
                    world: AdminCredentialWorld::parse(&check.world)?,
                    status: AdminCredentialCheckStatus::parse(&check.status)?,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        if observations.len() == 2 && observations[0].world == observations[1].world {
            return None;
        }
        Some(observations)
    }
}

/// 版本冲突时 Worker 回显的当前状态版本。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerMailCredentialCheckConflict {
    pub current_version: i64,
}

/// Admin 侧的邮件凭据检查转发端口；实现只能把检查交给 Worker 执行。
///
/// 错误沿用 [`AdminCredentialError`]：版本冲突、凭据不存在、请求无效与并发检查繁忙按原类别
/// 返回；Worker 未启用、不可达、超时或协议不兼容一律为 `CheckUnavailable`。
pub trait WorkerMailCredentialCheckPort: Send + Sync {
    fn check_mail_credential(
        &self,
        request: WorkerMailCredentialCheckRequest,
    ) -> ApplicationFuture<'_, Vec<AdminCredentialCheckObservation>>;
}

/// 无法连接 Worker control 时 Admin 装配的立即失败端口；不回退到本地直连。
pub struct UnavailableWorkerMailCredentialCheck;

impl WorkerMailCredentialCheckPort for UnavailableWorkerMailCredentialCheck {
    fn check_mail_credential(
        &self,
        _request: WorkerMailCredentialCheckRequest,
    ) -> ApplicationFuture<'_, Vec<AdminCredentialCheckObservation>> {
        Box::pin(async { Err(AdminCredentialError::CheckUnavailable.into()) })
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{
        WorkerMailCredentialCheckEntry, WorkerMailCredentialCheckRequest,
        WorkerMailCredentialCheckResponse,
    };
    use crate::ports::admin::{
        AdminCredentialCheckObservation, AdminCredentialCheckStatus, AdminCredentialWorld,
    };

    fn request() -> WorkerMailCredentialCheckRequest {
        WorkerMailCredentialCheckRequest {
            credential_id: Uuid::from_u128(7),
            expected_state_version: 3,
        }
    }

    #[test]
    fn request_requires_identity_and_positive_version() {
        assert!(request().validate());
        assert!(
            !WorkerMailCredentialCheckRequest { credential_id: Uuid::nil(), ..request() }
                .validate()
        );
        assert!(
            !WorkerMailCredentialCheckRequest { expected_state_version: 0, ..request() }.validate()
        );
    }

    #[test]
    fn response_round_trips_observations_bound_to_the_request() {
        let observations = vec![
            AdminCredentialCheckObservation {
                world: AdminCredentialWorld::Main,
                status: AdminCredentialCheckStatus::Valid,
            },
            AdminCredentialCheckObservation {
                world: AdminCredentialWorld::Isekai,
                status: AdminCredentialCheckStatus::Inconclusive,
            },
        ];
        let response = WorkerMailCredentialCheckResponse::new(
            request().credential_id,
            request().expected_state_version,
            &observations,
        );
        let encoded = serde_json::to_value(&response).expect("encode response");
        let decoded: WorkerMailCredentialCheckResponse =
            serde_json::from_value(encoded).expect("decode response");
        assert_eq!(decoded.into_observations(&request()), Some(observations));
    }

    #[test]
    fn response_not_bound_to_the_request_or_with_unknown_values_is_rejected() {
        let entry = |world: &str, status: &str| WorkerMailCredentialCheckEntry {
            world: world.to_owned(),
            status: status.to_owned(),
        };
        let bound =
            |checks: Vec<WorkerMailCredentialCheckEntry>| WorkerMailCredentialCheckResponse {
                credential_id: request().credential_id,
                state_version: request().expected_state_version,
                checks,
            };
        for response in [
            WorkerMailCredentialCheckResponse {
                credential_id: Uuid::from_u128(8),
                ..bound(vec![entry("main", "valid")])
            },
            WorkerMailCredentialCheckResponse {
                state_version: 4,
                ..bound(vec![entry("main", "valid")])
            },
            bound(Vec::new()),
            bound(vec![entry("main", "valid"), entry("isekai", "valid"), entry("main", "valid")]),
            bound(vec![entry("moon", "valid")]),
            bound(vec![entry("main", "ok")]),
        ] {
            assert_eq!(response.clone().into_observations(&request()), None, "{response:?}");
        }
    }

    #[test]
    fn response_rejects_unknown_fields() {
        let value = serde_json::json!({
            "credential_id": Uuid::from_u128(7),
            "state_version": 3,
            "checks": [],
            "cookie": "secret",
        });
        assert!(serde_json::from_value::<WorkerMailCredentialCheckResponse>(value).is_err());
    }
}
