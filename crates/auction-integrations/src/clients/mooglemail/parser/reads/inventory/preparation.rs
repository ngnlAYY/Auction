//! 发送前只读取目标装备的原生标签状态；凭据与完整响应均不进入调试投影。

use super::names::{InventoryNames, inventory_names};
use super::*;

/// 当前完整库存响应中的短期表单 token 和目标装备状态；没有命中不证明可发送。
pub(crate) struct MoogleMailInventoryPreparation {
    pub(crate) token: MoogleMailToken,
    pub(crate) equipment: Vec<MoogleMailInventoryEquipment>,
}

/// 只保存已核对的目标标签和元数据引用，不让 HTML DOM 跨过异步读取边界。
pub(crate) struct MoogleMailInventoryRead<'a> {
    token: MoogleMailToken,
    labels: Vec<(String, String)>,
    names: InventoryNames,
    wanted: HashSet<&'a str>,
    world: MoogleMailWorld,
    pub(crate) metadata_sources: Vec<String>,
}

impl MoogleMailInventoryRead<'_> {
    pub(crate) fn include_metadata(
        &mut self,
        reference: &str,
        bytes: &[u8],
    ) -> Result<(), MoogleMailError> {
        self.names.include_external(reference, bytes, &self.wanted, self.world)
    }

    /// 名称和精确标签全部匹配后才产生可供出库/解锁使用的状态。
    pub(crate) fn finish(self) -> Result<MoogleMailInventoryPreparation, MoogleMailError> {
        let fail = || protocol(MoogleMailOperation::Inventory, self.world);
        let equipment = self
            .labels
            .into_iter()
            .map(|(equipment_id, label)| {
                let name = self.names.values.get(&equipment_id).ok_or_else(fail)?;
                // t 是实际显示名；名称内的图标不能被当成装备状态。
                let prefix = label.strip_suffix(name).ok_or_else(fail)?;
                let flags = status_flags(prefix).ok_or_else(fail)?;
                Ok(MoogleMailInventoryEquipment {
                    equipment_id,
                    stored: flags & STORED != 0,
                    protected: flags & PROTECTED != 0,
                    locked: flags & LOCKED != 0,
                    equipped: flags & EQUIPPED != 0,
                })
            })
            .collect::<Result<_, _>>()?;
        Ok(MoogleMailInventoryPreparation { token: self.token, equipment })
    }
}

/// 与精确可选 EID 行及其原生名称绑定的状态，不从其它行或页面文字推断。
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct MoogleMailInventoryEquipment {
    pub(crate) equipment_id: String,
    pub(crate) stored: bool,
    pub(crate) protected: bool,
    pub(crate) locked: bool,
    pub(crate) equipped: bool,
}

impl MoogleMailParser {
    /// 保留完整库存证据，只为请求的 EID 解读状态；调用方仍须证明所有目标均唯一命中。
    pub(crate) fn parse_inventory_preparation<'a>(
        &self,
        bytes: &[u8],
        world: MoogleMailWorld,
        category: MoogleMailInventoryCategory,
        requested_ids: &'a [String],
    ) -> Result<MoogleMailInventoryRead<'a>, MoogleMailError> {
        let operation = MoogleMailOperation::Inventory;
        let fail = || protocol(operation, world);
        let mut wanted = HashSet::with_capacity(requested_ids.len());
        for eid in requested_ids {
            if !auction_application::settlement::valid_external_positive_id(eid)
                || !wanted.insert(eid.as_str())
            {
                return Err(fail());
            }
        }
        let document = trusted_document(bytes, world, operation)?;
        let mut labels = Vec::new();
        let form = self.visit_inventory_rows(&document, world, category, |eid, row, input| {
            if wanted.contains(eid) {
                labels.push((eid.to_owned(), equipment_label(eid, row, input, world)?));
            }
            Ok(())
        })?;
        let token = preparation_token(&document, form, world)?;
        let (names, metadata_sources) = if labels.is_empty() {
            (InventoryNames::default(), Vec::new())
        } else {
            inventory_names(&document, &wanted, world)?
        };
        Ok(MoogleMailInventoryRead { token, labels, names, wanted, world, metadata_sources })
    }
}

