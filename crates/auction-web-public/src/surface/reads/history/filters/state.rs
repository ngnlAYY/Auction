//! 成交价历史的已应用地址、可编辑草稿与提交门禁。

use auction_web_shared::QueryWriteMode;
use auction_web_shared::a11y::focus_element_by_id_after_render;
use auction_web_shared::validation::escaped_single_line_input;
use dioxus::prelude::*;

use super::model::{
    HistoryDateError, HistoryErrors, HistoryField, HistoryFilterError, HistoryPriceUnit,
};
use super::validation::{
    history_date_value, history_error_message, history_name_value, history_price_value,
};
use crate::api::PriceHistoryQuery;
use crate::surface::reads::history::values::{
    HistoryAddress, history_active_filter_count, history_date_parts, history_price_parts,
    history_query_errors,
};
use crate::surface::{
    PublicBoardQuery, commit_public_query_pairs, page_pathname, read_page_pairs, read_page_query,
    use_public_query_pairs,
};

pub(in crate::surface::reads::history) const HISTORY_PATH: &str = "/history";

#[derive(Clone)]
pub(super) struct DateDraft {
    pub(super) month: String,
    pub(super) year: String,
}

impl DateDraft {
    fn from_address(value: Option<&str>) -> Self {
        let (month, year) = history_date_parts(value);
        Self { month, year }
    }
}

#[derive(Clone)]
pub(super) struct PriceDraft {
    pub(super) value: String,
    pub(super) unit: HistoryPriceUnit,
}

impl PriceDraft {
    fn from_address(value: Option<&str>) -> Self {
        let (value, unit) = history_price_parts(value);
        Self { value, unit }
    }
}

#[derive(Clone)]
pub(super) struct HistoryDraft {
    pub(super) world: String,
    pub(super) name: String,
    pub(super) from: DateDraft,
    pub(super) to: DateDraft,
    pub(super) minimum: PriceDraft,
    pub(super) maximum: PriceDraft,
    pub(super) buyer: String,
    pub(super) seller: String,
}

impl HistoryDraft {
    fn from_address(address: &HistoryAddress) -> Self {
        let query = &address.query;
        Self {
            world: query.world.clone().unwrap_or_else(|| "all".to_owned()),
            name: query.name.clone().unwrap_or_default(),
            from: DateDraft::from_address(query.from.as_deref()),
            to: DateDraft::from_address(query.to.as_deref()),
            minimum: PriceDraft::from_address(query.min_price.as_deref()),
            maximum: PriceDraft::from_address(query.max_price.as_deref()),
            buyer: query.buyer.clone().unwrap_or_default(),
            seller: query.seller.clone().unwrap_or_default(),
        }
    }

    fn text(&self, field: HistoryField) -> &str {
        match field {
            HistoryField::World => &self.world,
            HistoryField::Name => &self.name,
            HistoryField::From => &self.from.year,
            HistoryField::To => &self.to.year,
            HistoryField::Minimum => &self.minimum.value,
            HistoryField::Maximum => &self.maximum.value,
            HistoryField::Buyer => &self.buyer,
            HistoryField::Seller => &self.seller,
        }
    }

    fn validated(
        &self,
        applied: &HistoryAddress,
        edited: [bool; 8],
    ) -> (PriceHistoryQuery, HistoryErrors) {
        let mut errors = HistoryErrors::default();
        for field in HistoryField::ALL {
            // 重复键和非法原文只由编辑该字段解除；区间错误随两端重算。
            let error = applied
                .errors
                .get(field)
                .filter(|error| !edited[field as usize] && !error.is_range());
            errors.set(field, error);
        }
        let name = validated_field(history_name_value(&self.name), &mut errors, HistoryField::Name);
        let from = draft_date(&self.from, applied, HistoryField::From, &mut errors);
        let to = draft_date(&self.to, applied, HistoryField::To, &mut errors);
        let min_price = draft_price(&self.minimum, HistoryField::Minimum, &mut errors);
        let max_price = draft_price(&self.maximum, HistoryField::Maximum, &mut errors);
        let query = PriceHistoryQuery {
            world: (self.world != "all").then(|| self.world.clone()),
            name,
            from,
            to,
            min_price,
            max_price,
            buyer: (!self.buyer.is_empty()).then(|| self.buyer.clone()),
            seller: (!self.seller.is_empty()).then(|| self.seller.clone()),
            sort: applied.query.sort.clone(),
            page: 1,
        };
        history_query_errors(&query, &mut errors);
        (query, errors)
    }
}

