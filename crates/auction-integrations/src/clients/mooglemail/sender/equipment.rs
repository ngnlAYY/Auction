//! 发送前的装备库存证明与最小出库/解锁；不回滚、不重试，也不自动卸下装备。

use std::collections::HashMap;

use super::*;
use crate::clients::mooglemail::{
    parser::MoogleMailInventoryPreparation, protocol::MoogleMailInventoryCategory,
};

impl MoogleMailSender {
    /// 只由 sending fence 后的发送流程调用；所有目标唯一定位后才允许首次 Armory POST。
    pub(super) async fn prepare_equipment(
        &self,
        cookie: &MoogleMailCookie,
        request: &MoogleMailSendRequest,
        write_started: &mut bool,
    ) -> Result<(), MoogleMailError> {
        if request.equipment_ids.is_empty() {
            return Ok(());
        }
        let mut located: HashMap<_, _> =
            request.equipment_ids.iter().map(|id| (id.as_str(), false)).collect();
        let mut inventories = Vec::with_capacity(MoogleMailInventoryCategory::ALL.len());
        // 扫描完整七类，不能发现首个匹配后就写入，否则会漏掉跨分类的重复身份。
        for category in MoogleMailInventoryCategory::ALL {
            let state = self
                .read_inventory_preparation(cookie, request, category, |state| {
                    state.equipment.iter().all(|equipment| {
                        if equipment.equipped {
                            return false;
                        }
                        match located.get_mut(equipment.equipment_id.as_str()) {
                            Some(found) if !*found => {
                                *found = true;
                                true
                            }
                            _ => false,
                        }
                    })
                })
                .await?;
            if !state.equipment.is_empty() {
                inventories.push((category, state));
            }
        }
        if located.values().any(|found| !found) {
            return Err(write_error(
                request.world,
                MoogleMailOperation::Inventory,
                MoogleMailErrorCategory::NotObserved,
            ));
        }
        for (category, discovered) in inventories {
            if discovered
                .equipment
                .iter()
                .all(|equipment| !equipment.stored && !equipment.protected && !equipment.locked)
            {
                continue;
            }
            self.prepare_equipment_category(cookie, request, category, discovered, write_started)
                .await?;
        }
        Ok(())
    }

    async fn prepare_equipment_category(
        &self,
        cookie: &MoogleMailCookie,
        request: &MoogleMailSendRequest,
        category: MoogleMailInventoryCategory,
        discovered: MoogleMailInventoryPreparation,
        write_started: &mut bool,
    ) -> Result<(), MoogleMailError> {
        let expected: HashSet<_> =
            discovered.equipment.iter().map(|equipment| equipment.equipment_id.as_str()).collect();
        let same_equipment = |state: &MoogleMailInventoryPreparation| {
            state.equipment.len() == expected.len()
                && state.equipment.iter().all(|equipment| {
                    !equipment.equipped && expected.contains(equipment.equipment_id.as_str())
                })
        };
        // 发现过程可能读取多个分类；写前重读本分类，不能提交早先页面的旧 postoken。
        let mut state =
            self.read_inventory_preparation(cookie, request, category, same_equipment).await?;
        let target = self.client.inventory_organize(request.world);
        for operation in
            [MoogleMailOperation::EquipmentUnstore, MoogleMailOperation::EquipmentUnlock]
        {
            let unstore = operation == MoogleMailOperation::EquipmentUnstore;
            let equipment_ids: Vec<_> = state
                .equipment
                .iter()
                .filter(|equipment| {
                    if unstore { equipment.stored } else { equipment.protected || equipment.locked }
                })
                .map(|equipment| equipment.equipment_id.as_str())
                .collect();
            if equipment_ids.is_empty() {
                continue;
            }
            let form = if unstore {
                MoogleMailForm::unstore_equipment(&state.token, &equipment_ids)
            } else {
                MoogleMailForm::unlock_equipment(&state.token, &equipment_ids)
            }
            .map_err(|_| {
                write_error(request.world, operation, MoogleMailErrorCategory::Configuration)
            })?;
            drop(equipment_ids);
            let response = self.client.post(&target, cookie, form).await;
            *write_started |=
                response.is_ok() || response.as_ref().is_err_and(|error| error.write_started());
            response?;
            // POST 成功页面不是状态证明；必须回读同分类，确认精确身份与期望状态并换新 token。
            state = self
                .read_inventory_preparation(cookie, request, category, |state| {
                    same_equipment(state)
                        && state.equipment.iter().all(|equipment| {
                            !equipment.stored
                                && (unstore || (!equipment.protected && !equipment.locked))
                        })
                })
                .await?;
        }
        Ok(())
    }

    async fn read_inventory_preparation(
        &self,
        cookie: &MoogleMailCookie,
        request: &MoogleMailSendRequest,
        category: MoogleMailInventoryCategory,
        accepts: impl FnOnce(&MoogleMailInventoryPreparation) -> bool,
    ) -> Result<MoogleMailInventoryPreparation, MoogleMailError> {
        let target = self.client.inventory(request.world, category);
        let document = self.client.get(&target, cookie).await?;
        let parsed = self
            .parser
            .parse_inventory_preparation(&document, request.world, category, &request.equipment_ids)
            .and_then(|state| {
                if accepts(&state) {
                    Ok(state)
                } else {
                    Err(MoogleMailError::protocol_drift(
                        MoogleMailOperation::Inventory,
                        request.world,
                    ))
                }
            });
        self.client.diagnose_parse(&target, &document, parsed).await
    }
}
