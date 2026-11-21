//! 寄售登记的本地准入、输入校验和凭证交接。

use std::sync::Arc;

use auction_domain::consignment::{FeeBps, ListingMode, ListingPrice};
use auction_domain::{CommandId, EquipmentId, UserId};
use uuid::Uuid;

use crate::ports::admin::EquipmentKeySealerPort;
use crate::ports::consignment::{
    ConsignmentError, ListingBusinessRejection, ListingCommandStore, ListingEquipmentReference,
    ListingEquipmentUrlValidator, ListingRegistered, ListingRegistrationPolicy,
    RegisterListingCommand, RegistrationInput, RegistrationReplayCrypto,
};
use crate::ports::public::PublicSessionIdentity;

/// 经 HTTP 严格解码的登记输入。原 URL 仅在本次请求内借用。
pub struct RegisterListingInput<'a> {
    /// 客户端原命令身份。
    pub command_id: CommandId,
    /// 经请求解码的闭集模式。
    pub mode: ListingMode,
    /// 待本地校验的装备链接，含敏感读取凭证。
    pub equipment_url: &'a str,
    /// 待领域校验的整型标价。
    pub price: i64,
    /// 待领域校验的预期费率。
    pub expected_fee_bps: i64,
}

/// 把经认证的登记交给寄售命令账本。
pub struct RegisterListingService {
    store: Arc<dyn ListingCommandStore>,
    validator: Arc<dyn ListingEquipmentUrlValidator>,
    sealer: Option<Arc<dyn EquipmentKeySealerPort>>,
    replay_crypto: Option<Arc<dyn RegistrationReplayCrypto>>,
    policy: ListingRegistrationPolicy,
    artifact_capable: bool,
    new_uuid: Arc<dyn Fn() -> Uuid + Send + Sync>,
}

impl RegisterListingService {
    /// 注入事务端口、本地 URL 与密码能力、只读准入和候选身份生成器。
    pub fn new(
        store: Arc<dyn ListingCommandStore>,
        validator: Arc<dyn ListingEquipmentUrlValidator>,
        sealer: Option<Arc<dyn EquipmentKeySealerPort>>,
        replay_crypto: Option<Arc<dyn RegistrationReplayCrypto>>,
        policy: ListingRegistrationPolicy,
        artifact_capable: bool,
        new_uuid: impl Fn() -> Uuid + Send + Sync + 'static,
    ) -> Self {
        Self {
            store,
            validator,
            sealer,
            replay_crypto,
            policy,
            artifact_capable,
            new_uuid: Arc::new(new_uuid),
        }
    }

    /// 校验本地输入并等待账本的原始决定。卖家身份必须来自已认证的 Public 会话。
    pub async fn register(
        &self,
        seller: &PublicSessionIdentity,
        input: RegisterListingInput<'_>,
    ) -> Result<ListingRegistered, ConsignmentError> {
        // 不变量 1、6：此入口的关闭门不访问依赖，也不占命令 ID。2P 未提供按输入
        // 验证的只重放端口，关停期间的原回执仍可由已认证本人查询。
        if !self.artifact_capable || !self.policy.enabled {
            return Err(ConsignmentError::Disabled);
        }
        let ListingEquipmentReference { identity, key } =
            self.validator.validate(input.equipment_url)?;
        if input.command_id.is_nil() {
            return Err(ConsignmentError::InvalidCommandId);
        }
        if seller.user_id.is_nil() {
            return Err(ConsignmentError::InvalidListingRequest);
        }
        let price = ListingPrice::new(input.price)?;
        let expected_fee_bps = FeeBps::new(input.expected_fee_bps)?;
        // 不变量 6、10：本地费率不符时拒绝继续处理凭证。2P 未提供可以先重放再做
        // 本地预检的边界，历史回执仍由本人命令查询提供。
        if expected_fee_bps != FeeBps::for_mode(input.mode, self.policy.consigned_fee_bps) {
            return Err(ListingBusinessRejection::FeeChanged.into());
        }

        let [listing_id, work_id, aad_uuid, candidate_replay_key_id] =
            std::array::from_fn(|_| (self.new_uuid)());
        if [listing_id, work_id, aad_uuid, candidate_replay_key_id].iter().any(Uuid::is_nil) {
            return Err(ConsignmentError::Unavailable);
        }
        let command = RegisterListingCommand {
            command_id: input.command_id,
            seller_user_id: UserId::from_uuid(seller.user_id),
            input: RegistrationInput {
                identity: &identity,
                key: &key,
                mode: input.mode,
                price,
                expected_fee_bps,
            },
            listing_id,
            work_id,
            aad_equipment_id: EquipmentId::from_uuid(aad_uuid),
            candidate_replay_key_id,
            policy: self.policy,
        };
        // 不变量 1、2、3、5、7、8：2P 将比较钥选择、MAC 及首次封装放在同一端口内。
        // 应用只借出脱敏凭证；端口在选定原逻辑比较钥后消费借用，不得持久化明文。
        let outcome = self
            .store
            .register(command, self.sealer.as_deref(), self.replay_crypto.as_deref())
            .await;
        drop(key);
        outcome?.into_result()
    }
}
