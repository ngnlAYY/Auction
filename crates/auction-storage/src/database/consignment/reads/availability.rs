//! 只读检查持久准入、Worker 心跳、可登记世界以及本地比较钥能力。

use auction_application::consignment::{
    ConsignmentError, ListingRegistrationAvailability, ListingWorlds, RegistrationReplayCrypto,
    WrappedRegistrationReplayKey,
};
use uuid::Uuid;

use crate::database::DatabaseStore;
use crate::database::consignment::lookup::control::read_admission;
use crate::database::consignment::reads::{snapshot, storage_error, unavailable};

pub(super) async fn registration(
    store: &DatabaseStore,
    replay_crypto: &dyn RegistrationReplayCrypto,
) -> Result<ListingRegistrationAvailability, ConsignmentError> {
    let (mut tx, _) = snapshot(store, false).await?;
    let admission = read_admission(&mut tx).await.map_err(storage_error)?;
    if !admission.permit.admitting || !admission.worker_ready {
        return Ok(ListingRegistrationAvailability::Unavailable);
    }
    let (main, isekai): (bool, bool) = sqlx::query_as(
        "SELECT coalesce(bool_or(id = 'main'), false), coalesce(bool_or(id = 'isekai'), false)
         FROM public.worlds WHERE id IN ('main', 'isekai') AND enabled AND availability = 'available'",
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(storage_error)?;
    if !main && !isekai {
        return Ok(ListingRegistrationAvailability::Unavailable);
    }
    let current: Option<(Uuid, Vec<u8>, Vec<u8>, String)> = sqlx::query_as(
        "SELECT id, wrapped_key, nonce, encryption_key_id
         FROM public.consignment_replay_keys WHERE write_current",
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(storage_error)?;
    tx.commit().await.map_err(storage_error)?;

    // 不变量 7：没有现存逻辑钥时只做本地封装自检，不持久化生成物。
    // 当前端口没有单独探测初始化能力的方法。
    let usable = if let Some((replay_key_id, ciphertext, nonce, key_id)) = current {
        let nonce = nonce.try_into().map_err(unavailable)?;
        WrappedRegistrationReplayKey::new(ciphertext, nonce, key_id)
            .and_then(|wrapped| replay_crypto.check_wrapped(replay_key_id, &wrapped))
    } else {
        let replay_key_id = Uuid::new_v4();
        replay_crypto
            .generate_wrapped(replay_key_id)
            .and_then(|wrapped| replay_crypto.check_wrapped(replay_key_id, &wrapped))
    };
    match usable {
        Ok(()) => Ok(ListingRegistrationAvailability::Ready(ListingWorlds { main, isekai })),
        Err(auction_application::consignment::ListingCredentialError) => {
            Ok(ListingRegistrationAvailability::Unavailable)
        }
    }
}
