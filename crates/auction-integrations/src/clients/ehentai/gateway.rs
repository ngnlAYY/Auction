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

    /// 只读按精确标题查找主题；读侧 session 过期时可安全轮换 Cookie。
    pub async fn find_topic_by_title(
        &self,
        world: SettlementWorld,
        title: &str,
    ) -> Result<Option<auction_application::ForumPublishedThread>, EhentaiError> {
        let mut session = self.credentials.begin()?;
        for _ in 0..MAXIMUM_CREDENTIAL_CANDIDATES {
            match self.client.find_topic_by_title(world, session.cookie(), title).await {
                Ok(published) => return Ok(published),
                Err(error) if error.is_session_expired() => {
                    if session.invalidate_current().await.is_err() || session.advance().is_err() {
                        return Err(EhentaiError::credential(EhentaiOperation::ReconcileTopic));
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Err(EhentaiError::credential(EhentaiOperation::ReconcileTopic))
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

    /// 建立固定 #2；仅在 POST 尚未开始且明确 session 过期时轮换凭据。
    pub async fn reply_topic(
        &self,
        world: SettlementWorld,
        thread_id: &str,
        body: ForumRenderedPost,
    ) -> Result<ForumPublishedPost, EhentaiError> {
        let mut session = self.credentials.begin()?;
        for _ in 0..MAXIMUM_CREDENTIAL_CANDIDATES {
            match self.client.reply_topic(world, thread_id, body.clone(), session.cookie()).await {
                Ok(published) => return Ok(published),
                Err(error) if error.is_session_expired() => {
                    if session.invalidate_current().await.is_err() || session.advance().is_err() {
                        return Err(EhentaiError::credential(EhentaiOperation::ReplyTopic));
                    }
                }
                Err(error) if error.category() == EhentaiErrorCategory::SessionExpired => {
                    let _ = session.invalidate_current().await;
                    return Err(error);
                }
                Err(error) => return Err(error),
            }
        }
        Err(EhentaiError::credential(EhentaiOperation::ReplyTopic))
    }

    /// 编辑固定 #1/#2；POST 开始后绝不切换 Cookie 重发。
    pub async fn edit_post(
        &self,
        world: SettlementWorld,
        thread_id: &str,
        post_id: &str,
        title: Option<&str>,
        body: ForumRenderedPost,
    ) -> Result<ForumPublishedPost, EhentaiError> {
        let mut session = self.credentials.begin()?;
        for _ in 0..MAXIMUM_CREDENTIAL_CANDIDATES {
            match self
                .client
                .edit_post(world, thread_id, post_id, title, body.clone(), session.cookie())
                .await
            {
                Ok(published) => return Ok(published),
                Err(error) if error.is_session_expired() => {
                    if session.invalidate_current().await.is_err() || session.advance().is_err() {
                        return Err(EhentaiError::credential(EhentaiOperation::EditPost));
                    }
                }
                Err(error) if error.category() == EhentaiErrorCategory::SessionExpired => {
                    let _ = session.invalidate_current().await;
                    return Err(error);
                }
                Err(error) => return Err(error),
            }
        }
        Err(EhentaiError::credential(EhentaiOperation::EditPost))
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
    fn find_topic_by_title(
        &self,
        world: SettlementWorld,
        title: String,
    ) -> ApplicationFuture<'_, Option<ForumPublishedThread>> {
        Box::pin(async move {
            EhentaiGateway::find_topic_by_title(self, world, &title)
                .await
                .map_err(EhentaiError::into_application_error)
        })
    }

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

impl ForumPostWriter for EhentaiGateway {
    fn reply_topic(
        &self,
        world: SettlementWorld,
        thread_id: String,
        body: ForumRenderedPost,
    ) -> ApplicationFuture<'_, ForumPublishedPost> {
        Box::pin(async move {
            EhentaiGateway::reply_topic(self, world, &thread_id, body)
                .await
                .map_err(EhentaiError::into_application_error)
        })
    }

    fn edit_post(
        &self,
        world: SettlementWorld,
        thread_id: String,
        post_id: String,
        title: Option<String>,
        body: ForumRenderedPost,
    ) -> ApplicationFuture<'_, ForumPublishedPost> {
        Box::pin(async move {
            EhentaiGateway::edit_post(self, world, &thread_id, &post_id, title.as_deref(), body)
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
