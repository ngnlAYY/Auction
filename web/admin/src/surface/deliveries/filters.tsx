// Delivery 检索筛选器组件：状态 + 收件人、参考编号、模板、类别、世界与时间。

import { useFocusElementByIdAfterRender } from "@auction/web-shared/platform/a11y";
import { createCell } from "@auction/web-shared/reactive/cell";
import { structuralEquals } from "@auction/web-shared/reactive/equality";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { type Result, unreachable } from "@auction/web-shared/result";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, For, Show, untrack } from "solid-js";

import { parseStrictRfc3339, trimRustWhitespace } from "../../api/session-time";
import { datetimeLocalValue, type LocalTimeError, parseLocalDatetime } from "../../api/validation";
import { adminWorldLabel } from "../presentation";
import { DELIVERY_STATUS_VALUES, deliveryCategoryLabel, deliveryStatusLabel } from "./labels";

/** 提交前的筛选草稿；只在「应用筛选」时写入地址栏，输入过程不触发网络请求。 */
export interface DeliveryFiltersDraft {
  readonly recipient: string;
  readonly reference: string;
  readonly template: string;
  readonly category: string;
  readonly world: string;
  readonly createdFrom: string;
  readonly createdTo: string;
}

const DELIVERY_CATEGORY_VALUES: readonly string[] = [
  "outbid",
  "account",
  "intake",
  "settlement",
  "equipment",
  "operations",
  "manual",
];
const CREATED_FROM_FIELD = "delivery-created-from";
const CREATED_TO_FIELD = "delivery-created-to";

/** 只转换显示值，草稿在实际编辑前保留地址栏的精确时刻。 */
function timestampToLocalInput(value: string): string {
  if (parseStrictRfc3339(value) !== null) {
    return datetimeLocalValue(value) ?? "";
  }
  return value;
}

function timeBound(value: string): Result<string, LocalTimeError> {
  if (value === "" || parseStrictRfc3339(value) !== null) {
    return { ok: true, value };
  }
  // 无效边界不能降级成无界查询。
  return parseLocalDatetime(value);
}

function timeBoundError(issue: LocalTimeError): string {
  switch (issue) {
    case "invalid":
      return t("admin-auctions-invalid-local-time");
    case "nonexistent":
      return t("admin-auctions-nonexistent-local-time");
    case "ambiguous":
      return t("admin-auctions-ambiguous-local-time");
    default:
      return unreachable(issue);
  }
}

/** 投递检索筛选器的属性；状态立即写回地址栏，其余字段作为草稿在提交时写回。 */
export interface DeliveryFiltersProps {
  /** 当前状态筛选（地址栏恢复后的值）。 */
  readonly status: string;
  /** 收件人筛选草稿。 */
  readonly recipient: string;
  /** 参考编号筛选草稿。 */
  readonly reference: string;
  /** 模板筛选草稿。 */
  readonly template: string;
  /** 类别筛选草稿；空串表示全部类别。 */
  readonly category: string;
  /** 世界筛选草稿；空串表示全部世界。 */
  readonly world: string;
  /** 创建时间下限草稿，保留地址栏的精确时刻直到编辑。 */
  readonly createdFrom: string;
  /** 创建时间上限草稿，保留地址栏的精确时刻直到编辑。 */
  readonly createdTo: string;
  /** 状态选择变化时立即通知页面。 */
  readonly onStatusChange: (status: string) => void;
  /** 提交筛选时携带校验并裁剪后的草稿。 */
  readonly onFiltersChange: (filters: DeliveryFiltersDraft) => void;
}

