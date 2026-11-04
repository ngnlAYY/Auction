//! Armory organize 的完整正向库存证据与发送前状态读取；没有命中不是负向库存证明。

use auction_application::diagnostics::{DiagnosticContext, diagnostic_world};

use crate::diagnostics as safe_diagnostics;

use std::collections::{BTreeMap, HashSet};

use super::super::*;
use super::identity::{trusted_document, unique, verify_main_url};
use crate::clients::mooglemail::protocol::MoogleMailInventoryCategory;

mod names;
mod preparation;

pub(crate) use preparation::MoogleMailInventoryPreparation;

impl MoogleMailParser {
    /// 完整结构/计数/行身份都成立后才返回目标的正向命中，不接受 disabled 或分页片段。
    pub fn parse_inventory_presence(
        &self,
        bytes: &[u8],
        world: MoogleMailWorld,
        category: MoogleMailInventoryCategory,
        external_equipment_id: &str,
    ) -> Result<bool, MoogleMailError> {
        let operation = MoogleMailOperation::Inventory;
        let fail = || protocol(operation, world);
        if !auction_application::valid_external_positive_id(external_equipment_id) {
            return Err(fail());
        }
        let document = trusted_document(bytes, world, operation)?;
        let mut present = false;
        self.visit_inventory_rows(&document, world, category, |eid, _, _| {
            present |= eid == external_equipment_id;
            Ok(())
        })?;
        Ok(present)
    }

    /// 两种库存读取共用完整页面证据，发送准备额外验证目标标签和状态。
    fn visit_inventory_rows<'a>(
        &self,
        document: &'a Html,
        world: MoogleMailWorld,
        category: MoogleMailInventoryCategory,
        mut visit: impl FnMut(&'a str, ElementRef<'a>, ElementRef<'a>) -> Result<(), MoogleMailError>,
    ) -> Result<ElementRef<'a>, MoogleMailError> {
        let operation = MoogleMailOperation::Inventory;
        let fail = || protocol(operation, world);
        verify_main_url(document, world, operation)?;
        unique(document, "#mainpane", world, operation)?;
        let armory = unique(document, "#armory_outer", world, operation)?;
        if armory.parent().and_then(ElementRef::wrap).and_then(|node| node.attr("id"))
            != Some("mainpane")
        {
            return Err(fail());
        }
        let form = unique(document, "#equipform", world, operation)?;
        let list = unique(document, "#equiplist", world, operation)?;
        let count = unique(document, "#equipcount", world, operation)?;
        if form.value().name() != "form"
            || form.attr("method").is_none_or(|method| !method.eq_ignore_ascii_case("post"))
            || !descendant_of(form, armory)
            || !descendant_of(list, form)
            || !descendant_of(count, armory)
        {
            return Err(fail());
        }
        let mut base = Url::parse(&format!("https://hentaiverse.org{}", world.path())).map_err(
            |diagnostic_cause| {
                safe_diagnostics::error(
                    "mooglemail_inventory",
                    "build_inventory_origin",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(world.as_str()),
                        ..Default::default()
                    },
                );
                fail()
            },
        )?;
        base.query_pairs_mut()
            .append_pair("s", "Bazaar")
            .append_pair("ss", "am")
            .append_pair("screen", "organize")
            .append_pair("filter", category.as_str());
        // 生产 organize 表单省略 action，按当前固定请求 URL 提交；显式 action 仍须通过下方闭集校验。
        let action =
            base.join(form.attr("action").unwrap_or_default()).map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "mooglemail_inventory",
                    "resolve_inventory_action",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(world.as_str()),
                        ..Default::default()
                    },
                );
                fail()
            })?;
        let mut pairs = BTreeMap::new();
        for (key, value) in action.query_pairs() {
            if pairs.insert(key.into_owned(), value.into_owned()).is_some() {
                return Err(fail());
            }
        }
        let expected = BTreeMap::from([
            ("s".to_owned(), "Bazaar".to_owned()),
            ("ss".to_owned(), "am".to_owned()),
            ("screen".to_owned(), "organize".to_owned()),
            ("filter".to_owned(), category.as_str().to_owned()),
        ]);
        if action.origin() != base.origin()
            || action.path() != world.path()
            || !action.username().is_empty()
            || action.password().is_some()
            || action.fragment().is_some()
            || pairs != expected
            || document
                .select(&selector("#pager, .pager, .pagination, [rel=next]", operation, world)?)
                .next()
                .is_some()
        {
            return Err(fail());
        }
        let text = normalize_text(count.text().collect::<String>());
        let available = text
            .strip_prefix("Selected 0 of ")
            .and_then(|value| value.strip_suffix(" matching equipment available to organize"))
            .and_then(|value| value.parse::<usize>().ok())
            .ok_or_else(fail)?;
        if text != format!("Selected 0 of {available} matching equipment available to organize") {
            return Err(fail());
        }
        let inputs = selector("input[name='eqids[]']", operation, world)?;
        let mut seen = HashSet::new();
        for input in document.select(&inputs) {
            let eid = input
                .attr("value")
                .filter(|value| auction_application::valid_external_positive_id(value))
                .ok_or_else(fail)?;
            let row = input
                .ancestors()
                .filter_map(ElementRef::wrap)
                .find(|node| node.value().name() == "tr")
                .ok_or_else(fail)?;
            if !descendant_of(input, list)
                || !descendant_of(row, list)
                || input.attr("type") != Some("checkbox")
                || input.attr("disabled").is_some()
                || input.attr("checked").is_some()
                || input.ancestors().filter_map(ElementRef::wrap).any(|node| {
                    node.value().name() == "fieldset" && node.attr("disabled").is_some()
                })
                || input.attr("id").and_then(|id| id.strip_prefix('e')) != Some(eid)
                || row
                    .attr("onclick")
                    .and_then(|handler| handler.strip_prefix("return select_equip("))
                    .and_then(|handler| handler.strip_suffix(",event,this)"))
                    != Some(eid)
                || !seen.insert(eid)
            {
                return Err(fail());
            }
            visit(eid, row, input)?;
        }
        if seen.len() != available {
            return Err(fail());
        }
        Ok(form)
    }
}

fn descendant_of(node: ElementRef<'_>, ancestor: ElementRef<'_>) -> bool {
    node.ancestors().any(|candidate| candidate.id() == ancestor.id())
}
