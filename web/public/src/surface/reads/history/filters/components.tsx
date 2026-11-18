// 价格历史表单的字段、交互时校验与错误关联。

import { sleep } from "@auction/web-shared/platform/timer";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import { t } from "@auction/web-shared/ui/i18n";
import { escapedSingleLineInput } from "@auction/web-shared/ui/validation";
import type { JSX } from "@solidjs/web";
import { For, onSettled, Show } from "solid-js";

import { worldLabel } from "../../../board/world";
import { HistoryApply } from "../../../presentation/mobile";
import type { WorldSwitchOption } from "../../../query";
import type { HistoryFilterField, HistoryPriceUnit } from "./model";
import { historyPriceUnitAsStr, historyPriceUnitFromStr } from "./model";
import type { HistoryDraft, useHistoryFilters } from "./state";
import { historyFilterErrorMessage, historyMonthLabel } from "./validation";

const HISTORY_MONTHS = Object.freeze(Array.from({ length: 12 }, (_, index) => index + 1));

interface FieldFeedback {
  readonly id: string;
  readonly field: HistoryFilterField;
  invalid(): "true" | "false";
  description(): string | undefined;
  message(): string | null;
  original(): string | null;
  edit(patch: Partial<HistoryDraft>): void;
  blur(): void;
}

