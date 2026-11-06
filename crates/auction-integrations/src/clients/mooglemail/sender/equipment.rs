//! 发送前的装备库存证明与最小出库/解锁；不回滚、不重试，也不自动卸下装备。

use auction_application::core::diagnostics::DiagnosticContext;

use crate::diagnostics as safe_diagnostics;

use std::collections::HashMap;

use super::*;
use crate::clients::mooglemail::{
    parser::MoogleMailInventoryPreparation, protocol::MoogleMailInventoryCategory,
};

#[cfg(test)]
mod tests;

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
        let mut inventories = Vec::new();
        let all = MoogleMailInventoryCategory::ALL;
        // 冻结装备名已确定 Armory 分类时只读这一类；提示不参与身份或资格证明，
        // 未覆盖全部目标时回退完整七类，保持跨分类重复身份检测。
        let hinted = request.category.map(|category| [category]);
        let categories: &[MoogleMailInventoryCategory] =
            hinted.as_ref().map_or(&all, |single| single.as_slice());
        self.scan_equipment_categories(cookie, request, categories, &mut located, &mut inventories)
            .await?;
        if located.values().any(|found| !found) && request.category.is_some() {
            located = request.equipment_ids.iter().map(|id| (id.as_str(), false)).collect();
            inventories.clear();
            self.scan_equipment_categories(cookie, request, &all, &mut located, &mut inventories)
                .await?;
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

    /// 逐类读取库存并把全部目标定位到唯一分类；任何目标重复出现都会失败关闭。
    async fn scan_equipment_categories<'a>(
        &self,
        cookie: &MoogleMailCookie,
        request: &'a MoogleMailSendRequest,
        categories: &[MoogleMailInventoryCategory],
        located: &mut HashMap<&'a str, bool>,
        inventories: &mut Vec<(MoogleMailInventoryCategory, MoogleMailInventoryPreparation)>,
    ) -> Result<(), MoogleMailError> {
        for &category in categories {
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
            .map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "mooglemail_send",
                    "build_organize_form",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
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
        let pending = self.parser.parse_inventory_preparation(
            &document,
            request.world,
            category,
            &request.equipment_ids,
        );
        let mut pending = self.client.diagnose_parse(&target, &document, pending).await?;
        // 只读取本页引用的同源元数据；全部正向证明完成前不执行任何 Armory POST。
        for reference in std::mem::take(&mut pending.metadata_sources) {
            let metadata_target = self.client.inventory_metadata(request.world, &reference)?;
            let source = self.client.get(&metadata_target, cookie).await?;
            let parsed = pending.include_metadata(&reference, &source);
            self.client.diagnose_parse(&metadata_target, &source, parsed).await?;
        }
        let parsed = pending.finish().and_then(|state| {
            if accepts(&state) {
                Ok(state)
            } else {
                Err(MoogleMailError::protocol_drift(MoogleMailOperation::Inventory, request.world))
            }
        });
        self.client.diagnose_parse(&target, &document, parsed).await
    }
}
