//! 可选缓存传输适配器。
//!
//! 缓存不是拍卖事实来源。应用层只能把本模块用于可回源的读取投影；竞价事务、幂等账本、
//! 审计、outbox 和 Worker 游标必须继续由数据库持久化。

use std::{fmt, time::Duration};

use auction_application::{ReadModelCache, ReadModelCacheError, ReadModelCacheFuture};
use thiserror::Error;

#[cfg(feature = "redis")]
use redis::aio::ConnectionManager;

/// 当前进程实际装配的缓存后端。
pub enum CacheStore {
    /// 不使用缓存，也不建立任何外部连接。
    Disabled,
    /// 使用 Redis 协议缓存；连接在第一次命令执行时按需建立。
    #[cfg(feature = "redis")]
    Redis(RedisCache),
}

impl CacheStore {
    /// 创建完全关闭的缓存实现。
    pub const fn disabled() -> Self {
        Self::Disabled
    }

    /// 创建 Redis 缓存。
    ///
    /// 未编译 `redis` feature 时返回明确错误，避免配置要求 Redis 却静默降级。
    pub fn redis(redis_url: &str) -> Result<Self, CacheError> {
        #[cfg(feature = "redis")]
        {
            RedisCache::new(redis_url).map(Self::Redis)
        }

        #[cfg(not(feature = "redis"))]
        {
            let _ = redis_url;
            Err(CacheError::RedisFeatureDisabled)
        }
    }

    /// 返回稳定的后端名称，供健康状态和结构化日志使用。
    pub const fn backend(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            #[cfg(feature = "redis")]
            Self::Redis(_) => "redis",
        }
    }

    /// 获取原始缓存值。调用方必须在错误或未命中时回源数据库。
    pub async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, CacheError> {
        match self {
            Self::Disabled => {
                let _ = key;
                Ok(None)
            }
            #[cfg(feature = "redis")]
            Self::Redis(cache) => cache.get(key).await,
        }
    }

    /// 写入带过期时间的原始缓存值。
    pub async fn set(&self, key: &str, value: &[u8], ttl: Duration) -> Result<(), CacheError> {
        if ttl.is_zero() {
            return Err(CacheError::ZeroTtl);
        }

        match self {
            Self::Disabled => {
                let _ = (key, value);
                Ok(())
            }
            #[cfg(feature = "redis")]
            Self::Redis(cache) => cache.set(key, value, ttl).await,
        }
    }

    /// 删除缓存键。失效失败不能回滚已经提交的数据库事务。
    pub async fn invalidate(&self, key: &str) -> Result<(), CacheError> {
        match self {
            Self::Disabled => {
                let _ = key;
                Ok(())
            }
            #[cfg(feature = "redis")]
            Self::Redis(cache) => cache.invalidate(key).await,
        }
    }
}

impl ReadModelCache for CacheStore {
    fn get(&self, key: String) -> ReadModelCacheFuture<'_, Option<Vec<u8>>> {
        Box::pin(async move {
            CacheStore::get(self, &key).await.map_err(|_| ReadModelCacheError::BackendUnavailable)
        })
    }

    fn set(&self, key: String, value: Vec<u8>, ttl: Duration) -> ReadModelCacheFuture<'_, ()> {
        Box::pin(async move {
            CacheStore::set(self, &key, &value, ttl)
                .await
                .map_err(|_| ReadModelCacheError::BackendUnavailable)
        })
    }

    fn invalidate(&self, key: String) -> ReadModelCacheFuture<'_, ()> {
        Box::pin(async move {
            CacheStore::invalidate(self, &key)
                .await
                .map_err(|_| ReadModelCacheError::BackendUnavailable)
        })
    }
}

impl fmt::Debug for CacheStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("CacheStore").field("backend", &self.backend()).finish()
    }
}

/// Redis 连接管理器。它可以廉价克隆并复用同一多路复用连接，不额外引入连接池。
#[cfg(feature = "redis")]
#[derive(Clone)]
pub struct RedisCache {
    connection: ConnectionManager,
}

#[cfg(feature = "redis")]
impl RedisCache {
    fn new(redis_url: &str) -> Result<Self, CacheError> {
        // 整个工作区统一使用 ring，避免 reqwest、SQLx 和 Redis 各自引入不同加密 provider。
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = redis::Client::open(redis_url).map_err(|_| CacheError::RedisConfiguration)?;
        let connection = client
            .get_connection_manager_lazy(Default::default())
            .map_err(|_| CacheError::RedisConfiguration)?;

        Ok(Self { connection })
    }

    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, CacheError> {
        let mut connection = self.connection.clone();
        redis::cmd("GET")
            .arg(key)
            .query_async(&mut connection)
            .await
            .map_err(CacheError::RedisCommand)
    }

    async fn set(&self, key: &str, value: &[u8], ttl: Duration) -> Result<(), CacheError> {
        let mut connection = self.connection.clone();
        // Redis PX 使用毫秒；亚毫秒正值向上取 1 ms，超大 Duration 则饱和到 u64 上限。
        let ttl_millis = u64::try_from(ttl.as_millis()).unwrap_or(u64::MAX).max(1);
        redis::cmd("SET")
            .arg(key)
            .arg(value)
            .arg("PX")
            .arg(ttl_millis)
            .exec_async(&mut connection)
            .await
            .map_err(CacheError::RedisCommand)
    }

    async fn invalidate(&self, key: &str) -> Result<(), CacheError> {
        let mut connection = self.connection.clone();
        redis::cmd("DEL")
            .arg(key)
            .exec_async(&mut connection)
            .await
            .map_err(CacheError::RedisCommand)
    }
}

#[cfg(feature = "redis")]
impl fmt::Debug for RedisCache {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RedisCache([REDACTED])")
    }
}

/// 缓存适配器错误。错误文本不得包含 Redis URL 或凭据。
#[derive(Debug, Error)]
pub enum CacheError {
    #[error("Redis cache support is not compiled; enable the cache-redis feature")]
    RedisFeatureDisabled,

    #[error("cache TTL must be greater than zero")]
    ZeroTtl,

    #[cfg(feature = "redis")]
    #[error("invalid Redis cache configuration")]
    RedisConfiguration,

    #[cfg(feature = "redis")]
    #[error("Redis cache command failed")]
    RedisCommand(#[source] redis::RedisError),
}