/** 历史筛选表单；深链错误随原文显示，交互后再标记输入控件。 */
export function PriceHistoryFilters(props: {
  readonly filters: ReturnType<typeof useHistoryFilters>;
  readonly worldOptions: readonly WorldSwitchOption[];
  readonly activeFilterCount: number;
}): JSX.Element {
  const filters = props.filters;
  const draft = filters.draft;
  const worldFilterVisible = createValueMemo(
    () =>
      filters.errors().world !== undefined ||
      props.worldOptions.filter((option) => !option.disabled).length > 1 ||
      props.worldOptions.some((option) => option.disabled),
  );
  const announcer = createCell("");
  const announcementTasks = createTaskScope();
  let announcement: ReturnType<typeof announcementTasks.spawn> | null = null;
  const clearAnnouncement = () => {
    announcement?.cancel();
    announcer.set("");
  };
  const field = (key: HistoryFilterField, label: () => string): FieldFeedback => {
    const id = `public-history-${key}-error`;
    const error = () => filters.errors()[key]?.code ?? null;
    return {
      id,
      field: key,
      invalid: () => (error() !== null && filters.interacted().includes(key) ? "true" : "false"),
      description: () => (error() !== null ? id : undefined),
      message: () => historyFilterErrorMessage(error()),
      original: () => {
        const input = filters.errors()[key]?.input;
        if (input == null) return null;
        if (typeof input !== "string") {
          return input.map((value) => JSON.stringify(value)).join(" / ");
        }
        // 原生搜索框会移除换行，年月选择器也不能显示精确到日的区间端点。
        return key === "world" || key === "from" || key === "to"
          ? JSON.stringify(input)
          : escapedSingleLineInput(input);
      },
      edit(patch) {
        clearAnnouncement();
        filters.updateField(key, patch);
      },
      blur() {
        const message = historyFilterErrorMessage(filters.blur(key));
        if (message !== null) {
          clearAnnouncement();
          // 等下一字段获得焦点后再读错误，避免打断读屏对新字段的提示。
          announcement = announcementTasks.spawn(async (context) => {
            await sleep(500, context.signal);
            if (context.alive()) {
              announcer.set(`${label()} ${message}`);
            }
          });
        }
      },
    };
  };

  const name = field("name", () => t("history-filter-name"));
  const world = field("world", () => t("history-filter-world"));
  const from = field("from", () => t("history-filter-from"));
  const to = field("to", () => t("history-filter-to"));
  const minimum = field("minPrice", () => t("history-filter-min-price"));
  const maximum = field("maxPrice", () => t("history-filter-max-price"));
  const buyer = field("buyer", () => t("history-filter-buyer"));
  const seller = field("seller", () => t("history-filter-seller"));

  let settingsToggle: HTMLButtonElement | undefined;
  const closeSettings = () => {
    filters.settingsOpen.set(false);
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
      novalidate
      onSubmit={(event) => {
        event.preventDefault();
        clearAnnouncement();
        const form = event.currentTarget;
        const firstError = filters.apply();
        if (firstError !== null) {
          onSettled(() => {
            form.querySelector<HTMLElement>(`[data-history-field="${firstError}"]`)?.focus();
          });
        }
      }}
    >
      <div class="public-history-primary-filters">
        <label for="public-history-name">
          <span>{t("history-filter-name")}</span>
          <input
            id="public-history-name"
            data-history-field="name"
            name="name"
            type="search"
            enterkeyhint="search"
            autocapitalize="none"
            spellcheck="false"
            value={draft().name}
            placeholder={t("history-filter-name-placeholder")}
            aria-invalid={name.invalid()}
            aria-describedby={
              name.message() === null
                ? "public-history-name-hint"
                : `public-history-name-hint ${name.id}`
            }
            onInput={(event) => name.edit({ name: event.currentTarget.value })}
            onBlur={name.blur}
          />
        </label>
        <div class="public-history-filter-toolbar">
          <button
            type="button"
            class="public-history-settings-toggle"
            aria-controls="public-history-settings"
            aria-expanded={filters.settingsOpen() ? "true" : "false"}
            ref={(element) => {
              settingsToggle = element;
            }}
            onClick={() => filters.settingsOpen.update((open) => !open)}
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
      <HistoryFieldError feedback={name} />
      <p class="ui-pager__sr-only" role="status" aria-live="polite">
        {announcer()}
      </p>
      <Show when={filters.settingsOpen()}>
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
                        data-history-field={option().disabled ? undefined : "world"}
                        class="public-auction-filter__tab"
                        aria-pressed={draft().world === option().value ? "true" : "false"}
                        aria-describedby={world.description()}
                        disabled={option().disabled}
                        onClick={() => world.edit({ world: option().value })}
                      >
                        {option().value === "all"
                          ? t("auction-history-world-all")
                          : worldLabel(option().value)}
                      </button>
                    )}
                  </For>
                </div>
                <HistoryFieldError feedback={world} />
              </fieldset>
            </Show>
            <HistoryDateFilter
              feedback={from}
              label={t("history-filter-from")}
              monthLabel={t("history-filter-from-month")}
              yearLabel={t("history-filter-from-year")}
              month={draft().fromMonth}
              year={draft().fromYear}
              setMonth={(fromMonth) => from.edit({ fromMonth })}
              setYear={(fromYear) => from.edit({ fromYear })}
            />
            <HistoryDateFilter
              feedback={to}
              label={t("history-filter-to")}
              monthLabel={t("history-filter-to-month")}
              yearLabel={t("history-filter-to-year")}
              month={draft().toMonth}
              year={draft().toYear}
              setMonth={(toMonth) => to.edit({ toMonth })}
              setYear={(toYear) => to.edit({ toYear })}
            />
            <HistoryPriceFilter
              feedback={minimum}
              fieldName="minimum_price"
              label={t("history-filter-min-price")}
              valueLabel={t("history-filter-min-price-value")}
              unitLabel={t("history-filter-min-price-unit")}
              priceValue={draft().minimumPrice}
              priceUnit={draft().minimumPriceUnit}
              setPriceValue={(minimumPrice) => minimum.edit({ minimumPrice })}
              setPriceUnit={(minimumPriceUnit) => minimum.edit({ minimumPriceUnit })}
            />
            <HistoryPriceFilter
              feedback={maximum}
              fieldName="maximum_price"
              label={t("history-filter-max-price")}
              valueLabel={t("history-filter-max-price-value")}
              unitLabel={t("history-filter-max-price-unit")}
              priceValue={draft().maximumPrice}
              priceUnit={draft().maximumPriceUnit}
              setPriceValue={(maximumPrice) => maximum.edit({ maximumPrice })}
              setPriceUnit={(maximumPriceUnit) => maximum.edit({ maximumPriceUnit })}
            />
            <HistoryPlayerFilter
              feedback={buyer}
              label={t("history-filter-buyer")}
              value={draft().buyer}
              setValue={(value) => buyer.edit({ buyer: value })}
            />
            <HistoryPlayerFilter
              feedback={seller}
              label={t("history-filter-seller")}
              value={draft().seller}
              setValue={(value) => seller.edit({ seller: value })}
            />
          </div>
          <p class="public-history-price-hint">{t("history-filter-price-hint")}</p>
          <p id="public-history-player-hint" class="public-history-player-hint">
            {t("history-filter-player-hint")}
          </p>
          <div class="public-history-filter-actions">
            <button
              type="button"
              class="public-history-filter-actions__clear"
              onClick={() => {
                clearAnnouncement();
                filters.reset();
              }}
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

