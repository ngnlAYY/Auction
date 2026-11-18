// 价格历史筛选器的表单和字段控件。
// 世界是“筛选条件”面板里的一项，按钮只改草稿，点「应用」才生效，缺省为全部世界；Rust 版把
// 世界下拉框放在表单的主筛选行。

import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, onSettled, Show } from "solid-js";

import { worldLabel } from "../../../board/world";
import { HistoryApply } from "../../../presentation/mobile";
import type { WorldSwitchOption } from "../../../query";
import type { HistoryFilterError, HistoryPriceUnit } from "./model";
import { historyPriceUnitAsStr, historyPriceUnitFromStr } from "./model";
import {
  historyDateError,
  historyMonthLabel,
  historyNameError,
  historyNameValue,
  historyPriceError,
} from "./validation";

const HISTORY_MONTHS = Object.freeze(Array.from({ length: 12 }, (_, index) => index + 1));

/** 渲染价格历史的主筛选表单。 */
export function PriceHistoryFilters(props: {
  readonly world: string;
  readonly worldOptions: readonly WorldSwitchOption[];
  readonly name: string;
  readonly fromMonth: string;
  readonly fromYear: string;
  readonly toMonth: string;
  readonly toYear: string;
  readonly minimumPrice: string;
  readonly minimumPriceUnit: HistoryPriceUnit;
  readonly maximumPrice: string;
  readonly maximumPriceUnit: HistoryPriceUnit;
  readonly buyer: string;
  readonly seller: string;
  readonly activeFilterCount: number;
  readonly settingsOpen: boolean;
  readonly filterError: HistoryFilterError | null;
  readonly setWorld: (world: string) => void;
  readonly setName: (name: string) => void;
  readonly setFromMonth: (month: string) => void;
  readonly setFromYear: (year: string) => void;
  readonly setToMonth: (month: string) => void;
  readonly setToYear: (year: string) => void;
  readonly setMinimumPrice: (price: string) => void;
  readonly setMinimumPriceUnit: (unit: HistoryPriceUnit) => void;
  readonly setMaximumPrice: (price: string) => void;
  readonly setMaximumPriceUnit: (unit: HistoryPriceUnit) => void;
  readonly setBuyer: (buyer: string) => void;
  readonly setSeller: (seller: string) => void;
  readonly setSettingsOpen: (open: boolean) => void;
  readonly setFilterError: (error: HistoryFilterError | null) => void;
  readonly onApply: () => void;
  readonly onReset: () => void;
}): JSX.Element {
  const currentError = createValueMemo(() => props.filterError);
  // 与顶部按钮组相同的条件：至少两个开放世界，或地址正指向一个已关闭世界的禁用项。
  const worldFilterVisible = createValueMemo(() => {
    const options = props.worldOptions;
    return (
      options.filter((option) => !option.disabled).length > 1 ||
      options.some((option) => option.disabled)
    );
  });
  const nameError = createValueMemo(() => historyNameError(currentError()));
  const nameDescription = createValueMemo(() =>
    nameError() !== null
      ? "public-history-name-hint public-history-name-error"
      : "public-history-name-hint",
  );
  const fromDateError = createValueMemo(() => historyDateError(currentError(), true));
  const toDateError = createValueMemo(() => historyDateError(currentError(), false));
  const minimumPriceError = createValueMemo(() => historyPriceError(currentError(), true));
  const maximumPriceError = createValueMemo(() => historyPriceError(currentError(), false));
  let settingsToggle: HTMLButtonElement | undefined;
  const closeSettings = () => {
    props.setSettingsOpen(false);
    const toggle = settingsToggle;
    if (toggle !== undefined) {
      onSettled(() => {
        toggle.focus();
      });
    }
  };
  return (
    <form
      class="public-history-filters"
      autocomplete="off"
      onSubmit={(event) => {
        event.preventDefault();
        props.onApply();
      }}
    >
      <div class="public-history-primary-filters">
        <label for="public-history-name">
          <span>{t("history-filter-name")}</span>
          <input
            id="public-history-name"
            name="name"
            type="search"
            enterkeyhint="search"
            autocapitalize="none"
            spellcheck="false"
            value={props.name}
            placeholder={t("history-filter-name-placeholder")}
            maxlength="1024"
            aria-invalid={nameError() !== null ? "true" : "false"}
            aria-describedby={nameDescription()}
            onInput={(event) => {
              props.setName(event.currentTarget.value);
              props.setFilterError(null);
            }}
            onBlur={() => {
              const name = historyNameValue(props.name);
              const error = name.ok ? null : name.error;
              if (error !== null || historyNameError(props.filterError) !== null) {
                props.setFilterError(error);
              }
            }}
          />
        </label>
        <div class="public-history-filter-toolbar">
          <button
            type="button"
            class="public-history-settings-toggle"
            aria-controls="public-history-settings"
            aria-expanded={props.settingsOpen ? "true" : "false"}
            ref={(element) => {
              settingsToggle = element;
            }}
            onClick={() => props.setSettingsOpen(!props.settingsOpen)}
          >
            <span aria-hidden="true">⚙</span>
            {t("history-filter-settings")}
            <Show when={props.activeFilterCount > 0}>
              <span class="public-history-active-count">
                {t("history-filter-settings-applied", { count: props.activeFilterCount })}
              </span>
            </Show>
          </button>
          <button type="submit" class="public-history-search public-history-filter-actions__apply">
            {t("history-filter-apply")}
          </button>
        </div>
      </div>
      <p id="public-history-name-hint" class="public-history-name-hint">
        {t("history-filter-name-hint")}
      </p>
      <Show when={nameError() !== null}>
        <p id="public-history-name-error" class="public-history-field-error" role="alert">
          {nameError()}
        </p>
      </Show>
      <Show when={props.settingsOpen}>
        {/* biome-ignore lint/a11y/useSemanticElements: 源码使用 role=group 的 div，保留既有 DOM。 */}
        <div
          id="public-history-settings"
          class="public-history-settings-panel"
          role="group"
          aria-label={t("history-filter-settings")}
          onKeyDown={(event) => {
            if (event.key === "Escape") {
              event.preventDefault();
              event.stopPropagation();
              closeSettings();
            }
          }}
        >
          <div class="public-history-settings-header">
            <strong>{t("history-filter-settings")}</strong>
            <button
              type="button"
              class="public-history-settings-close"
              aria-label={t("history-filter-settings-close")}
              onClick={closeSettings}
            >
              <span aria-hidden="true">×</span>
            </button>
          </div>
          <div class="public-history-settings-grid">
            <Show when={worldFilterVisible()}>
              <fieldset class="public-history-filter-cell public-history-filter-world">
                <legend>{t("history-filter-world")}</legend>
                <div class="public-history-world-options">
                  <For each={props.worldOptions} keyed={(option) => option.value}>
                    {(option) => (
                      <button
                        type="button"
                        class="public-auction-filter__tab"
                        aria-pressed={props.world === option().value ? "true" : "false"}
                        disabled={option().disabled}
                        onClick={() => props.setWorld(option().value)}
                      >
                        {option().value === "all"
                          ? t("auction-history-world-all")
                          : worldLabel(option().value)}
                      </button>
                    )}
                  </For>
                </div>
              </fieldset>
            </Show>
            <HistoryDateFilter
              fieldName="from"
              label={t("history-filter-from")}
              monthLabel={t("history-filter-from-month")}
              yearLabel={t("history-filter-from-year")}
              month={props.fromMonth}
              year={props.fromYear}
              error={fromDateError()}
              setMonth={props.setFromMonth}
              setYear={props.setFromYear}
              setFilterError={props.setFilterError}
            />
            <HistoryDateFilter
              fieldName="to"
              label={t("history-filter-to")}
              monthLabel={t("history-filter-to-month")}
              yearLabel={t("history-filter-to-year")}
              month={props.toMonth}
              year={props.toYear}
              error={toDateError()}
              setMonth={props.setToMonth}
              setYear={props.setToYear}
              setFilterError={props.setFilterError}
            />
            <HistoryPriceFilter
              fieldName="minimum_price"
              label={t("history-filter-min-price")}
              valueLabel={t("history-filter-min-price-value")}
              unitLabel={t("history-filter-min-price-unit")}
              priceValue={props.minimumPrice}
              priceUnit={props.minimumPriceUnit}
              error={minimumPriceError()}
              setPriceValue={props.setMinimumPrice}
              setPriceUnit={props.setMinimumPriceUnit}
              setFilterError={props.setFilterError}
            />
            <HistoryPriceFilter
              fieldName="maximum_price"
              label={t("history-filter-max-price")}
              valueLabel={t("history-filter-max-price-value")}
              unitLabel={t("history-filter-max-price-unit")}
              priceValue={props.maximumPrice}
              priceUnit={props.maximumPriceUnit}
              error={maximumPriceError()}
              setPriceValue={props.setMaximumPrice}
              setPriceUnit={props.setMaximumPriceUnit}
              setFilterError={props.setFilterError}
            />
            <label class="public-history-filter-cell">
              <span>{t("history-filter-buyer")}</span>
              <input
                name="buyer"
                type="search"
                enterkeyhint="search"
                autocapitalize="none"
                spellcheck="false"
                value={props.buyer}
                maxlength="256"
                onInput={(event) => {
                  props.setBuyer(event.currentTarget.value);
                  props.setFilterError(null);
                }}
              />
            </label>
            <label class="public-history-filter-cell">
              <span>{t("history-filter-seller")}</span>
              <input
                name="seller"
                type="search"
                enterkeyhint="search"
                autocapitalize="none"
                spellcheck="false"
                value={props.seller}
                maxlength="256"
                onInput={(event) => {
                  props.setSeller(event.currentTarget.value);
                  props.setFilterError(null);
                }}
              />
            </label>
          </div>
          <p class="public-history-price-hint">{t("history-filter-price-hint")}</p>
          <p class="public-history-player-hint">{t("history-filter-player-hint")}</p>
          <div class="public-history-filter-actions">
            {/* 同步清除已应用的筛选条件（含世界，回到全部世界），并移除旧结果。 */}
            <button
              type="button"
              class="public-history-filter-actions__clear"
              onClick={() => props.onReset()}
            >
              {t("history-filter-clear")}
            </button>
            <HistoryApply />
          </div>
        </div>
      </Show>
    </form>
  );
}

