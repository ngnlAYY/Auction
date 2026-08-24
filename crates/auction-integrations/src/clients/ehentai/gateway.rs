//! E-Hentai 凭据轮换网关与 Forum 端口适配。

use super::*;

/// 负责按读侧 session 过期结果轮换论坛凭据的网关。
#[derive(Clone)]
pub struct EhentaiGateway {
    client: Arc<EhentaiClient>,
    credentials: EhentaiCredentialSet,
}

impl EhentaiGateway {
    /// 建立不读取凭据、不发起网络请求的网关。
    pub fn new(
        client: Arc<EhentaiClient>,
        credentials: EhentaiCredentialSet,
    ) -> Result<Self, EhentaiError> {
        if credentials.is_empty() {
            return Err(EhentaiError::credential(EhentaiOperation::CreateTopic));
        }
        Ok(Self { client, credentials })
    }

    /// 创建主题；只有读侧 session 过期才会尝试下一个 Cookie。
    pub async fn create_topic(
        &self,
        world: SettlementWorld,
        rendered: ForumRenderedThread,
    ) -> Result<auction_application::ForumPublishedThread, EhentaiError> {
        let mut session = self.credentials.begin()?;
        for _ in 0..MAXIMUM_CREDENTIAL_CANDIDATES {
            match self.client.create_topic(world, session.cookie(), rendered.clone()).await {
                Ok(published) => return Ok(published),
                Err(error) if error.is_session_expired() => {
                    if session.invalidate_current().await.is_err() || session.advance().is_err() {
                        return Err(EhentaiError::credential(EhentaiOperation::CreateTopic));
                    }
                }
                Err(error) if error.category() == EhentaiErrorCategory::SessionExpired => {
                    let _ = session.invalidate_current().await;
                    return Err(error);
                }
                Err(error) => return Err(error),
            }
        }
        Err(EhentaiError::credential(EhentaiOperation::CreateTopic))
    }

    /// 读取主题；读取失败且确认为 session 过期时才会尝试下一个 Cookie。
    pub async fn fetch_topic(
        &self,
        world: SettlementWorld,
        thread_id: &str,
    ) -> Result<AdminForumPreviewResult, EhentaiError> {
        let mut session = self.credentials.begin()?;
        for _ in 0..MAXIMUM_CREDENTIAL_CANDIDATES {
            match self.client.fetch_topic(world, thread_id, session.cookie()).await {
                Ok(result) => return Ok(result),
                Err(error) if error.is_session_expired() => {
                    if session.invalidate_current().await.is_err() || session.advance().is_err() {
                        return Err(EhentaiError::credential(EhentaiOperation::FetchTopic));
                    }
                }
                Err(error) if error.category() == EhentaiErrorCategory::SessionExpired => {
                    let _ = session.invalidate_current().await;
                    return Err(error);
                }
                Err(error) => return Err(error),
            }
        }
        Err(EhentaiError::credential(EhentaiOperation::FetchTopic))
    }
}

impl fmt::Debug for EhentaiGateway {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EhentaiGateway")
            .field("client", &self.client)
            .field("credentials", &self.credentials)
            .finish()
    }
}

impl ForumPublisher for EhentaiGateway {
    fn create_topic(
        &self,
        world: SettlementWorld,
        rendered: ForumRenderedThread,
    ) -> ApplicationFuture<'_, ForumPublishedThread> {
        Box::pin(async move {
            EhentaiGateway::create_topic(self, world, rendered)
                .await
                .map_err(EhentaiError::into_application_error)
        })
    }
}

impl ForumTopicReader for EhentaiGateway {
    fn fetch_topic(
        &self,
        world: SettlementWorld,
        thread_id: String,
    ) -> ApplicationFuture<'_, AdminForumPreviewResult> {
        Box::pin(async move {
            EhentaiGateway::fetch_topic(self, world, &thread_id)
                .await
                .map_err(EhentaiError::into_application_error)
        })
    }
}