fn preparation_token(
    document: &Html,
    form: ElementRef<'_>,
    world: MoogleMailWorld,
) -> Result<MoogleMailToken, MoogleMailError> {
    let operation = MoogleMailOperation::Inventory;
    let fail = || protocol(operation, world);
    let token = unique(document, "[name='postoken']", world, operation)?;
    if token.value().name() != "input"
        || token.attr("type") != Some("hidden")
        || token.attr("disabled").is_some()
        || token.attr("form").is_some_and(|id| id != "equipform")
        || token
            .ancestors()
            .filter_map(ElementRef::wrap)
            .find(|node| node.value().name() == "form")
            .is_none_or(|node| node.id() != form.id())
        || token
            .ancestors()
            .filter_map(ElementRef::wrap)
            .any(|node| node.value().name() == "fieldset" && node.attr("disabled").is_some())
    {
        return Err(fail());
    }
    token.attr("value").and_then(MoogleMailToken::new).ok_or_else(fail)
}

fn equipment_label(
    eid: &str,
    row: ElementRef<'_>,
    input: ElementRef<'_>,
    world: MoogleMailWorld,
) -> Result<String, MoogleMailError> {
    let fail = || protocol(MoogleMailOperation::Inventory, world);
    let label = input.parent().and_then(ElementRef::wrap).ok_or_else(fail)?;
    let hook = row.attr("onmouseover").ok_or_else(fail)?.trim();
    let hook = hook.strip_suffix(';').unwrap_or(hook).trim_end();
    let hover_id = hook
        .strip_prefix("hover_equip(")
        .or_else(|| hook.strip_prefix("equips.set("))
        .and_then(|value| value.strip_suffix(')'));
    if hover_id != Some(eid)
        || label.value().name() != "label"
        || !descendant_of(label, row)
        || label.attr("for").is_some_and(|id| Some(id) != input.attr("id"))
        || input.attr("form").is_some_and(|id| id != "equipform")
        || row.descendants().filter_map(ElementRef::wrap).any(|node| {
            node.id() != row.id()
                && (node.attr("onmouseover").is_some()
                    || node.value().name() == "tr"
                    || node.value().name() == "label" && node.id() != label.id())
        })
    {
        return Err(fail());
    }
    // 原生 checkbox 后可有一个无属性、无内容的 span 装饰；只有后续唯一文字节点能声明状态。
    let mut after_input = false;
    let mut decoration = false;
    let mut label_text: Option<&str> = None;
    for child in label.children() {
        if child.id() == input.id() {
            after_input = true;
        } else if let Some(text) = child.value().as_text() {
            if after_input {
                if label_text.replace(text.as_ref()).is_some() {
                    return Err(fail());
                }
            } else if !text.trim().is_empty() {
                return Err(fail());
            }
        } else if after_input
            && !decoration
            && label_text.is_none()
            && ElementRef::wrap(child).is_some_and(|node| {
                node.value().name() == "span"
                    && node.value().attrs().next().is_none()
                    && node.children().next().is_none()
            })
        {
            decoration = true;
        } else {
            return Err(fail());
        }
    }
    label_text.map(str::to_owned).ok_or_else(fail)
}

const EQUIPPED: u8 = 1 << 2;
const STORED: u8 = 1 << 3;
const PROTECTED: u8 = 1 << 5;
const LOCKED: u8 = 1 << 6;

fn status_flags(prefix: &str) -> Option<u8> {
    let mut flags = 0;
    let mut characters = prefix.chars().peekable();
    while let Some(character) = characters.next() {
        if character.is_whitespace() {
            continue;
        }
        let (flag, required_variation, optional_variation) = match character {
            '\u{26a0}' => (1, true, false),
            '\u{274c}' => (1 << 1, false, false),
            '\u{1f5e1}' => (EQUIPPED, true, false),
            '\u{1f4e6}' => (STORED, false, false),
            '\u{1f4cc}' => (1 << 4, false, false),
            '\u{1f6e1}' => (PROTECTED, false, true),
            '\u{1f512}' => (LOCKED, false, false),
            '\u{1f53a}' => (1 << 7, false, false),
            _ => return None,
        };
        if flags & flag != 0 {
            return None;
        }
        if required_variation {
            if characters.next() != Some('\u{fe0f}') {
                return None;
            }
        } else if optional_variation && characters.peek() == Some(&'\u{fe0f}') {
            characters.next();
        }
        flags |= flag;
    }
    if flags & (PROTECTED | LOCKED) == PROTECTED | LOCKED
        || flags & (EQUIPPED | STORED) == EQUIPPED | STORED
    {
        return None;
    }
    Some(flags)
}
