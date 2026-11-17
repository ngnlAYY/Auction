//! 联合证据内的 compose 干净证明：同一次证据调用中每个世界只读取一次。
//!
//! 独占 guard 期间本进程不写 compose，证据流程也只有只读 GET；某世界一旦证明干净，
//! 该世界随后的库存、余额与 Sent 观察沿用这次证明，仍保持“compose 证明先于该世界观察”
//! 的顺序。记录随单次证据调用建立，不跨 guard 复用，也不复用投递 preflight 的观察。

use auction_application::settlement::InternalEvidenceError as Error;

use super::diagnostics::{EvidenceStage as Stage, at};
use super::map_read_error;
use crate::clients::mooglemail::gateway::credentials::MoogleMailCredentialSession;
use crate::clients::mooglemail::gateway::reader::MoogleMailGateway;
use crate::clients::mooglemail::protocol::{MoogleMailOperation, MoogleMailWorld};

/// 本次证据调用中已证明 compose 干净的世界。
#[derive(Default)]
pub(in crate::clients::mooglemail::gateway) struct CleanComposeWorlds {
    main: bool,
    isekai: bool,
}

impl CleanComposeWorlds {
    fn proven(&mut self, world: MoogleMailWorld) -> &mut bool {
        match world {
            MoogleMailWorld::Main => &mut self.main,
            MoogleMailWorld::Isekai => &mut self.isekai,
        }
    }
}

impl MoogleMailGateway {
    /// 证明 world 的 compose 干净。运行时绑定每次都复核；同一记录中已证明的世界不再发 GET。
    pub(in crate::clients::mooglemail::gateway) async fn require_clean_compose(
        &self,
        session: &MoogleMailCredentialSession,
        world: MoogleMailWorld,
        clean: &mut CleanComposeWorlds,
    ) -> Result<(), Error> {
        session
            .internal_credential_binding(world)
            .map_err(|error| at(Stage::RuntimeBinding, world, error))?;
        if *clean.proven(world) {
            return Ok(());
        }
        let target = self
            .client
            .compose(world)
            .map_err(|error| at(Stage::ComposeGet, world, map_read_error(error)))?;
        let document = self
            .read_session_document(session, &target)
            .await
            .map_err(|error| at(Stage::ComposeGet, world, error))?;
        let document = self
            .parser
            .evidence_document(&document, world, MoogleMailOperation::Compose)
            .map_err(|error| at(Stage::ComposeWorld, world, map_read_error(error)))?;
        if !self
            .parser
            .parse_compose_document(&document, world)
            .map_err(|error| at(Stage::ComposeParse, world, map_read_error(error)))?
            .is_clean()
        {
            return Err(at(Stage::ComposeDirty, world, Error::Unproven));
        }
        *clean.proven(world) = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use auction_application::admin::{
        AdminCredentialRuntimeOwnership, AdminCredentialRuntimeWriter,
    };
    use auction_application::core::ApplicationFuture;
    use auction_application::settlement::InternalEvidenceError;
    use auction_test_support::mooglemail::MailDouble;
    use auction_time::Timestamp;
    use uuid::Uuid;

    use super::CleanComposeWorlds;
    use crate::clients::mooglemail::{
        MoogleMailClient, MoogleMailClientConfig, MoogleMailCookie, MoogleMailCredentialSet,
        MoogleMailGateway, MoogleMailGatewayConfig, MoogleMailOperation,
        MoogleMailRuntimeCredential, MoogleMailWorld,
    };
    use crate::clients::owner_probe::fixture::GatedOwnership;

    struct NoInvalidation;

    impl AdminCredentialRuntimeWriter for NoInvalidation {
        fn mark_invalid(&self, _credential_id: Uuid, _now: Timestamp) -> ApplicationFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
    }

    fn gateway(server: &MailDouble) -> MoogleMailGateway {
        let client = MoogleMailClient::with_config(MoogleMailClientConfig {
            endpoint: server.endpoint(),
            maximum_response_bytes: 8 * 1024 * 1024,
            minimum_request_interval: Duration::from_millis(1),
            request_timeout: Duration::from_secs(5),
        })
        .expect("loopback client");
        let owner: Arc<dyn AdminCredentialRuntimeOwnership> = Arc::new(GatedOwnership::open());
        let credentials = MoogleMailCredentialSet::from_runtime_credentials(
            vec![MoogleMailRuntimeCredential {
                credential_id: Uuid::new_v4(),
                state_version: 1,
                world: None,
                player_name: None,
                cookie: MoogleMailCookie::new("ipb_member_id=fixture; ipb_pass_hash=fixture")
                    .expect("fixture cookie"),
            }],
            Arc::new(NoInvalidation),
            Some(owner),
        )
        .expect("runtime credential set");
        MoogleMailGateway::new(Arc::new(client), credentials, MoogleMailGatewayConfig::default())
            .expect("gateway")
    }

    fn compose_reads(server: &MailDouble) -> usize {
        server.requests().iter().filter(|request| request.contains("filter=new")).count()
    }

    #[tokio::test]
    async fn compose_is_read_once_per_world_within_one_evidence_call() {
        let server = MailDouble::start().await.expect("loopback server");
        let gateway = gateway(&server);
        let session = gateway
            .credentials
            .begin_exclusive(MoogleMailWorld::Main, MoogleMailOperation::Identity)
            .await
            .expect("exclusive session");
        let mut clean = CleanComposeWorlds::default();

        // 派发世界与 Main 现金/实收观察同为 Main：同一证据调用只读一次。
        for _ in 0..2 {
            gateway
                .require_clean_compose(&session, MoogleMailWorld::Main, &mut clean)
                .await
                .expect("main compose clean");
        }
        assert_eq!(compose_reads(&server), 1);

        // 装备或现金在另一世界时仍各自证明一次。
        for _ in 0..2 {
            gateway
                .require_clean_compose(&session, MoogleMailWorld::Isekai, &mut clean)
                .await
                .expect("isekai compose clean");
        }
        assert_eq!(compose_reads(&server), 2);

        // 新的证据调用不沿用上一次的证明。
        gateway
            .require_clean_compose(
                &session,
                MoogleMailWorld::Main,
                &mut CleanComposeWorlds::default(),
            )
            .await
            .expect("fresh proof");
        assert_eq!(compose_reads(&server), 3);

        // 沿用证明时仍复核运行时绑定：owner 撤销后失败关闭，且不发 GET。
        session.revoke_internal_owner();
        let error = gateway
            .require_clean_compose(&session, MoogleMailWorld::Main, &mut clean)
            .await
            .expect_err("撤销后不得沿用证明");
        assert!(matches!(error, InternalEvidenceError::Revoked));
        assert_eq!(compose_reads(&server), 3);
    }
}
