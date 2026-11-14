//! Admin 装备 URL 来源改为经 Worker control 代为读取。
//! Admin 进程不再发起 HentaiVerse 装备页请求，也不持有第二套限速器；URL 分支只把世界与
//! 公开装备链接交给 Worker，canonical JSON 分支仍在本进程完成纯解析。

use std::sync::Arc;

use auction_application::admin::{
    AdminIntakeError, AdminIntakeSourceReader, AdminIntakeSourceSnapshot, CanonicalEquipment,
};
use auction_application::auction::ApplicationFuture;
use auction_application::core::ApplicationError;
use auction_application::operations::{WorkerEquipmentReadRequest, WorkerRemoteReadPort};

use super::reader::canonical_json_snapshot;

/// 以 Worker control 作为装备 URL 来源的读取器。
pub struct WorkerEquipmentSourceReader {
    remote: Arc<dyn WorkerRemoteReadPort>,
}

impl WorkerEquipmentSourceReader {
    /// 注入 Worker control 远端读取端口；端口不可用时读取明确失败，不回退到本地直连。
    pub fn new(remote: Arc<dyn WorkerRemoteReadPort>) -> Self {
        Self { remote }
    }
}

impl AdminIntakeSourceReader for WorkerEquipmentSourceReader {
    fn read_url(
        &self,
        world_id: String,
        equipment_url: String,
    ) -> ApplicationFuture<'_, AdminIntakeSourceSnapshot> {
        Box::pin(async move {
            let request = WorkerEquipmentReadRequest { world_id, equipment_url };
            if !request.validate() {
                return Err(ApplicationError::from(AdminIntakeError::InvalidCommand));
            }
            let snapshot = self
                .remote
                .read_equipment(request)
                .await
                .map_err(AdminIntakeError::from)
                .and_then(|response| response.into_snapshot().map_err(AdminIntakeError::from))
                .map_err(ApplicationError::from)?;
            Ok(snapshot)
        })
    }

    fn read_json(
        &self,
        world_id: String,
        equipment: CanonicalEquipment,
    ) -> ApplicationFuture<'_, AdminIntakeSourceSnapshot> {
        Box::pin(async move {
            canonical_json_snapshot(&world_id, equipment).map_err(ApplicationError::from)
        })
    }
}

#[cfg(test)]
mod tests {
    use auction_application::operations::{
        WorkerEquipmentReadRequest, WorkerEquipmentReadResponse, WorkerRemoteReadError,
        WorkerRemoteReadFuture, WorkerSentLookupRequest, WorkerSentLookupResponse,
    };
    use chrono::Utc;
    use serde_json::json;

    use super::*;

    struct Stub {
        error: Option<WorkerRemoteReadError>,
    }

    impl Stub {
        fn failing(error: WorkerRemoteReadError) -> Arc<dyn WorkerRemoteReadPort> {
            Arc::new(Self { error: Some(error) })
        }
    }

    impl WorkerRemoteReadPort for Stub {
        fn read_equipment(
            &self,
            _request: WorkerEquipmentReadRequest,
        ) -> WorkerRemoteReadFuture<'_, WorkerEquipmentReadResponse> {
            let error = self.error;
            Box::pin(async move {
                match error {
                    Some(error) => Err(error),
                    None => Ok(WorkerEquipmentReadResponse {
                        adapter: "hentaiverse-equipment-v1".to_owned(),
                        source_fingerprint: vec![3; 32],
                        external_equipment_id: "300753957".to_owned(),
                        name: "Legendary Phase Shoes".to_owned(),
                        category: "Shoes".to_owned(),
                        level: Some(500),
                        tradeable: true,
                        attributes: json!({"Primary Attributes / Strength": {"value": "+1.00"}}),
                        collected_at: Utc::now(),
                        key: "0123456789".to_owned(),
                    }),
                }
            })
        }

        fn lookup_sent(
            &self,
            _request: WorkerSentLookupRequest,
        ) -> WorkerRemoteReadFuture<'_, WorkerSentLookupResponse> {
            Box::pin(async { Err(WorkerRemoteReadError::Unavailable) })
        }
    }

    fn url() -> String {
        "https://hentaiverse.org/equip/300753957/0123456789".to_owned()
    }

    #[tokio::test]
    async fn url_reads_are_served_by_the_worker_projection() {
        let success = WorkerEquipmentSourceReader::new(Arc::new(Stub { error: None }));
        let snapshot = success
            .read_url("main".to_owned(), url())
            .await
            .expect("worker projection is accepted");
        assert_eq!(snapshot.external_equipment_id, "300753957");
        assert_eq!(snapshot.source_fingerprint, [3; 32]);
    }

    #[tokio::test]
    async fn pause_and_unavailable_map_to_admin_intake_errors() {
        let paused = WorkerEquipmentSourceReader::new(Stub::failing(WorkerRemoteReadError::Paused));
        let error = paused.read_url("main".to_owned(), url()).await.expect_err("paused");
        assert!(matches!(
            error,
            ApplicationError::AdminIntake(AdminIntakeError::ExternalAccessPaused)
        ));

        let unavailable =
            WorkerEquipmentSourceReader::new(Stub::failing(WorkerRemoteReadError::Unavailable));
        let error = unavailable.read_url("main".to_owned(), url()).await.expect_err("unavailable");
        assert!(matches!(
            error,
            ApplicationError::AdminIntake(AdminIntakeError::SourceUnavailable)
        ));
    }

    #[tokio::test]
    async fn invalid_targets_are_rejected_without_calling_the_worker() {
        let reader = WorkerEquipmentSourceReader::new(Stub::failing(WorkerRemoteReadError::Paused));
        let error = reader
            .read_url("shared".to_owned(), url())
            .await
            .expect_err("invalid world must fail before the worker call");
        assert!(matches!(error, ApplicationError::AdminIntake(AdminIntakeError::InvalidCommand)));
    }
}
