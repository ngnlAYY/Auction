//! Public 规则读模型的可选缓存装饰器。
//!
//! 规则文档由数据库权威持有。缓存只保存通过应用校验的完整快照，不缓存空结果；命中内容
//! 解析失败、版本格式不兼容或后端不可用时，始终回到原始 `PublicRulesReader`。

use std::{sync::Arc, time::Duration};

use crate::{
    ApplicationFuture, PUBLIC_RULES_LOCALES, PublicRulesDocument, PublicRulesReader,
    PublicRulesRequest, ReadModelCache,
};

const PUBLIC_RULES_CACHE_KEY_PREFIX: &str = "auction:read-model:public-rules:v1:";

/// 返回受支持语言对应的稳定缓存 key；未知语言不进入缓存命名空间。
pub fn public_rules_cache_key(locale: &str) -> Option<String> {
    PUBLIC_RULES_LOCALES
        .contains(&locale)
        .then(|| format!("{PUBLIC_RULES_CACHE_KEY_PREFIX}{locale}"))
}

/// 为 Public 规则读取端口增加可选缓存。
pub struct CachedPublicRulesReader {
    authority: Arc<dyn PublicRulesReader>,
    cache: Arc<dyn ReadModelCache>,
    ttl: Duration,
}

impl CachedPublicRulesReader {
    /// 创建规则读模型装饰器；TTL 由主配置校验后传入。
    pub fn new(
        authority: Arc<dyn PublicRulesReader>,
        cache: Arc<dyn ReadModelCache>,
        ttl: Duration,
    ) -> Self {
        Self { authority, cache, ttl }
    }
}

impl PublicRulesReader for CachedPublicRulesReader {
    fn get_public_rules(
        &self,
        request: PublicRulesRequest,
    ) -> ApplicationFuture<'_, Option<PublicRulesDocument>> {
        let authority = Arc::clone(&self.authority);
        let cache = Arc::clone(&self.cache);
        let ttl = self.ttl;
        Box::pin(async move {
            let Some(key) = public_rules_cache_key(&request.locale) else {
                return authority.get_public_rules(request).await;
            };

            let cached = cache.get(key.clone()).await;
            let cache_available = cached.is_ok();
            if let Ok(Some(value)) = cached {
                if let Ok(document) = serde_json::from_slice::<PublicRulesDocument>(&value)
                    && document.locale == request.locale
                    && document.validate().is_ok()
                {
                    return Ok(Some(document));
                }

                // 损坏或旧版本快照不能阻断回源；删除只是尽力而为。
                let _ = cache.invalidate(key.clone()).await;
            }

            let result = authority.get_public_rules(request).await?;
            if ttl.is_zero() {
                return Ok(result);
            }

            // 已知缓存不可用时不再尝试回填，避免权威读取成功后再次消耗同一故障的等待预算。
            if cache_available
                && let Some(document) = result.as_ref()
                && document.validate().is_ok()
                && let Ok(value) = serde_json::to_vec(document)
            {
                // 写入失败只损失命中率，不能把已经成功的权威读取改成失败。
                let _ = cache.set(key, value, ttl).await;
            }
            Ok(result)
        })
    }
}
