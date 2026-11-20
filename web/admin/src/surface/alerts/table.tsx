// 告警列表与单行操作：确认、带到期的静默、处置手册锚点。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { Time } from "@auction/web-shared/ui/components/time";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import type { AdminAlert, AdminAlertPage } from "../../api/alerts";
import { useAdminSessionHost } from "../auth/shell";
import { requirePendingCommands } from "../pending";
import type { AdminActionFeedback } from "../presentation";
import {
  type AlertCommand,
  type AlertSilenceHours,
  alertCommandAcknowledge,
  alertCommandEntity,
  alertCommandReason,
  alertCommandSilence,
  submitAlertCommand,
} from "./commands";
import { type AlertReasonDraft, AlertReasonInput, validAlertReasonLength } from "./reason";

/** 公开仓库中的处置手册；Admin 不复制手册正文，只链接到固定锚点。 */
const RUNBOOK_BASE_URL = "https://github.com/ngnlAYY/Auction/blob/main/ops/README.md";
/** 行内静默时长选项（小时），均在服务端 60 秒至 7 天的边界内。 */
const SILENCE_HOUR_OPTIONS: readonly AlertSilenceHours[] = [1, 8, 24, 72];
const DEFAULT_SILENCE_HOURS: AlertSilenceHours = 24;

export function AlertTable(props: {
  readonly page: AdminAlertPage;
  readonly reason: AlertReasonDraft;
  readonly notice: Cell<AdminActionFeedback | null>;
  readonly refresh: Cell<bigint>;
  readonly pageAlive: () => boolean;
}): JSX.Element {
  const host = useAdminSessionHost();
  const pending = requirePendingCommands();
  const focusReason = createCell(false);
  const onAction = (command: AlertCommand): void => {
    if (!validAlertReasonLength(alertCommandReason(command))) {
      props.reason.markInvalid();
      focusReason.set(true);
      return;
    }
    submitAlertCommand(host, pending, props.notice, props.refresh, props.pageAlive, command);
  };
  const hasAlerts = createValueMemo(() => props.page.alerts.length > 0);
  return (
    <Show
      when={hasAlerts()}
      fallback={<AsyncState kind="empty" title={t("alert-console-empty")} detail="" />}
    >
      <div class="admin-shell__table-wrap">
        <table
          class="admin-shell__table admin-alert-console__table"
          aria-label={t("page-alerts-title")}
        >
          <thead>
            <tr>
              <th scope="col">{t("alert-console-kind")}</th>
              <th scope="col">{t("alert-console-scope")}</th>
              <th scope="col">{t("alert-console-count")}</th>
              <th scope="col">{t("alert-console-first-seen")}</th>
              <th scope="col">{t("alert-console-last-seen")}</th>
              <th scope="col">{t("alert-console-state")}</th>
              <th scope="col">{t("alert-console-runbook")}</th>
              <th scope="col">{t("alert-console-actions")}</th>
            </tr>
          </thead>
          <tbody>
            <For each={props.page.alerts} keyed={(alert) => alert.id}>
              {(alert) => (
                <AlertRow alert={alert()} reason={props.reason.value} onAction={onAction} />
              )}
            </For>
          </tbody>
        </table>
        <AlertReasonInput draft={props.reason} focusRequested={focusReason} />
      </div>
    </Show>
  );
}

