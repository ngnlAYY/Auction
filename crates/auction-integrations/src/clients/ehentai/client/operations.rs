//! 主题发布、精确标题对账、回帖、编辑与读取；表单加载与提交细节在 topic.rs。

use auction_application::forum::{
    ForumPublishedPost, ForumReadPost, ForumRenderedPost, ForumRenderedThread, ForumTopicMetadata,
};
use auction_domain::SettlementWorld;

use crate::clients::metrics::{forum_operation, forum_world};

use url::Url;

use super::super::forms::{
    valid_cookie, valid_post_id, valid_rendered_post, valid_rendered_thread,
    valid_topic_description, valid_topic_id, valid_topic_title, valid_topic_titles,
};
use super::super::{EhentaiError, EhentaiOperation};
use super::{EhentaiClient, EhentaiCookie, ForumRequestScope};

impl EhentaiClient {
    /// 创建或按精确标题查找一个论坛主题。
    pub async fn create_topic(
        &self,
        world: SettlementWorld,
        cookie: &EhentaiCookie,
        rendered: ForumRenderedThread,
    ) -> Result<auction_application::forum::ForumTopicPublication, EhentaiError> {
        let operation = EhentaiOperation::CreateTopic;
        if !self.valid_for() || !valid_cookie(cookie) || !valid_rendered_thread(&rendered) {
            return Err(EhentaiError::configuration(operation));
        }
        let (board_id, board_url, existing) = self
            .find_topic_by_titles_with_operation(
                world,
                cookie,
                &rendered.title,
                &rendered.reconciliation_titles,
                operation,
            )
            .await?;
        if let Some(published) = existing {
            return Ok(auction_application::forum::ForumTopicPublication::Existing(published));
        }

        let form = self.load_topic_form(board_id, &board_url, forum_world(world), cookie).await?;
        self.submit_topic(form, board_id, rendered, forum_world(world), cookie)
            .await
            .map(auction_application::forum::ForumTopicPublication::Created)
    }

    /// 一次版面读取核对全部精确标题及唯一主题身份；永远不加载发帖表单或发送 POST。
    pub async fn find_topic_by_titles(
        &self,
        world: SettlementWorld,
        cookie: &EhentaiCookie,
        titles: &[String],
    ) -> Result<Option<auction_application::forum::ForumPublishedThread>, EhentaiError> {
        let (title, reconciliation_titles) = titles
            .split_first()
            .ok_or_else(|| EhentaiError::configuration(EhentaiOperation::ReconcileTopic))?;
        let (_, _, published) = self
            .find_topic_by_titles_with_operation(
                world,
                cookie,
                title,
                reconciliation_titles,
                EhentaiOperation::ReconcileTopic,
            )
            .await?;
        Ok(published)
    }

    async fn find_topic_by_titles_with_operation(
        &self,
        world: SettlementWorld,
        cookie: &EhentaiCookie,
        title: &str,
        reconciliation_titles: &[String],
        operation: EhentaiOperation,
    ) -> Result<(i32, Url, Option<auction_application::forum::ForumPublishedThread>), EhentaiError>
    {
        if !self.valid_for()
            || !valid_cookie(cookie)
            || !valid_topic_titles(title, reconciliation_titles)
        {
            return Err(EhentaiError::configuration(operation));
        }
        let board_id = self.board_for_world(world);
        let board_value = board_id.to_string();
        let board_url = self.index_url(&[("showforum", board_value.as_str())], operation)?;
        let (body, _) = self
            .get_page(
                operation,
                forum_operation(operation),
                ForumRequestScope::new(forum_world(world), cookie),
                board_url.clone(),
                None,
            )
            .await?;
        let published =
            self.find_topic(&board_url, board_id, title, reconciliation_titles, &body, operation)?;
        Ok((board_id, board_url, published))
    }

    /// 建立目录续页或系统日志回复；POST 后必须返回 exact thread/post proof。
    pub async fn reply_topic(
        &self,
        world: SettlementWorld,
        thread_id: &str,
        body: ForumRenderedPost,
        cookie: &EhentaiCookie,
    ) -> Result<ForumPublishedPost, EhentaiError> {
        let operation = EhentaiOperation::ReplyTopic;
        if !self.valid_for()
            || !valid_cookie(cookie)
            || !valid_topic_id(thread_id)
            || !valid_rendered_post(&body)
        {
            return Err(EhentaiError::configuration(operation));
        }
        let board_id = self.board_for_world(world);
        let form = self.load_reply_form(board_id, thread_id, forum_world(world), cookie).await?;
        self.submit_fixed_post(
            form,
            board_id,
            thread_id,
            None,
            body,
            ForumRequestScope::new(forum_world(world), cookie),
        )
        .await
    }

    /// 编辑已持久化的固定目录页或系统日志；目标 post ID 必须由调用方提供，不能从正文猜测。
    pub async fn edit_post(
        &self,
        world: SettlementWorld,
        thread_id: &str,
        post_id: &str,
        metadata: Option<&ForumTopicMetadata>,
        body: ForumRenderedPost,
        cookie: &EhentaiCookie,
    ) -> Result<ForumPublishedPost, EhentaiError> {
        let operation = EhentaiOperation::EditPost;
        if !self.valid_for()
            || !valid_cookie(cookie)
            || !valid_topic_id(thread_id)
            || !valid_post_id(post_id)
            || metadata.is_some_and(|metadata| {
                metadata.title.as_deref().is_some_and(|value| !valid_topic_title(value))
                    || metadata
                        .description
                        .as_deref()
                        .is_some_and(|value| !valid_topic_description(value))
            })
            || !valid_rendered_post(&body)
        {
            return Err(EhentaiError::configuration(operation));
        }
        let board_id = self.board_for_world(world);
        let form =
            self.load_edit_form(board_id, thread_id, post_id, forum_world(world), cookie).await?;
        if metadata
            .is_some_and(|metadata| metadata.title.is_some() || metadata.description.is_some())
            && form.metadata.is_none()
        {
            // 缺少首帖元数据字段时不能假定服务器会接受额外标题字段。
            return Err(EhentaiError::protocol(operation, false));
        }
        self.submit_fixed_post(
            form,
            board_id,
            thread_id,
            Some((
                post_id,
                metadata.and_then(|metadata| metadata.title.as_deref()),
                metadata.and_then(|metadata| metadata.description.as_deref()),
            )),
            body,
            ForumRequestScope::new(forum_world(world), cookie),
        )
        .await
    }

    /// 读取已校验编辑表单的原始 BBCode 与首帖元数据，不能以正文代替标题证明。
    pub async fn read_post(
        &self,
        world: SettlementWorld,
        thread_id: &str,
        post_id: &str,
        cookie: &EhentaiCookie,
    ) -> Result<ForumReadPost, EhentaiError> {
        let operation = EhentaiOperation::ReadPost;
        if !self.valid_for()
            || !valid_cookie(cookie)
            || !valid_topic_id(thread_id)
            || !valid_post_id(post_id)
        {
            return Err(EhentaiError::configuration(operation));
        }
        let board_id = self.board_for_world(world);
        let form =
            self.load_edit_form(board_id, thread_id, post_id, forum_world(world), cookie).await?;
        let body = ForumRenderedPost { body: form.body };
        if !valid_rendered_post(&body) {
            return Err(EhentaiError::protocol(operation, false));
        }
        Ok(ForumReadPost { body: body.body, metadata: form.metadata })
    }
}