/** 渲染一个年月组合筛选字段。 */
function HistoryDateFilter(props: {
  readonly fieldName: string;
  readonly label: string;
  readonly monthLabel: string;
  readonly yearLabel: string;
  readonly month: string;
  readonly year: string;
  readonly error: string | null;
  readonly setMonth: (month: string) => void;
  readonly setYear: (year: string) => void;
  readonly setFilterError: (error: HistoryFilterError | null) => void;
}): JSX.Element {
  const errorId = `public-history-${props.fieldName}-error`;
  return (
    <fieldset class="public-history-filter-cell">
      <legend>{props.label}</legend>
      <div class="public-history-compound public-history-date">
        <select
          name={`${props.fieldName}_month`}
          aria-describedby={props.error === null ? undefined : errorId}
          aria-label={props.monthLabel}
          aria-invalid={props.error !== null ? "true" : "false"}
          value={props.month}
          onChange={(event) => {
            props.setMonth(event.currentTarget.value);
            props.setFilterError(null);
          }}
        >
          <option value="">{t("history-filter-month")}</option>
          <For each={HISTORY_MONTHS} keyed={false}>
            {(monthNumber) => (
              <option value={monthNumber().toString().padStart(2, "0")}>
                {historyMonthLabel(monthNumber())}
              </option>
            )}
          </For>
        </select>
        <input
          name={`${props.fieldName}_year`}
          aria-describedby={props.error === null ? undefined : errorId}
          type="number"
          min="1"
          max="9999"
          inputmode="numeric"
          aria-label={props.yearLabel}
          aria-invalid={props.error !== null ? "true" : "false"}
          placeholder={t("history-filter-year")}
          value={props.year}
          onInput={(event) => {
            props.setYear(event.currentTarget.value);
            props.setFilterError(null);
          }}
        />
      </div>
      <Show when={props.error !== null}>
        <span id={errorId} class="public-history-field-error" role="alert">
          {props.error}
        </span>
      </Show>
    </fieldset>
  );
}

