// 审计记录表格：当前页排序、行投影与手机端排序入口。

import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { compareText } from "@auction/web-shared/text";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { TableColumns } from "@auction/web-shared/ui/components/table-columns";
import { Time } from "@auction/web-shared/ui/components/time";
import { TruncatedText } from "@auction/web-shared/ui/components/truncated-text";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import type { AdminAuditPage, AdminAuditRecord } from "../../api/audit";
import { MobileSortControl, SortableTh, type SortDirection, sortRows } from "../sort";

/** 审计记录表格；排序只作用于服务端返回的当前页。 */
export function AuditTable(props: {
  readonly page: AdminAuditPage;
  /** 当前排序列（空串 = 服务端原始顺序，即发生时间倒序）与方向。 */
  readonly sort: string;
  readonly direction: SortDirection;
  readonly onSortChange: (value: [string, SortDirection]) => void;
}): JSX.Element {
  const empty = createValueMemo(() => props.page.records.length === 0);
  const records = createValueMemo(() => {
    const sorted = [...props.page.records];
    sortAuditRecords(sorted, props.sort, props.direction);
    return sorted;
  });
  const columns = createValueMemo((): readonly (readonly [string, string])[] => [
    ["at", t("admin-audit-at")],
    ["actor", t("admin-audit-actor")],
    ["action", t("admin-audit-action")],
    ["entity", t("admin-audit-entity")],
    ["source", t("admin-audit-source")],
    ["result", t("admin-audit-result")],
  ]);

  return (
    <>
      <MobileSortControl
        columns={columns()}
        sort={props.sort}
        direction={props.direction}
        onSortChange={props.onSortChange}
      />
      <div class="admin-shell__table-wrap admin-audit-table-wrap">
        <table class="admin-shell__table admin-audit-table" aria-label={t("page-audit-title")}>
          <TableColumns layout="admin-audit" widths={[17, 12, 18, 15, 6, 6, 10, 16]} />
          <thead>
            <tr>
              <SortableTh
                column="at"
                label={t("admin-audit-at")}
                active={props.sort}
                direction={props.direction}
                onSort={props.onSortChange}
                class="admin-audit-table__time"
              />
              <SortableTh
                column="actor"
                label={t("admin-audit-actor")}
                active={props.sort}
                direction={props.direction}
                onSort={props.onSortChange}
              />
              <SortableTh
                column="action"
                label={t("admin-audit-action")}
                active={props.sort}
                direction={props.direction}
                onSort={props.onSortChange}
              />
              <SortableTh
                column="entity"
                label={t("admin-audit-entity")}
                active={props.sort}
                direction={props.direction}
                onSort={props.onSortChange}
              />
              <SortableTh
                column="source"
                label={t("admin-audit-source")}
                active={props.sort}
                direction={props.direction}
                onSort={props.onSortChange}
              />
              <SortableTh
                column="result"
                label={t("admin-audit-result")}
                active={props.sort}
                direction={props.direction}
                onSort={props.onSortChange}
              />
              <th scope="col">{t("admin-audit-request-id")}</th>
              <th class="admin-audit-table__metadata" scope="col">
                {t("admin-audit-metadata")}
              </th>
            </tr>
          </thead>
          <tbody>
            <For each={records()} keyed={false}>
              {(record) => <AuditRow record={record()} />}
            </For>
          </tbody>
        </table>
      </div>
      <Show when={empty()}>
        <AsyncState kind="empty" title={t("admin-audit-empty")} detail="" />
      </Show>
    </>
  );
}

function AuditRow(props: { readonly record: AdminAuditRecord }): JSX.Element {
  const actorType = createValueMemo(() => auditActorTypeLabel(props.record.actor_type));
  const source = createValueMemo(() => auditSourceLabel(props.record.source));
  const result = createValueMemo(() => auditResultLabel(props.record.result));
  const resultClass = createValueMemo(() => {
    switch (props.record.result) {
      case "success":
        return "admin-audit-result admin-audit-result--success";
      case "rejected":
        return "admin-audit-result admin-audit-result--rejected";
      case "failure":
        return "admin-audit-result admin-audit-result--failure";
      default:
        return "admin-audit-result admin-audit-result--neutral";
    }
  });
  const requestId = createValueMemo(() => props.record.request_id ?? t("value-empty"));
  const metadata = createValueMemo(() => metadataText(props.record.metadata));
  const actorIdentity = createValueMemo(() => `${props.record.actor_id}(${actorType()})`);
  const entityIdentity = createValueMemo(
    () => `${props.record.entity_id}(${props.record.entity_type})`,
  );

  return (
    <tr class="admin-audit__row">
      <td class="admin-audit-table__time" data-label={t("admin-audit-at")}>
        <Time value={props.record.occurred_at} />
      </td>
      <td data-label={t("admin-audit-actor")}>
        <TruncatedText
          value={actorIdentity()}
          className="admin-audit-table__value admin-audit-identity"
        />
      </td>
      <td data-label={t("admin-audit-action")}>
        <TruncatedText value={props.record.action} className="admin-audit-table__value" />
      </td>
      <td data-label={t("admin-audit-entity")}>
        <TruncatedText
          value={entityIdentity()}
          className="admin-audit-table__value admin-audit-identity admin-audit-code"
        />
      </td>
      <td data-label={t("admin-audit-source")}>
        <span class="admin-audit-source">{source()}</span>
      </td>
      <td data-label={t("admin-audit-result")}>
        <span class={resultClass()}>{result()}</span>
      </td>
      <td data-label={t("admin-audit-request-id")}>
        <TruncatedText value={requestId()} className="admin-audit-table__value admin-audit-code" />
      </td>
      <td
        class="admin-audit-table__metadata admin-audit-metadata"
        data-label={t("admin-audit-metadata")}
      >
        <TruncatedText
          value={metadata()}
          size="wide"
          className="admin-audit-table__value admin-audit-code"
        />
      </td>
    </tr>
  );
}

function metadataText(value: unknown): string {
  try {
    return rustJsonText(value);
  } catch {
    return "{}";
  }
}

/** serde_json 默认以 BTreeMap 保存对象，`Value::to_string()` 的键按 Unicode 码点顺序输出。 */
function rustJsonText(value: unknown): string {
  if (Array.isArray(value)) {
    return `[${value.map(rustJsonText).join(",")}]`;
  }
  if (isJsonObject(value)) {
    const fields = Object.keys(value)
      .sort(compareText)
      .map((key) => `${JSON.stringify(key)}:${rustJsonText(value[key])}`);
    return `{${fields.join(",")}}`;
  }
  const text = JSON.stringify(value);
  return typeof text === "string" ? text : "null";
}

function isJsonObject(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
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

/** 对当前页审计记录排序；`column` 为空或不在允许集合时保持服务端原始顺序。 */
function sortAuditRecords(
  records: AdminAuditRecord[],
  column: string,
  direction: SortDirection,
): void {
  sortRows(records, column, direction, (left, right, current) => {
    switch (current) {
      case "at":
        return compareText(left.occurred_at, right.occurred_at);
      case "actor":
        return compareText(left.actor_id, right.actor_id);
      case "action":
        return compareText(left.action, right.action);
      case "entity":
        return compareText(left.entity_id, right.entity_id);
      case "source":
        return compareText(left.source, right.source);
      case "result":
        return compareText(left.result, right.result);
      default:
        return 0;
    }
  });
}
