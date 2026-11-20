// 外部账号分区：进程内忙碌/暂停、请求门、维护与论坛安全摘要。
// 只渲染 Worker 快照与既有 owner 投影；子来源不可用时显示明确的不可用文案，不用零值或
// 空列表冒充“没有暂停”或“凭据齐全”。

import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Match, Show, Switch } from "solid-js";

import type { AdminDashboardSnapshot, AdminExternalAccountsData } from "../../../api/dashboard";
import { credentialKindLabel, credentialWorldLabel } from "../../credentials/labels";
import { formatAdminTime } from "../../presentation";
import type { DashboardTone } from "./derived";
import { Badge, Card } from "./widgets";

/** 外部账号面板的输入；面板保持挂载，随快照更新。 */
export interface ExternalAccountsPanelProps {
  readonly snapshot: AdminDashboardSnapshot;
}

/** 外部账号分区；子分区不可用时只显示明确的不可用提示。 */
export function ExternalAccountsPanel(props: ExternalAccountsPanelProps): JSX.Element {
  const accounts = createValueMemo(() => props.snapshot.external_accounts.data);
  return (
    <section class="admin-dashboard-panel" aria-labelledby="dashboard-external-accounts-title">
      <h2 id="dashboard-external-accounts-title">{t("dashboard-external-accounts-title")}</h2>
      <Show
        when={accounts()}
        fallback={<p class="admin-dashboard-status">{t("dashboard-external-unavailable")}</p>}
      >
        {(current) => (
          <>
            <MailSection snapshot={props.snapshot} accounts={current()} />
            <PollerSection accounts={current()} />
            <ForumSection accounts={current()} />
            <MaintenanceSection accounts={current()} />
            <CredentialsSection accounts={current()} />
          </>
        )}
      </Show>
    </section>
  );
}

interface MailSectionProps {
  readonly snapshot: AdminDashboardSnapshot;
  readonly accounts: AdminExternalAccountsData;
}

function MailSection(props: MailSectionProps): JSX.Element {
  const health = createValueMemo(() => healthBadge(props.snapshot.external_accounts.status));
  const mail = createValueMemo(() => props.accounts.mail);
  const busyAge = createValueMemo(() => mail().busy_age_seconds);
  const mailTone = createValueMemo<DashboardTone>(() => (mail().busy ? "warn" : "ok"));
  const mailLabel = createValueMemo(() =>
    mail().busy ? t("dashboard-external-mail-busy") : t("dashboard-external-mail-idle"),
  );
  return (
    <>
      <h3>{t("dashboard-external-health")}</h3>
      <div class="admin-dashboard-cards">
        <Card
          label={t("dashboard-external-health")}
          value={<Badge state={health()[0]} value={health()[1]} />}
        />
        <Card
          label={t("dashboard-external-mail-state")}
          value={<Badge state={mailTone()} value={mailLabel()} />}
        />
        <Show when={busyAge() !== null}>
          <Card
            label={t("dashboard-external-mail-busy-age")}
            value={t("dashboard-legacy-duration-seconds", { seconds: busyAge() ?? 0 })}
          />
        </Show>
        <Card
          label={t("dashboard-external-mail-gate-waits")}
          value={mail().gate_wait_count.toString()}
        />
        <Card
          label={t("dashboard-external-mail-gate-seconds")}
          value={t("dashboard-legacy-duration-seconds", {
            seconds: mail().gate_wait_seconds_total,
          })}
        />
        <Card
          label={t("dashboard-external-mail-requests")}
          value={mail().request_count.toString()}
        />
      </div>
      <details class="admin-dashboard-details">
        <summary>{t("dashboard-metrics-details")}</summary>
        <div class="admin-dashboard-cards admin-dashboard-cards--timestamps">
          <Card
            label={t("dashboard-external-mail-last-request")}
            value={observed(mail().last_request_at)}
          />
          <Card
            label={t("dashboard-external-mail-last-success")}
            value={observed(mail().last_success_at)}
          />
          <Card
            label={t("dashboard-external-mail-last-failure")}
            value={observed(mail().last_failure_at)}
          />
        </div>
      </details>
    </>
  );
}

interface PollerSectionProps {
  readonly accounts: AdminExternalAccountsData;
}

function PollerSection(props: PollerSectionProps): JSX.Element {
  const pollers = createValueMemo(() => props.accounts.mail.pollers);
  return (
    <>
      <h3>{t("dashboard-external-pollers")}</h3>
      <Switch
        fallback={
          <div class="admin-dashboard-cards">
            <For each={pollers().items} keyed={(poller) => poller.mailbox_key}>
              {(poller) => (
                <div class="admin-dashboard-card">
                  <div class="admin-dashboard-value">
                    <Badge
                      state={poller().paused ? "warn" : "ok"}
                      value={
                        poller().paused
                          ? t("dashboard-external-poller-paused")
                          : t("dashboard-external-poller-running")
                      }
                    />
                  </div>
                  <div class="admin-dashboard-label">{poller().mailbox_key}</div>
                  <Show when={poller().paused_since !== null}>
                    <div class="admin-dashboard-label">
                      {t("dashboard-external-poller-paused-since", {
                        at: formatAdminTime(poller().paused_since ?? ""),
                      })}
                    </div>
                  </Show>
                  <Show when={poller().consecutive_failures > 0}>
                    <div class="admin-dashboard-label">
                      {t("dashboard-external-poller-failures", {
                        count: poller().consecutive_failures,
                      })}
                    </div>
                  </Show>
                </div>
              )}
            </For>
          </div>
        }
      >
        <Match when={pollers().status !== "available"}>
          <p class="admin-dashboard-status">{t("dashboard-external-projection-unavailable")}</p>
        </Match>
        <Match when={pollers().items.length === 0}>
          <p class="admin-dashboard-status">{t("dashboard-legacy-no-data")}</p>
        </Match>
      </Switch>
    </>
  );
}