/** 渲染投递检索筛选器；输入只改草稿，提交时才校验时间边界并通知页面。 */
export function DeliveryFilters(props: DeliveryFiltersProps): JSX.Element {
  const applied = createValueMemo<DeliveryFiltersDraft>(() => ({
    recipient: props.recipient,
    reference: props.reference,
    template: props.template,
    category: props.category,
    world: props.world,
    createdFrom: props.createdFrom,
    createdTo: props.createdTo,
  }));
  // 对应 Rust 的首次取值：只取一次初值，后续由两段式 effect 同步已应用筛选。
  const draft = createCell<DeliveryFiltersDraft>(untrack(applied));
  const fromError = createCell<LocalTimeError | null>(null);
  const toError = createCell<LocalTimeError | null>(null);
  const focusElementById = useFocusElementByIdAfterRender();
  // 已应用筛选随前进/后退或提交变化时整体重建草稿；否则旧草稿会在下一次“应用”时把过期
  // 条件写回地址栏。已应用筛选不变时保留操作员尚未提交的编辑。
  createEffect(applied, (next) => {
    if (!structuralEquals(draft.peek(), next)) {
      draft.set(next);
    }
    fromError.set(null);
    toError.set(null);
  });
  const submit = (event: SubmitEvent): void => {
    event.preventDefault();
    const current = draft();
    const from = timeBound(current.createdFrom);
    const to = timeBound(current.createdTo);
    fromError.set(from.ok ? null : from.error);
    toError.set(to.ok ? null : to.error);
    if (!from.ok || !to.ok) {
      focusElementById(from.ok ? CREATED_TO_FIELD : CREATED_FROM_FIELD);
      return;
    }
    props.onFiltersChange({
      recipient: trimRustWhitespace(current.recipient),
      reference: trimRustWhitespace(current.reference),
      template: trimRustWhitespace(current.template),
      category: current.category,
      world: current.world,
      createdFrom: from.value,
      createdTo: to.value,
    });
  };
  return (
    <div class="admin-delivery-filter">
      <label for="delivery-status">
        {t("delivery-filter")}
        <select
          id="delivery-status"
          value={props.status}
          onChange={(event) => props.onStatusChange(event.currentTarget.value)}
        >
          <option value="">{t("delivery-all")}</option>
          <For each={DELIVERY_STATUS_VALUES} keyed={false}>
            {(status) => <option value={status()}>{deliveryStatusLabel(status())}</option>}
          </For>
        </select>
      </label>
      <form class="admin-delivery-search" onSubmit={submit}>
        <label for="delivery-recipient">
          <span>{t("delivery-search-recipient")}</span>
          <input
            id="delivery-recipient"
            type="text"
            maxlength="256"
            value={draft().recipient}
            onInput={(event) =>
              draft.update((current) => ({ ...current, recipient: event.currentTarget.value }))
            }
          />
        </label>
        <label for="delivery-reference">
          <span>{t("delivery-search-reference")}</span>
          <input
            id="delivery-reference"
            type="text"
            maxlength="36"
            value={draft().reference}
            onInput={(event) =>
              draft.update((current) => ({ ...current, reference: event.currentTarget.value }))
            }
          />
        </label>
        <label for="delivery-template">
          <span>{t("delivery-template")}</span>
          <input
            id="delivery-template"
            type="text"
            maxlength="64"
            value={draft().template}
            onInput={(event) =>
              draft.update((current) => ({ ...current, template: event.currentTarget.value }))
            }
          />
        </label>
        <label for="delivery-category">
          <span>{t("delivery-category")}</span>
          <select
            id="delivery-category"
            value={draft().category}
            onChange={(event) =>
              draft.update((current) => ({ ...current, category: event.currentTarget.value }))
            }
          >
            <option value="">{t("delivery-all")}</option>
            <For each={DELIVERY_CATEGORY_VALUES} keyed={false}>
              {(value) => <option value={value()}>{deliveryCategoryLabel(value())}</option>}
            </For>
          </select>
        </label>
        <label for="delivery-world">
          <span>{t("delivery-world")}</span>
          <select
            id="delivery-world"
            value={draft().world}
            onChange={(event) =>
              draft.update((current) => ({ ...current, world: event.currentTarget.value }))
            }
          >
            <option value="">{t("delivery-all")}</option>
            <option value="main">{adminWorldLabel("main")}</option>
            <option value="isekai">{adminWorldLabel("isekai")}</option>
          </select>
        </label>
        <label for={CREATED_FROM_FIELD}>
          <span>{t("delivery-created-from")}</span>
          <input
            id={CREATED_FROM_FIELD}
            type="datetime-local"
            aria-invalid={fromError() === null ? "false" : "true"}
            aria-describedby={fromError() === null ? undefined : "delivery-created-from-error"}
            value={timestampToLocalInput(draft().createdFrom)}
            onInput={(event) => {
              const value = event.currentTarget.value;
              if (fromError.peek() !== null && timeBound(value).ok) {
                fromError.set(null);
              }
              draft.update((current) => ({ ...current, createdFrom: value }));
            }}
          />
          <Show when={fromError()}>
            {(issue) => (
              <small id="delivery-created-from-error" class="admin-field-error">
                {timeBoundError(issue())}
              </small>
            )}
          </Show>
        </label>
        <label for={CREATED_TO_FIELD}>
          <span>{t("delivery-created-to")}</span>
          <input
            id={CREATED_TO_FIELD}
            type="datetime-local"
            aria-invalid={toError() === null ? "false" : "true"}
            aria-describedby={toError() === null ? undefined : "delivery-created-to-error"}
            value={timestampToLocalInput(draft().createdTo)}
            onInput={(event) => {
              const value = event.currentTarget.value;
              if (toError.peek() !== null && timeBound(value).ok) {
                toError.set(null);
              }
              draft.update((current) => ({ ...current, createdTo: value }));
            }}
          />
          <Show when={toError()}>
            {(issue) => (
              <small id="delivery-created-to-error" class="admin-field-error">
                {timeBoundError(issue())}
              </small>
            )}
          </Show>
        </label>
        <button class="admin-btn-primary" type="submit">
          {t("delivery-search-apply")}
        </button>
      </form>
    </div>
  );
}
