// 审计筛选表单：草稿留在表单内，只有九项都有效时才提交到地址栏。

import { useFocusElementByIdAfterRender } from "@auction/web-shared/platform/a11y";
import type { Cell } from "@auction/web-shared/reactive/cell";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { unreachable } from "@auction/web-shared/result";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { type Accessor, createEffect, For, Show, untrack } from "solid-js";

import { type AdminAuditFilter, adminAuditFilterDefault } from "../../api/audit";
import { QueryFilterInput, QueryFilterSelect } from "../realtime/filters";
import {
  AUDIT_ADVANCED_FIELDS,
  AUDIT_PRIMARY_FIELDS,
  type AuditField,
  type AuditFilterState,
  auditFieldChoices,
  auditFieldIndex,
  auditFieldNames,
  auditFieldNormalize,
  auditFilterStateDefault,
  auditFilterStateEquals,
  auditFilterStateFromFilter,
  auditFilterStatePrepare,
  auditFilterStateWithValue,
  auditFilterValueErrorValues,
  auditFilterValueInput,
  auditFilterValueText,
  auditTextFilter,
  auditTimeInput,
} from "./filter-state";

/** 已应用筛选变化时刷新草稿，但翻页、排序和同一筛选的重读不能抹去未提交输入。 */
export function AuditFilters(props: {
  readonly applied: AuditFilterState;
  readonly onSubmit: (filter: AdminAuditFilter) => void;
}): JSX.Element {
  const applied = createValueMemo(() => props.applied);
  const [draft, submitted] = useAuditFilterDraft(applied);
  const focusField = useFocusElementByIdAfterRender();

  const advancedActive = createValueMemo(() =>
    draft()
      .values.slice(AUDIT_PRIMARY_FIELDS.length)
      .some((value) => {
        const text = auditFilterValueText(value);
        return text === null || text !== "";
      }),
  );

  return (
    <form
      class="admin-audit-filter"
      autocomplete="off"
      onSubmit={(event) => {
        event.preventDefault();
        const next = auditFilterStatePrepare(draft.peek());
        if (next.ok) {
          submitted.set(false);
          draft.set(auditFilterStateFromFilter(next.value));
          props.onSubmit(next.value);
          return;
        }
        submitted.set(true);
        focusField(auditFieldNames(next.error)[1]);
      }}
    >
      <p class="admin-audit-filter__hint">{t("admin-audit-filter-hint")}</p>
      <div class="admin-audit-filter__primary">
        <For each={AUDIT_PRIMARY_FIELDS} keyed={(field) => field}>
          {(field) => <AuditFilterControl field={field()} draft={draft} showErrors={submitted()} />}
        </For>
      </div>
      <details class="admin-audit-filter__advanced" open={advancedActive()}>
        <summary>{t("admin-audit-advanced-filters")}</summary>
        <div class="admin-audit-filter__advanced-grid">
          <For each={AUDIT_ADVANCED_FIELDS} keyed={(field) => field}>
            {(field) => (
              <AuditFilterControl field={field()} draft={draft} showErrors={submitted()} />
            )}
          </For>
        </div>
      </details>
      <div class="admin-audit-filter__actions">
        <button class="admin-btn-primary" type="submit">
          {t("admin-audit-load")}
        </button>
        <button
          type="button"
          onClick={() => {
            draft.set(auditFilterStateDefault());
            submitted.set(false);
            props.onSubmit(adminAuditFilterDefault());
          }}
        >
          {t("admin-audit-clear")}
        </button>
      </div>
    </form>
  );
}

function useAuditFilterDraft(
  applied: Accessor<AuditFilterState>,
): readonly [Cell<AuditFilterState>, Cell<boolean>] {
  const draft = createCell<AuditFilterState>(untrack(applied));
  const submitted = createCell(false);
  // 草稿只订阅已应用的筛选，排序、翻页和重读不能抹去未提交输入。
  createEffect(applied, (next) => {
    if (!auditFilterStateEquals(draft.peek(), next)) {
      draft.set(next);
    }
    submitted.set(false);
  });
  return [draft, submitted];
}