fn validated_field(
    result: Result<Option<String>, HistoryFilterError>,
    errors: &mut HistoryErrors,
    field: HistoryField,
) -> Option<String> {
    match result {
        Ok(value) => value,
        Err(error) => {
            if errors.get(field).is_none() {
                errors.set(field, Some(error));
            }
            None
        }
    }
}

fn draft_date(
    draft: &DateDraft,
    applied: &HistoryAddress,
    field: HistoryField,
    errors: &mut HistoryErrors,
) -> Option<String> {
    let end = field == HistoryField::To;
    let date = history_date_value(&draft.month, &draft.year, end, field.value(&applied.query));
    validated_field(
        date.map_err(|error| match (end, error) {
            (false, HistoryDateError::Incomplete) => HistoryFilterError::FromDateIncomplete,
            (false, HistoryDateError::Invalid) => HistoryFilterError::FromDateInvalid,
            (true, HistoryDateError::Incomplete) => HistoryFilterError::ToDateIncomplete,
            (true, HistoryDateError::Invalid) => HistoryFilterError::ToDateInvalid,
        }),
        errors,
        field,
    )
}

fn draft_price(
    draft: &PriceDraft,
    field: HistoryField,
    errors: &mut HistoryErrors,
) -> Option<String> {
    let price = history_price_value(&draft.value, draft.unit);
    validated_field(
        price.map_err(|error| match (field == HistoryField::Minimum, draft.unit, error) {
            (true, HistoryPriceUnit::C, _) => HistoryFilterError::MinimumPriceC,
            (true, HistoryPriceUnit::K, _) => HistoryFilterError::MinimumPriceK,
            (true, HistoryPriceUnit::M, _) => HistoryFilterError::MinimumPriceM,
            (false, HistoryPriceUnit::C, _) => HistoryFilterError::MaximumPriceC,
            (false, HistoryPriceUnit::K, _) => HistoryFilterError::MaximumPriceK,
            (false, HistoryPriceUnit::M, _) => HistoryFilterError::MaximumPriceM,
        }),
        errors,
        field,
    )
}

#[derive(Clone, Copy, PartialEq)]
pub(in crate::surface::reads::history) struct HistoryForm {
    query_state: Signal<PublicBoardQuery>,
    pub(in crate::surface::reads::history) applied: Memo<HistoryAddress>,
    pub(super) draft: Signal<HistoryDraft>,
    pub(super) settings_open: Signal<bool>,
    errors: Signal<HistoryErrors>,
    edited: Signal<[bool; 8]>,
    baseline: Signal<HistoryAddress>,
    pub(super) announcement: Signal<Option<HistoryFilterError>>,
}

pub(in crate::surface::reads::history) fn use_history_form(
    query_state: Signal<PublicBoardQuery>,
) -> HistoryForm {
    let pairs = use_public_query_pairs(query_state);
    let applied = use_memo(move || HistoryAddress::read(&query_state.read(), &pairs.read()));
    let applied_filters = use_memo(move || applied().form());
    let form = HistoryForm {
        query_state,
        applied,
        draft: use_signal(|| HistoryDraft::from_address(&applied_filters.peek())),
        settings_open: use_signal(|| applied_filters.peek().errors.in_settings()),
        errors: use_signal(|| applied_filters.peek().errors),
        edited: use_signal(|| [false; 8]),
        baseline: use_signal(|| applied_filters.peek().clone()),
        announcement: use_signal(|| None),
    };
    use_effect(move || {
        let current = applied_filters();
        if current != *form.baseline.peek() {
            form.restore(current);
        }
    });
    form
}