function HistoryFieldError(props: { readonly feedback: FieldFeedback }): JSX.Element {
  return (
    <Show when={props.feedback.message() !== null}>
      <p id={props.feedback.id} class="public-history-field-error">
        {props.feedback.message()}
        <Show when={props.feedback.original()}>
          {(raw) => (
            <>
              {" "}
              <bdi>{raw()}</bdi>
            </>
          )}
        </Show>
      </p>
    </Show>
  );
}

function HistoryPlayerFilter(props: {
  readonly feedback: FieldFeedback;
  readonly label: string;
  readonly value: string;
  readonly setValue: (value: string) => void;
}): JSX.Element {
  const id = `public-history-${props.feedback.field}`;
  return (
    <div class="public-history-filter-cell">
      <label for={id}>{props.label}</label>
      <input
        id={id}
        data-history-field={props.feedback.field}
        name={props.feedback.field}
        type="search"
        enterkeyhint="search"
        autocapitalize="none"
        spellcheck="false"
        value={props.value}
        aria-invalid={props.feedback.invalid()}
        aria-describedby={
          props.feedback.message() === null
            ? "public-history-player-hint"
            : `public-history-player-hint ${props.feedback.id}`
        }
        onInput={(event) => props.setValue(event.currentTarget.value)}
        onBlur={() => props.feedback.blur()}
      />
      <HistoryFieldError feedback={props.feedback} />
    </div>
  );
}

function HistoryDateFilter(props: {
  readonly feedback: FieldFeedback;
  readonly label: string;
  readonly monthLabel: string;
  readonly yearLabel: string;
  readonly month: string;
  readonly year: string;
  readonly setMonth: (month: string) => void;
  readonly setYear: (year: string) => void;
}): JSX.Element {
  return (
    <fieldset class="public-history-filter-cell">
      <legend>{props.label}</legend>
      <div class="public-history-compound public-history-date">
        <select
          name={`${props.feedback.field}_month`}
          aria-describedby={props.feedback.description()}
          aria-label={props.monthLabel}
          aria-invalid={props.feedback.invalid()}
          value={props.month}
          onChange={(event) => props.setMonth(event.currentTarget.value)}
          onBlur={() => props.feedback.blur()}
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
          name={`${props.feedback.field}_year`}
          data-history-field={props.feedback.field}
          aria-describedby={props.feedback.description()}
          type="text"
          inputmode="numeric"
          aria-label={props.yearLabel}
          aria-invalid={props.feedback.invalid()}
          placeholder={t("history-filter-year")}
          value={props.year}
          onInput={(event) => props.setYear(event.currentTarget.value)}
          onBlur={() => props.feedback.blur()}
        />
      </div>
      <HistoryFieldError feedback={props.feedback} />
    </fieldset>
  );
}

function HistoryPriceFilter(props: {
  readonly feedback: FieldFeedback;
  readonly fieldName: string;
  readonly label: string;
  readonly valueLabel: string;
  readonly unitLabel: string;
  readonly priceValue: string;
  readonly priceUnit: HistoryPriceUnit;
  readonly setPriceValue: (price: string) => void;
  readonly setPriceUnit: (unit: HistoryPriceUnit) => void;
}): JSX.Element {
  return (
    <fieldset class="public-history-filter-cell">
      <legend>{props.label}</legend>
      <div class="public-history-compound public-history-price-control">
        <input
          name={props.fieldName}
          data-history-field={props.feedback.field}
          aria-describedby={props.feedback.description()}
          type="text"
          inputmode={props.priceUnit === "C" ? "numeric" : "decimal"}
          aria-label={props.valueLabel}
          aria-invalid={props.feedback.invalid()}
          value={props.priceValue}
          onInput={(event) => props.setPriceValue(event.currentTarget.value)}
          onBlur={() => props.feedback.blur()}
        />
        <select
          name={`${props.fieldName}_unit`}
          aria-describedby={props.feedback.description()}
          aria-label={props.unitLabel}
          value={historyPriceUnitAsStr(props.priceUnit)}
          onChange={(event) =>
            props.setPriceUnit(historyPriceUnitFromStr(event.currentTarget.value))
          }
          onBlur={() => props.feedback.blur()}
        >
          <option value="C">C</option>
          <option value="K">K</option>
          <option value="M">M</option>
        </select>
      </div>
      <HistoryFieldError feedback={props.feedback} />
    </fieldset>
  );
}