interface ForumSectionProps {
  readonly accounts: AdminExternalAccountsData;
}

function ForumSection(props: ForumSectionProps): JSX.Element {
  const forum = createValueMemo(() => props.accounts.forum);
  return (
    <details class="admin-dashboard-details">
      <summary>{t("dashboard-external-forum-state")}</summary>
      <div class="admin-dashboard-cards admin-dashboard-cards--timestamps">
        <Card
          label={t("dashboard-external-forum-last-request")}
          value={observed(forum().last_request_at)}
        />
        <Card
          label={t("dashboard-external-forum-last-success")}
          value={observed(forum().last_success_at)}
        />
        <Card
          label={t("dashboard-external-forum-last-failure")}
          value={observed(forum().last_failure_at)}
        />
        <Card
          label={t("dashboard-external-forum-write-unknown")}
          value={forum().write_unknown_count.toString()}
        />
      </div>
    </details>
  );
}

interface MaintenanceSectionProps {
  readonly accounts: AdminExternalAccountsData;
}

function MaintenanceSection(props: MaintenanceSectionProps): JSX.Element {
  const maintenance = createValueMemo(() => props.accounts.maintenance);
  const tone = createValueMemo<DashboardTone>(() => maintenanceBadge(maintenance().state)[0]);
  const label = createValueMemo(() => maintenanceBadge(maintenance().state)[1]);
  const runningAge = createValueMemo(() => maintenance().running_age_seconds);
  return (
    <>
      <h3>{t("dashboard-external-maintenance")}</h3>
      <div class="admin-dashboard-cards">
        <Card
          label={t("dashboard-external-maintenance")}
          value={<Badge state={tone()} value={label()} />}
        />
        <Card
          label={t("dashboard-external-maintenance-last-start")}
          value={observed(maintenance().last_started_at)}
        />
        <Card
          label={t("dashboard-external-maintenance-last-success")}
          value={observed(maintenance().last_success_at)}
        />
        <Card
          label={t("dashboard-external-maintenance-last-failure")}
          value={observed(maintenance().last_failure_at)}
        />
        <Show when={runningAge() !== null}>
          <Card
            label={t("dashboard-external-maintenance-running-age")}
            value={t("dashboard-legacy-duration-seconds", { seconds: runningAge() ?? 0 })}
          />
        </Show>
      </div>
    </>
  );
}

interface CredentialsSectionProps {
  readonly accounts: AdminExternalAccountsData;
}

function CredentialsSection(props: CredentialsSectionProps): JSX.Element {
  const credentials = createValueMemo(() => props.accounts.credentials);
  return (
    <>
      <h3>{t("dashboard-external-credentials")}</h3>
      <Switch
        fallback={
          <div class="admin-dashboard-cards">
            <For
              each={credentials().items}
              keyed={(credential) => `${credential.world}-${credential.kind}`}
            >
              {(credential) => (
                <div class="admin-dashboard-card">
                  <div class="admin-dashboard-value">
                    <Badge
                      state={credentialTone(credential().available, credential().invalid)}
                      value={credential().available.toString()}
                    />
                  </div>
                  <div class="admin-dashboard-label">
                    {t("dashboard-external-credential-row", {
                      world: credentialWorldLabel(credential().world),
                      kind: credentialKindLabel(credential().kind),
                      available: credential().available,
                      invalid: credential().invalid,
                    })}
                  </div>
                </div>
              )}
            </For>
          </div>
        }
      >
        <Match when={credentials().status !== "available"}>
          <p class="admin-dashboard-status">{t("dashboard-external-projection-unavailable")}</p>
        </Match>
        <Match when={credentials().items.length === 0}>
          <p class="admin-dashboard-status">{t("dashboard-legacy-no-data")}</p>
        </Match>
      </Switch>
    </>
  );
}

function healthBadge(status: string): readonly [DashboardTone, string] {
  if (status === "ok") {
    return ["ok", t("dashboard-external-status-ok")];
  }
  if (status === "degraded") {
    return ["warn", t("dashboard-external-status-degraded")];
  }
  return ["neutral", t("dashboard-external-status-unknown")];
}

function maintenanceBadge(state: string): readonly [DashboardTone, string] {
  switch (state) {
    case "running":
      return ["warn", t("dashboard-external-maintenance-running")];
    case "succeeded":
      return ["ok", t("dashboard-external-maintenance-succeeded")];
    case "failed":
      return ["error", t("dashboard-external-maintenance-failed")];
    case "canceled":
      return ["neutral", t("dashboard-external-maintenance-canceled")];
    default:
      return ["neutral", t("dashboard-external-maintenance-idle")];
  }
}

/** 存在失效凭据即标错，其次按是否有可用凭据区分正常与警告。 */
function credentialTone(available: number, invalid: number): DashboardTone {
  if (invalid > 0) {
    return "error";
  }
  return available > 0 ? "ok" : "warn";
}

/** 与其他页面同一时区与语言格式；服务端原始 RFC3339 UTC 文本不直接展示。 */
function observed(value: string | null): string {
  return value === null ? t("dashboard-external-never") : formatAdminTime(value);
}
