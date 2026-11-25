//! 受限权利 ID 的确定性派生：合同与角色唯一决定一个权利。
//!
//! 字节格式冻结：`UUIDv5(NAMESPACE_URL, "auction.consignment.right.v1\0" ‖ contract_id 原 16 字节 ‖ role)`，
//! 与迁移 000137 的 `c7_right_id` 逐字节一致；7D-6 登记新权利必须用同一派生，不得另起算法。

use uuid::Uuid;

/// 权利 ID 派生的域前缀（含结尾的 NUL 分隔字节）。
const RIGHT_ID_PREFIX: &[u8] = b"auction.consignment.right.v1\0";

/// 合同上的受益角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RightRole {
    /// 买方。
    Buyer,
    /// 卖方。
    Seller,
}

impl RightRole {
    /// 参与派生的稳定角色文本；与 SQL `c7_right_id` 接受的取值一致。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Buyer => "buyer",
            Self::Seller => "seller",
        }
    }
}

/// 派生合同上某一角色的权利 ID。
pub fn right_id(contract_id: Uuid, role: RightRole) -> Uuid {
    let mut name = Vec::with_capacity(RIGHT_ID_PREFIX.len() + 16 + 6);
    name.extend_from_slice(RIGHT_ID_PREFIX);
    name.extend_from_slice(contract_id.as_bytes());
    name.extend_from_slice(role.as_str().as_bytes());
    Uuid::new_v5(&Uuid::NAMESPACE_URL, &name)
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{RightRole, right_id};

    #[test]
    fn right_id_is_a_frozen_uuid_v5_per_contract_and_role() {
        let contract = Uuid::from_u128(0x0123_4567_89ab_4def_8123_4567_89ab_cdef);
        let buyer = right_id(contract, RightRole::Buyer);
        let seller = right_id(contract, RightRole::Seller);
        assert_eq!(buyer.get_version_num(), 5);
        assert_ne!(buyer, seller);
        // 向量由 Python uuid.uuid5 独立算出，钉住字节格式；改动派生即破坏既有权利。
        assert_eq!(buyer.to_string(), "540a2ee6-711f-566b-9b56-de8c4c73cebb");
        assert_eq!(seller.to_string(), "bd58f8ef-836b-5d89-9d12-48b1c9113419");
    }
}