/** 渲染一个带 K/M 单位选择的价格字段。 */
function HistoryPriceFilter(props: {
  readonly fieldName: string;
  readonly label: string;
  readonly valueLabel: string;
  readonly unitLabel: string;
  readonly priceValue: string;
  readonly priceUnit: HistoryPriceUnit;
  readonly error: string | null;
  readonly setPriceValue: (price: string) => void;
  readonly setPriceUnit: (unit: HistoryPriceUnit) => void;
  readonly setFilterError: (error: HistoryFilterError | null) => void;
}): JSX.Element {
  const errorId = `public-history-${props.fieldName}-error`;
  return (
    <fieldset class="public-history-filter-cell">
      <legend>{props.label}</legend>
      <div class="public-history-compound public-history-price-control">
        <input
          name={props.fieldName}
          aria-describedby={props.error === null ? undefined : errorId}
          type="text"
          inputmode={props.priceUnit === "C" ? "numeric" : "decimal"}
          maxlength="32"
          aria-label={props.valueLabel}
          aria-invalid={props.error !== null ? "true" : "false"}
          value={props.priceValue}
          onInput={(event) => {
            props.setPriceValue(event.currentTarget.value);
            props.setFilterError(null);
          }}
        />
        <select
          name={`${props.fieldName}_unit`}
          aria-describedby={props.error === null ? undefined : errorId}
          aria-label={props.unitLabel}
          value={historyPriceUnitAsStr(props.priceUnit)}
          onChange={(event) => {
            props.setPriceUnit(historyPriceUnitFromStr(event.currentTarget.value));
            props.setFilterError(null);
          }}
        >
          <option value="C">C</option>
          <option value="K">K</option>
          <option value="M">M</option>
        </select>
      </div>
      <Show when={props.error !== null}>
        <span id={errorId} class="public-history-field-error" role="alert">
          {props.error}
        </span>
      </Show>
    </fieldset>
  );
}