function AuditFilterControl(props: {
  readonly field: AuditField;
  readonly draft: Cell<AuditFilterState>;
  readonly showErrors: boolean;
}): JSX.Element {
  // 字段与草稿下标在同一次响应式读取中计算；在组件体顶层读取 props 会冻结下标。
  const value = createValueMemo(() => {
    const index = auditFieldIndex(props.field);
    return props.draft().values[index] ?? auditTextFilter("");
  });
  const name = createValueMemo(() => auditFieldNames(props.field)[0]);
  const id = createValueMemo(() => auditFieldNames(props.field)[1]);
  const label = createValueMemo(() => auditFieldLabel(props.field));
  const choices = createValueMemo(() => {
    const options = auditFieldChoices(props.field);
    if (options === null) {
      return null;
    }
    const values: [string, string, boolean][] = [["", t("admin-audit-all"), false]];
    for (const key of options) {
      values.push([key, auditFieldChoiceLabel(props.field, key), false]);
    }
    return values;
  });
  const inputValue = createValueMemo(() => {
    const raw = auditFilterValueInput(value());
    return props.field === "from" || props.field === "to" ? auditTimeInput(raw) : raw;
  });
  const invalidDraft = createValueMemo(() => {
    const text = auditFilterValueText(value());
    return props.showErrors && text !== null && auditFieldNormalize(props.field, text) === null;
  });
  const errorValues = createValueMemo(() => auditFilterValueErrorValues(value(), invalidDraft()));
  const onInput = (next: string): void => {
    props.draft.set(
      auditFilterStateWithValue(props.draft.peek(), props.field, auditTextFilter(next)),
    );
  };

  return (
    <>
      <Show when={choices()}>
        {(options) => (
          <QueryFilterSelect
            id={id()}
            name={name()}
            label={label()}
            value={value()}
            choices={options()}
            onChange={onInput}
          />
        )}
      </Show>
      <Show when={choices() === null}>
        <QueryFilterInput
          id={id()}
          name={name()}
          label={label()}
          inputType={props.field === "from" || props.field === "to" ? "datetime-local" : "search"}
          value={inputValue()}
          errorValues={errorValues()}
          onInput={onInput}
        />
      </Show>
    </>
  );
}

function auditFieldLabel(field: AuditField): string {
  switch (field) {
    case "actor":
      return t("admin-audit-actor-id");
    case "entity":
      return t("admin-audit-entity-identity");
    case "action":
      return t("admin-audit-action");
    case "request_id":
      return t("admin-audit-request-id");
    case "actor_type":
      return t("admin-audit-actor-type");
    case "source":
      return t("admin-audit-source");
    case "result":
      return t("admin-audit-result");
    case "from":
      return t("admin-audit-from");
    case "to":
      return t("admin-audit-to");
    default:
      return unreachable(field);
  }
}

function auditFieldChoiceLabel(field: AuditField, value: string): string {
  switch (field) {
    case "actor_type":
      return auditActorTypeLabel(value);
    case "source":
      return auditSourceLabel(value);
    case "result":
      return auditResultLabel(value);
    case "actor":
    case "entity":
    case "action":
    case "request_id":
    case "from":
    case "to":
      return value;
    default:
      return unreachable(field);
  }
}

function auditActorTypeLabel(value: string): string {
  switch (value) {
    case "public_user":
      return t("admin-audit-actor-public-user");
    case "admin":
      return t("admin-audit-actor-admin");
    case "worker":
      return t("admin-audit-actor-worker");
    case "system":
      return t("admin-audit-actor-system");
    default:
      return value;
  }
}

function auditSourceLabel(value: string): string {
  switch (value) {
    case "public":
      return t("admin-audit-source-public");
    case "admin":
      return t("admin-audit-source-admin");
    case "cli":
      return t("admin-audit-source-cli");
    case "worker":
      return t("admin-audit-source-worker");
    case "system":
      return t("admin-audit-source-system");
    default:
      return value;
  }
}

function auditResultLabel(value: string): string {
  switch (value) {
    case "success":
      return t("admin-audit-result-success");
    case "rejected":
      return t("admin-audit-result-rejected");
    case "failure":
      return t("admin-audit-result-failure");
    default:
      return value;
  }
}