/** 单条告警；静默时长是本行草稿，点击时与共享备注一起冻结进命令负载。 */
function AlertRow(props: {
  readonly alert: AdminAlert;
  readonly reason: Cell<string>;
  readonly onAction: (command: AlertCommand) => void;
}): JSX.Element {
  const pending = requirePendingCommands();
  const silenceHours = createCell(DEFAULT_SILENCE_HOURS);
  // 同一告警最多保留一个已提交意图；结果未知时只能在未决区按原 command_id 重试。
  const blocked = (): boolean => pending.contains(alertCommandEntity(props.alert.id));
  const incident = createValueMemo(() =>
    props.alert.active ? present(props.alert.incident_generation) : null,
  );
  return (
    <tr>
      <td>
        <div class="admin-alert-console__kind">{props.alert.kind}</div>
        <span
          class={
            props.alert.severity === "urgent"
              ? "admin-dashboard-badge admin-dashboard-badge--error"
              : "admin-dashboard-badge admin-dashboard-badge--neutral"
          }
        >
          {severityLabel(props.alert)}
        </span>
      </td>
      <td class="admin-alert-console__scope">{`${props.alert.source} / ${props.alert.scope}`}</td>
      <td class="admin-alert-console__count">{props.alert.count}</td>
      <td>
        <Time value={props.alert.first_seen_at} />
      </td>
      <td>
        <Time value={props.alert.last_seen_at} />
      </td>
      <td>
        <div
          class={
            props.alert.active
              ? "admin-dashboard-badge admin-dashboard-badge--warn"
              : "admin-dashboard-badge admin-dashboard-badge--ok"
          }
        >
          {props.alert.active ? t("alert-console-active") : t("alert-console-resolved")}
        </div>
        <Show when={present(props.alert.resolved_at)}>
          {(resolvedAt) => (
            <div>
              <Time value={resolvedAt().value} />
            </div>
          )}
        </Show>
        <Show when={present(props.alert.acknowledged_at)}>
          {(acknowledgedAt) => (
            <div class="admin-alert-console__ack">
              {t("alert-console-acknowledged")} <Time value={acknowledgedAt().value} />
              <Show when={present(props.alert.acknowledged_by)}>
                {(actor) => (
                  <>
                    {" "}
                    <span>{actor().value}</span>
                  </>
                )}
              </Show>
            </div>
          )}
        </Show>
        <Show when={present(props.alert.silenced_until)}>
          {(silencedUntil) => (
            <div class="admin-alert-console__silence">
              {t("alert-console-silenced-until")} <Time value={silencedUntil().value} />
            </div>
          )}
        </Show>
      </td>
      <td>
        <a
          class="admin-alert-console__anchor"
          href={`${RUNBOOK_BASE_URL}#${props.alert.runbook_anchor}`}
          target="_blank"
          rel="noopener noreferrer"
        >
          {props.alert.runbook_anchor}
        </a>
      </td>
      <td>
        <Show when={incident()}>
          {(generation) => (
            <div class="admin-alert-console__action-group">
              <button
                class="admin-btn-secondary"
                type="button"
                disabled={blocked()}
                onClick={() => {
                  props.onAction(
                    alertCommandAcknowledge(props.alert.id, generation().value, props.reason()),
                  );
                }}
              >
                {t("alert-console-acknowledge")}
              </button>
              {/* 行内没有可见标签：可访问名称说明它选的是静默时长，旁边的按钮不能代为命名。 */}
              <select
                aria-label={t("alert-console-silence-duration")}
                value={String(silenceHours())}
                onChange={(event) => {
                  const hours = parseSilenceHours(event.currentTarget.value);
                  if (hours !== null) {
                    silenceHours.set(hours);
                  }
                }}
              >
                <option value="1">{t("alert-console-1h")}</option>
                <option value="8">{t("alert-console-8h")}</option>
                <option value="24">{t("alert-console-24h")}</option>
                <option value="72">{t("alert-console-72h")}</option>
              </select>
              <button
                class="admin-btn-secondary"
                type="button"
                disabled={blocked()}
                onClick={() => {
                  props.onAction(
                    alertCommandSilence(
                      props.alert.id,
                      generation().value,
                      silenceHours(),
                      props.reason(),
                    ),
                  );
                }}
              >
                {t("alert-console-silence")}
              </button>
            </div>
          )}
        </Show>
      </td>
    </tr>
  );
}

function severityLabel(alert: AdminAlert): string {
  return alert.severity === "digest"
    ? t("alert-console-severity-digest")
    : t("alert-console-severity-urgent");
}

/** `Option<T>` 的非空视图；避免把空字符串或数字 0 当成缺失值。 */
function present<T>(value: T | null): { readonly value: T } | null {
  return value === null ? null : { value };
}

/** 与 Rust `i64::from_str` 相同的十进制词法；只接受允许的静默时长，其余返回 null。 */
function parseSilenceHours(value: string): AlertSilenceHours | null {
  if (!/^[+-]?[0-9]+$/.test(value)) {
    return null;
  }
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed)) {
    return null;
  }
  return isSilenceHours(parsed) ? parsed : null;
}

function isSilenceHours(value: number): value is AlertSilenceHours {
  return SILENCE_HOUR_OPTIONS.some((hours) => hours === value);
}