impl HistoryForm {
    fn restore(mut self, applied: HistoryAddress) {
        self.draft.set(HistoryDraft::from_address(&applied));
        self.errors.set(applied.errors);
        if applied.errors.in_settings() {
            self.settings_open.set(true);
        }
        self.edited.set([false; 8]);
        self.announcement.set(None);
        self.baseline.set(applied.form());
    }

    pub(super) fn error(self, field: HistoryField) -> Option<String> {
        let error = self.errors.read().get(field)?;
        let message = history_error_message(error);
        let baseline = self.baseline.read();
        let raw = baseline.raw_values(field);
        let original = if error == HistoryFilterError::Duplicate {
            Some(raw.iter().map(|value| format!("{value:?}")).collect::<Vec<_>>().join(" / "))
        } else if error == HistoryFilterError::WorldInvalid {
            Some(format!("{:?}", self.draft.read().world))
        } else if !self.edited.read()[field as usize]
            && matches!(field, HistoryField::From | HistoryField::To)
        {
            raw.first().map(|value| format!("{value:?}"))
        } else {
            escaped_single_line_input(self.draft.read().text(field))
        };
        Some(if let Some(original) = original { format!("{message} {original}") } else { message })
    }

    pub(super) fn edit(mut self, field: HistoryField, update: impl FnOnce(&mut HistoryDraft)) {
        update(&mut self.draft.write());
        self.edited.write()[field as usize] = true;
        let errors = *self.errors.peek();
        if errors.first().is_some() {
            let (_, candidate_errors) = self.validate(&self.baseline.peek());
            let mut remaining = errors;
            for field in HistoryField::ALL {
                if errors.get(field) != candidate_errors.get(field) {
                    remaining.set(field, None);
                }
            }
            self.errors.set(remaining);
        }
        self.announcement.set(None);
    }

    pub(super) fn blur(mut self, field: HistoryField) {
        let edited = *self.edited.peek();
        if !edited[field as usize] && !field.partner().is_some_and(|other| edited[other as usize]) {
            return;
        }
        let (_, validated) = self.validate(&self.baseline.peek());
        let mut visible = *self.errors.peek();
        let mut announcement = None;
        for key in std::iter::once(field).chain(field.partner()) {
            let previous = visible.get(key);
            let next = validated.get(key);
            if key != field
                && !previous.is_some_and(HistoryFilterError::is_range)
                && !next.is_some_and(HistoryFilterError::is_range)
            {
                continue;
            }
            if next != previous {
                announcement = announcement.or(next);
                visible.set(key, next);
            }
        }
        self.errors.set(visible);
        self.announcement.set(announcement);
    }

    fn validate(self, current: &HistoryAddress) -> (PriceHistoryQuery, HistoryErrors) {
        self.draft.peek().validated(current, *self.edited.peek())
    }

    pub(super) fn apply(self) {
        if page_pathname() != HISTORY_PATH {
            return;
        }
        let current = HistoryAddress::read(&read_page_query(), &read_page_pairs());
        self.apply_at(current, |candidate| write_filters(self.query_state, Some(candidate)));
    }

    fn apply_at(mut self, current: HistoryAddress, write: impl FnOnce(&PriceHistoryQuery) -> bool) {
        let matches_baseline = current.clone().form() == *self.baseline.peek();
        if !matches_baseline {
            self.restore(current);
            return;
        }
        let (candidate, errors) = self.validate(&current);
        if let Some(field) = errors.first() {
            self.errors.set(errors);
            if errors.in_settings() {
                self.settings_open.set(true);
            }
            focus_element_by_id_after_render(field.focus_id());
            return;
        }
        if (current.errors.first().is_some() || current.query != candidate) && !write(&candidate) {
            self.restore(current);
            return;
        }
        self.errors.set(HistoryErrors::default());
        self.announcement.set(None);
    }

