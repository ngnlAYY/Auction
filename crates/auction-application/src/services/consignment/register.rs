//! 寄售登记的本地输入校验、准入策略与凭证交接。

use std::sync::Arc;

use auction_domain::consignment::{FeeBps, ListingMode, ListingPrice, ListingSellerName};
use auction_domain::{CommandId, EquipmentId, UserId};
use uuid::Uuid;

use crate::ports::admin::EquipmentKeySealerPort;
use crate::ports::consignment::{
    ConsignmentError, ListingCommandStore, ListingEquipmentReference, ListingEquipmentUrlValidator,
    ListingRegistered, ListingRegistrationPolicy, RegisterListingCommand, RegistrationInput,
    RegistrationReplayCrypto, RegistrationSeller, validate_listing_client_command_id,
    validate_listing_request_id,
};
use crate::ports::public::PublicSessionIdentity;

/// 经 HTTP 严格解码的登记输入。原 URL 仅在本次请求内借用。
pub struct RegisterListingInput<'a> {
    /// 客户端原命令身份，须为 UUIDv4。
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

/// 未登录登记人的非寄售输入。模式固定为非寄售、费率固定为零，不由请求决定。
pub struct AnonymousRegisterListingInput<'a> {
    /// 客户端原命令身份，须为 UUIDv4。
    pub command_id: CommandId,
    /// 待领域校验的自填游戏角色名。
    pub seller_game_name: &'a str,
    /// 待本地校验的装备链接，含敏感读取凭证。
    pub equipment_url: &'a str,
    /// 待领域校验的整型标价。
    pub price: i64,
}

/// 两个登记入口共用的、卖家与模式已确定的输入。
struct RegistrationDraft<'a> {
    command_id: CommandId,
    seller: RegistrationSeller<'a>,
    mode: ListingMode,
    equipment_url: &'a str,
    price: i64,
    expected_fee_bps: i64,
}

/// 把经认证的登记交给寄售命令账本。
pub struct RegisterListingService {
    store: Arc<dyn ListingCommandStore>,
    validator: Arc<dyn ListingEquipmentUrlValidator>,
    sealer: Option<Arc<dyn EquipmentKeySealerPort>>,
    replay_crypto: Option<Arc<dyn RegistrationReplayCrypto>>,
    policy: ListingRegistrationPolicy,
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
            policy: ListingRegistrationPolicy {
                enabled: policy.enabled && artifact_capable,
                ..policy
            },
            new_uuid: Arc::new(new_uuid),
        }
    }

    /// 校验本地输入并等待账本的原始决定。卖家身份必须来自已认证的 Public 会话。
    /// 请求号由调用方提供，仅用于审计关联。
    pub async fn register(
        &self,
        seller: &PublicSessionIdentity,
        input: RegisterListingInput<'_>,
        request_id: Option<String>,
    ) -> Result<ListingRegistered, ConsignmentError> {
        let draft = RegistrationDraft {
            command_id: input.command_id,
            seller: RegistrationSeller::Account(UserId::from_uuid(seller.user_id)),
            mode: input.mode,
            equipment_url: input.equipment_url,
            price: input.price,
            expected_fee_bps: input.expected_fee_bps,
        };
        self.submit(draft, request_id).await
    }

    /// 未登录登记人的非寄售登记。卖家名只是自填标识，资格、历史门与额度由仓储按名判定。
    pub async fn register_anonymous(
        &self,
        input: AnonymousRegisterListingInput<'_>,
        request_id: Option<String>,
    ) -> Result<ListingRegistered, ConsignmentError> {
        // 不变量 6：名称不规范失败关闭；模式与零费率固定，请求无法借匿名入口登记寄售。
        let name = ListingSellerName::new(input.seller_game_name)?;
        let draft = RegistrationDraft {
            command_id: input.command_id,
            seller: RegistrationSeller::GameName(&name),
            mode: ListingMode::Direct,
            equipment_url: input.equipment_url,
            price: input.price,
            expected_fee_bps: i64::from(FeeBps::DIRECT.value()),
        };
        self.submit(draft, request_id).await
    }

    async fn submit(
        &self,
        draft: RegistrationDraft<'_>,
        request_id: Option<String>,
    ) -> Result<ListingRegistered, ConsignmentError> {
        let RegistrationDraft { command_id, seller, mode, equipment_url, price, expected_fee_bps } =
            draft;
        let ListingEquipmentReference { identity, key } = self.validator.validate(equipment_url)?;
        validate_listing_client_command_id(command_id)?;
        if matches!(seller, RegistrationSeller::Account(user_id) if user_id.is_nil()) {
            return Err(ConsignmentError::InvalidListingRequest);
        }
        let price = ListingPrice::new(price)?;
        let expected_fee_bps = FeeBps::new(expected_fee_bps)?;
        validate_listing_request_id(request_id.as_deref())?;

        let [listing_id, work_id, aad_uuid, candidate_replay_key_id] =
            std::array::from_fn(|_| (self.new_uuid)());
        if [listing_id, work_id, aad_uuid, candidate_replay_key_id].iter().any(Uuid::is_nil) {
            return Err(ConsignmentError::Unavailable);
        }
        let command = RegisterListingCommand {
            command_id,
            request_id,
            input: RegistrationInput {
                seller,
                identity: &identity,
                key: &key,
                mode,
                price,
                expected_fee_bps,
            },
            listing_id,
            work_id,
            aad_equipment_id: EquipmentId::from_uuid(aad_uuid),
            candidate_replay_key_id,
            policy: self.policy,
        };
        // 不变量 1、2、3、5、6、8、10：仓储先仲裁账本，再对首次命令应用准入策略，
        // 并在同一事务内负责比较钥选择、MAC 和首次封装。缺少本地密码能力按 None 传递。
        // 请求号只关联首次审计，登记 MAC 只接收 RegistrationInput。
        // 不变量 7：明文 key 只借给本次仓储调用，返回即丢弃，不复制、不持久化。
        let outcome = self
            .store
            .register(command, self.sealer.as_deref(), self.replay_crypto.as_deref())
            .await;
        drop(key);
        outcome?.into_result()
    }
}