    pub(super) fn reset(mut self) {
        if page_pathname() != HISTORY_PATH {
            return;
        }
        let board = read_page_query();
        let current = HistoryAddress::read(&board, &read_page_pairs());
        let changes = history_active_filter_count(&current.query) != 0
            || current.errors.first().is_some()
            || board.page != 1
            || board.history_sort != "closed_desc"
            || board.history_group != "equipment";
        if changes && !write_filters(self.query_state, None) {
            self.restore(current);
            return;
        }
        self.restore(HistoryAddress::read(&PublicBoardQuery::default(), &[]));
        self.settings_open.set(false);
    }
}

fn write_filters(state: Signal<PublicBoardQuery>, candidate: Option<&PriceHistoryQuery>) -> bool {
    let mut keys = HistoryField::ALL.iter().map(|field| field.key()).collect::<Vec<_>>();
    keys.push("page");
    if candidate.is_none() {
        keys.extend(["history_sort", "history_group"]);
    }
    let pairs = candidate.map_or_else(Vec::new, |query| {
        HistoryField::ALL
            .iter()
            .filter_map(|field| Some((field.key().to_owned(), field.value(query)?.to_owned())))
            .collect()
    });
    // 地址栏是已应用筛选的提交点；失败时恢复该地址的原值与错误。
    commit_public_query_pairs(state, &keys, &pairs, QueryWriteMode::Push)
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use dioxus::prelude::*;

    use super::{HistoryDraft, HistoryField, HistoryFilterError, HistoryForm, use_history_form};
    use crate::api::PriceHistoryQuery;
    use crate::surface::reads::history::values::HistoryAddress;
    use crate::surface::reads::history::{filter_history_player, write_history_control};
    use crate::surface::{PageBrowserFixture, PublicBoardQuery, settle_page, use_page_query};

    #[test]
    fn submit_keeps_unedited_duplicates_and_invalid_dates_until_each_field_is_repaired() {
        let applied = HistoryAddress::read(
            &PublicBoardQuery::default(),
            &[
                ("name".to_owned(), "Robe".to_owned()),
                ("buyer".to_owned(), "First".to_owned()),
                ("buyer".to_owned(), "Second".to_owned()),
                ("from".to_owned(), "2026-02-30".to_owned()),
            ],
        );
        let mut draft = HistoryDraft::from_address(&applied);
        assert_eq!(draft.from.year, "2026-02-30");
        let mut edited = [false; 8];
        assert_eq!(draft.validated(&applied, edited).1.first(), Some(HistoryField::From));
        draft.from.year = "2026".to_owned();
        draft.from.month = "02".to_owned();
        edited[HistoryField::From as usize] = true;
        assert_eq!(draft.validated(&applied, edited).1.first(), Some(HistoryField::Buyer));
        edited[HistoryField::Buyer as usize] = true;
        let (candidate, errors) = draft.validated(&applied, edited);
        assert_eq!(errors.first(), None);
        assert_eq!(candidate.from.as_deref(), Some("2026-02-01"));
        assert_eq!(candidate.buyer.as_deref(), Some("First"));
    }

    #[derive(Default)]
    struct FormTrace {
        form: Cell<Option<HistoryForm>>,
        requests: RefCell<Vec<PriceHistoryQuery>>,
    }

    #[component]
    fn FormHarness() -> Element {
        let trace = consume_context::<Rc<FormTrace>>();
        let form = use_history_form(use_page_query());
        trace.form.set(Some(form));
        use_resource(move || {
            let request = form.applied.read().request(20);
            let trace = trace.clone();
            async move {
                if let Some((query, _)) = request {
                    trace.requests.borrow_mut().push(query);
                }
            }
        });
        rsx! {}
    }

    #[test]
    fn correcting_the_buyer_only_reads_after_the_address_accepts_it() {
        let trace = Rc::new(FormTrace::default());
        let original = "買".repeat(86);
        let browser = PageBrowserFixture::new("/history", &format!("name=Robe&buyer={original}"));
        let mut dom = VirtualDom::new(FormHarness)
            .with_root_context(trace.clone())
            .with_root_context(browser.clone());
        dom.rebuild_in_place();
        settle_page(&mut dom);
        let form = trace.form.get().unwrap();

        browser.allow_writes(false);
        dom.in_scope(ScopeId::APP, || {
            assert_eq!(form.draft.peek().buyer, original);
            assert_eq!(
                form.errors.peek().get(HistoryField::Buyer),
                Some(HistoryFilterError::PlayerLength)
            );
            assert!(trace.requests.borrow().is_empty());
            form.edit(HistoryField::Buyer, |draft| draft.buyer = "One".to_owned());
            form.apply();
            assert_eq!(form.draft.peek().buyer, original);
            assert_eq!(browser.state().history_buyer.as_deref(), Some(original.as_str()));
        });
        assert!(trace.requests.borrow().is_empty());

        browser.allow_writes(true);
        dom.in_scope(ScopeId::APP, || {
            form.edit(HistoryField::Buyer, |draft| draft.buyer = "One".to_owned());
            form.apply();
            assert_eq!(browser.state().history_buyer.as_deref(), Some("One"));
        });
        settle_page(&mut dom);
        {
            let requests = trace.requests.borrow();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].buyer.as_deref(), Some("One"));
            assert_eq!(requests[0].name.as_deref(), Some("Robe"));
        }

        browser.allow_writes(false);
        dom.in_scope(ScopeId::APP, || {
            form.reset();
            assert_eq!(form.draft.peek().buyer, "One");
            assert_eq!(browser.state().history_buyer.as_deref(), Some("One"));
        });
        browser.allow_writes(true);
        dom.in_scope(ScopeId::APP, || {
            form.reset();
            assert_eq!(browser.state().history_buyer, None);
            assert!(form.draft.peek().buyer.is_empty());
        });
        settle_page(&mut dom);
        assert_eq!(trace.requests.borrow().len(), 1);
    }

    #[test]
    fn result_controls_commit_only_their_keys_to_the_history_signal() {
        let trace = Rc::new(FormTrace::default());
        let browser = PageBrowserFixture::new("/history", "name=Robe&page=3&keep=yes");
        let mut dom = VirtualDom::new(FormHarness)
            .with_root_context(trace.clone())
            .with_root_context(browser.clone());
        dom.rebuild_in_place();
        settle_page(&mut dom);
        let state = trace.form.get().unwrap().query_state;
        dom.in_scope(ScopeId::APP, || {
            assert!(write_history_control(
                state,
                &["history_group"],
                "history_group",
                "buyer".to_owned()
            ));
            assert_eq!(browser.state().history_group, "buyer");
            assert_eq!(browser.state().page, 3);
            assert!(write_history_control(
                state,
                &["history_sort", "page"],
                "history_sort",
                "price_asc".to_owned()
            ));
            assert_eq!(browser.state().history_sort, "price_asc");
            assert_eq!(browser.state().page, 1);
            filter_history_player(state, "buyer", "One".to_owned());
            filter_history_player(state, "seller", "Two".to_owned());
            assert_eq!(browser.state().history_buyer.as_deref(), Some("One"));
            assert_eq!(browser.state().history_seller.as_deref(), Some("Two"));
            assert_eq!(browser.state().history_name.as_deref(), Some("Robe"));
        });
        assert_eq!(browser.value("keep").as_deref(), Some("yes"));
    }
}
