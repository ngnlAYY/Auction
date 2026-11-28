// Admin 隔离详情：观察列表、重检、六种解除分支回执与安全错误。

import type {
  QuarantineDecisionKind,
  QuarantineDetail,
  QuarantineObservation,
  QuarantineObservationItemKind,
  QuarantineReason,
  QuarantineRecheckReceipt,
  QuarantineReleaseReceipt,
  QuarantineState,
} from "@auction/contracts/dto/admin/consignment/quarantine";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { Link, type RoutePageProps } from "@auction/web-shared/router";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { Time } from "@auction/web-shared/ui/components/time";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, For, Match, Show, Switch, untrack } from "solid-js";

import {
  fetchAdminQuarantine,
  prepareAdminQuarantineRecheck,
  prepareAdminQuarantineRelease,
} from "../../api/consignment/quarantine";
import { useAdminSessionHost } from "../auth/shell";
import { PendingCommandPanel } from "../pending/panel";
import { advanceCommandReload, createCommandReload, usePendingCommands } from "../pending/sources";
import { consignmentErrorMessage } from "./presentation";

const DECISION_KINDS = [
  "accept",
  "return",
  "archive_empty",
  "acquire_return",
  "auction_return_case",
  "auction_recovery_return",
] as const satisfies readonly QuarantineDecisionKind[];

const STATE_KEYS = {
  active: "consignment-quarantine-state-active",
  released: "consignment-quarantine-state-released",
} satisfies Record<QuarantineState, string>;

const REASON_KEYS = {
  duplicate_raw_eid: "consignment-quarantine-reason-duplicate_raw_eid",
  canonical_alias_ambiguous: "consignment-quarantine-reason-canonical_alias_ambiguous",
  unknown_attachment: "consignment-quarantine-reason-unknown_attachment",
  list_detail_conflict: "consignment-quarantine-reason-list_detail_conflict",
  asset_count_unknown: "consignment-quarantine-reason-asset_count_unknown",
  same_name_changed_uid: "consignment-quarantine-reason-same_name_changed_uid",
  identity_unproven: "consignment-quarantine-reason-identity_unproven",
  amount_unproven: "consignment-quarantine-reason-amount_unproven",
  return_unproven: "consignment-quarantine-reason-return_unproven",
  take_unproven: "consignment-quarantine-reason-take_unproven",
  unsafe_return_shape: "consignment-quarantine-reason-unsafe_return_shape",
  suspected_system_rts: "consignment-quarantine-reason-suspected_system_rts",
} satisfies Record<QuarantineReason, string>;

const ITEM_KEYS = {
  equipment: "consignment-quarantine-item-equipment",
  credits: "consignment-quarantine-item-credits",
  cod: "consignment-quarantine-item-cod",
  unknown: "consignment-quarantine-item-unknown",
} satisfies Record<QuarantineObservationItemKind, string>;

const DECISION_KEYS = {
  accept: "consignment-quarantine-decision-accept",
  return: "consignment-quarantine-decision-return",
  archive_empty: "consignment-quarantine-decision-archive_empty",
  acquire_return: "consignment-quarantine-decision-acquire_return",
  auction_return_case: "consignment-quarantine-decision-auction_return_case",
  auction_recovery_return: "consignment-quarantine-decision-auction_recovery_return",
} satisfies Record<QuarantineDecisionKind, string>;

interface Feedback {
  readonly kind: "error" | "recheck" | "release";
  readonly code?: string;
  readonly recheck?: QuarantineRecheckReceipt;
  readonly release?: QuarantineReleaseReceipt;
}

/** 路由页内切换隔离时卸载旧观察、草稿和回执。 */
export function quarantinePage(props: RoutePageProps): JSX.Element {
  return (
    <For each={[props.params.quarantine_id ?? ""]} keyed={(id) => id}>
      {(id) => <Quarantine untrackedId={untrack(id)} />}
    </For>
  );
}

function Quarantine(props: { readonly untrackedId: string }): JSX.Element {
  const host = useAdminSessionHost();
  const quarantineId = untrack(() => props.untrackedId);
  const refresh = createCommandReload();
  const entity = `quarantine:${quarantineId}`;
  const commands = usePendingCommands(refresh, (candidate) => candidate === entity);
  const feedback = createCell<Feedback | null>(null);
  const preparing = createCell(false);
  const detail = createTaskResource(
    () => refresh(),
    async (generation, context) => ({
      generation,
      result: await fetchAdminQuarantine(host, quarantineId, context.signal),
    }),
  );
  const loaded = createValueMemo(() => {
    const read = detail();
    return read?.generation === refresh() ? read.result : null;
  });
  const projection = createValueMemo(() => {
    const result = loaded();
    return result?.ok ? result.value : null;
  });
  const loadError = createValueMemo(() => {
    const result = loaded();
    return result !== null && !result.ok ? result.error : null;
  });

  const submitRecheck = async () => {
    const current = projection();
    if (current === null || preparing() || commands.contains(entity) || commands.anyBusy()) {
      return;
    }
    preparing.set(true);
    feedback.set(null);
    try {
      const operation = await prepareAdminQuarantineRecheck(host, quarantineId, current.version);
      // 准备期间刷新可能已替换投影；旧版本的命令不能覆盖新的权威状态。
      const latest = projection();
      if (latest === null || latest.version !== current.version) return;
      commands.submit(
        entity,
        t("consignment-quarantine-recheck"),
        operation,
        (receipt) => {
          feedback.set({ kind: "recheck", recheck: receipt });
          advanceCommandReload(refresh);
        },
        (code) => feedback.set({ kind: "error", code }),
      );
    } finally {
      preparing.set(false);
    }
  };

  const submitRelease = async (observationId: string, kind: QuarantineDecisionKind) => {
    const current = projection();
    if (current === null || preparing() || commands.contains(entity) || commands.anyBusy()) {
      return;
    }
    preparing.set(true);
    feedback.set(null);
    try {
      const operation = await prepareAdminQuarantineRelease(
        host,
        quarantineId,
        current.version,
        observationId,
        kind,
      );
      // 版本或观察在准备期间变化时，旧详情构造的命令不再成立。
      const latest = projection();
      if (
        latest === null ||
        latest.version !== current.version ||
        !latest.observations.some((item) => item.observation_id === observationId)
      ) {
        return;
      }
      commands.submit(
        entity,
        t("consignment-quarantine-release"),
        operation,
        (receipt) => feedback.set({ kind: "release", release: receipt }),
        (code) => feedback.set({ kind: "error", code }),
      );
    } finally {
      preparing.set(false);
    }
  };

  return (
    <section class="admin-consignment-detail" aria-labelledby="admin-quarantine-title">
      <div class="admin-consignment-detail__header">
        <h1 id="admin-quarantine-title" tabindex="-1">
          {t("consignment-quarantine-title")}
        </h1>
        <Link href="/consignment">{t("consignment-admin-title")}</Link>
        <Show when={projection()}>
          <button type="button" onClick={() => advanceCommandReload(refresh)}>
            {t("admin-record-refresh")}
          </button>
        </Show>
      </div>
      <Show when={feedback()}>{(notice) => <FeedbackPanel feedback={notice()} />}</Show>
      <PendingCommandPanel owner={commands} />
      <Switch
        fallback={<AsyncState kind="loading" title={t("dashboard-legacy-loading")} detail="" />}
      >
        <Match when={loadError()}>
          {(error) => (
            <AsyncState
              kind="error"
              title={quarantineErrorMessage(error())}
              detail=""
              retryLabel={t("admin-retry")}
              onRetry={() => advanceCommandReload(refresh)}
            />
          )}
        </Match>
        <Match when={projection()}>
          {(current) => (
            <>
              <QuarantineSummary detail={current()} />
              <ObservationList detail={current()} />
              <QuarantineActions
                detail={current()}
                disabled={preparing() || commands.contains(entity) || commands.anyBusy()}
                onRecheck={submitRecheck}
                onRelease={submitRelease}
              />
            </>
          )}
        </Match>
      </Switch>
    </section>
  );
}

function QuarantineSummary(props: { readonly detail: QuarantineDetail }): JSX.Element {
  return (
    <dl class="admin-consignment-facts">
      <div>
        <dt>{t("consignment-quarantine-id")}</dt>
        <dd>
          <code>{props.detail.quarantine_id}</code>
        </dd>
      </div>
      <div>
        <dt>{t("consignment-quarantine-state")}</dt>
        <dd>{t(STATE_KEYS[props.detail.state])}</dd>
      </div>
      <div>
        <dt>{t("consignment-quarantine-reason")}</dt>
        <dd>{t(REASON_KEYS[props.detail.reason])}</dd>
      </div>
      <div>
        <dt>{t("consignment-quarantine-world")}</dt>
        <dd>
          {t(
            props.detail.world === "main"
              ? "admin-runtime-world-main"
              : "admin-runtime-world-isekai",
          )}
        </dd>
      </div>
      <div>
        <dt>{t("consignment-quarantine-mid")}</dt>
        <dd>
          <code>{props.detail.external_mid}</code>
        </dd>
      </div>
    </dl>
  );
}

function ObservationList(props: { readonly detail: QuarantineDetail }): JSX.Element {
  return (
    <section aria-labelledby="admin-quarantine-observations">
      <h2 id="admin-quarantine-observations">{t("consignment-quarantine-observations")}</h2>
      <Show when={props.detail.observation_total > props.detail.observations.length}>
        <p role="status">
          {t("consignment-quarantine-observation-total", {
            count: String(props.detail.observations.length),
            total: String(props.detail.observation_total),
          })}
        </p>
      </Show>
      <ul>
        <For each={props.detail.observations} keyed={(item) => item.observation_id}>
          {(observation) => <Observation observation={observation()} />}
        </For>
      </ul>
    </section>
  );
}

function Observation(props: { readonly observation: QuarantineObservation }): JSX.Element {
  return (
    <li>
      <dl class="admin-consignment-facts">
        <div>
          <dt>{t("consignment-quarantine-observation")}</dt>
          <dd>
            <code>{props.observation.observation_id}</code>
          </dd>
        </div>
        <div>
          <dt>{t("consignment-quarantine-observed-at")}</dt>
          <dd>
            <Time value={props.observation.observed_at} />
          </dd>
        </div>
        <div>
          <dt>{t("consignment-quarantine-parser")}</dt>
          <dd>{props.observation.parser_version}</dd>
        </div>
        <div>
          <dt>{t("consignment-quarantine-digest")}</dt>
          <dd>
            <code>{props.observation.digest}</code>
          </dd>
        </div>
        <div>
          <dt>{t("consignment-quarantine-attachments")}</dt>
          <dd>{props.observation.attachment_count}</dd>
        </div>
        <div>
          <dt>{t("consignment-quarantine-returnable")}</dt>
          <dd>{boolLabel(props.observation.returnable)}</dd>
        </div>
        <div>
          <dt>{t("consignment-quarantine-takeable")}</dt>
          <dd>{boolLabel(props.observation.takeable)}</dd>
        </div>
        <div>
          <dt>{t("consignment-quarantine-credits")}</dt>
          <dd>{props.observation.credits ?? "—"}</dd>
        </div>
        <div>
          <dt>{t("consignment-quarantine-cod")}</dt>
          <dd>{props.observation.cod ?? "—"}</dd>
        </div>
      </dl>
      <ul>
        <For each={props.observation.items}>
          {(item) => (
            <li>
              <span>{t(ITEM_KEYS[item.kind])}</span>
              <span>{item.external_equipment_id ?? item.amount ?? "—"}</span>
            </li>
          )}
        </For>
      </ul>
    </li>
  );
}

function QuarantineActions(props: {
  readonly detail: QuarantineDetail;
  readonly disabled: boolean;
  readonly onRecheck: () => void;
  readonly onRelease: (observationId: string, kind: QuarantineDecisionKind) => void;
}): JSX.Element {
  const observation = createCell(props.detail.observations[0]?.observation_id ?? "");
  const kind = createCell<QuarantineDecisionKind>("accept");
  // 详情刷新后旧观察可能已不在列表；回落到首个观察，避免用旧 ID 配新版本提交。
  createEffect(
    () => props.detail.observations,
    (observations) => {
      if (!observations.some((item) => item.observation_id === observation.peek())) {
        observation.set(observations[0]?.observation_id ?? "");
      }
    },
  );
  const allowedRecheck = () => props.detail.allowed_actions.includes("recheck");
  const allowedRelease = () => props.detail.allowed_actions.includes("release");
  return (
    <section class="admin-consignment-actions" aria-labelledby="admin-quarantine-actions">
      <h2 id="admin-quarantine-actions">{t("consignment-quarantine-actions")}</h2>
      <Show when={allowedRecheck()}>
        <button type="button" disabled={props.disabled} onClick={props.onRecheck}>
          {t("consignment-quarantine-recheck")}
        </button>
      </Show>
      <Show when={allowedRelease()}>
        <form
          class="admin-consignment-actions__form"
          onSubmit={(event) => {
            event.preventDefault();
            const selected = props.detail.observations.find(
              (item) => item.observation_id === observation(),
            );
            if (props.disabled || selected === undefined) return;
            props.onRelease(selected.observation_id, kind());
          }}
        >
          <label>
            {t("consignment-quarantine-release-observation")}
            <select
              value={observation()}
              onChange={(event) => observation.set(event.currentTarget.value)}
            >
              <For each={props.detail.observations}>
                {(item) => (
                  <option value={item.observation_id}>
                    {item.observation_id} · {item.observed_at}
                  </option>
                )}
              </For>
            </select>
          </label>
          <label>
            {t("consignment-quarantine-release-kind")}
            <select
              value={kind()}
              onChange={(event) => kind.set(event.currentTarget.value as QuarantineDecisionKind)}
            >
              <For each={DECISION_KINDS}>
                {(value) => <option value={value}>{t(DECISION_KEYS[value])}</option>}
              </For>
            </select>
          </label>
          <button type="submit" disabled={props.disabled}>
            {t("consignment-quarantine-release")}
          </button>
        </form>
      </Show>
    </section>
  );
}

function FeedbackPanel(props: { readonly feedback: Feedback }): JSX.Element {
  return (
    <Switch>
      <Match when={props.feedback.kind === "error"}>
        <p role="alert" class="action-feedback action-feedback--error">
          {quarantineErrorMessage(props.feedback.code ?? "unknown_error")}
        </p>
      </Match>
      <Match when={props.feedback.recheck}>
        {(receipt) => (
          <p role="status">
            {t("consignment-quarantine-recheck-queued", { work_id: receipt().work_id })}
          </p>
        )}
      </Match>
      <Match when={props.feedback.release}>
        {(receipt) => <ReleaseReceiptPanel receipt={receipt()} />}
      </Match>
    </Switch>
  );
}

function ReleaseReceiptPanel(props: { readonly receipt: QuarantineReleaseReceipt }): JSX.Element {
  return (
    <section aria-label={t("consignment-quarantine-release-result")}>
      <p role="status">{t("consignment-quarantine-released")}</p>
      <dl class="admin-consignment-facts">
        <div>
          <dt>{t("consignment-quarantine-release-result")}</dt>
          <dd>{t(DECISION_KEYS[props.receipt.decision_kind])}</dd>
        </div>
        <div>
          <dt>{t("consignment-quarantine-release-id")}</dt>
          <dd>
            <code>{props.receipt.release_id}</code>
          </dd>
        </div>
        <div>
          <dt>{t("consignment-quarantine-decision-id")}</dt>
          <dd>
            <code>{props.receipt.decision_id}</code>
          </dd>
        </div>
        <Show when={props.receipt.decision_kind === "accept"}>
          <div>
            <dt>{t("consignment-quarantine-release-accept")}</dt>
            <dd>
              <For
                each={props.receipt.decision_kind === "accept" ? props.receipt.listing_changes : []}
              >
                {(listing) => (
                  <p>
                    <code>{listing.listing_id}</code> · {listing.listing_version} · {listing.state}
                  </p>
                )}
              </For>
            </dd>
          </div>
        </Show>
        <Show when={props.receipt.decision_kind === "acquire_return"}>
          <div>
            <dt>{t("consignment-quarantine-intake")}</dt>
            <dd>
              <code>
                {props.receipt.decision_kind === "acquire_return"
                  ? props.receipt.return_intake_id
                  : ""}
              </code>
            </dd>
          </div>
          <div>
            <dt>{t("consignment-quarantine-round")}</dt>
            <dd>
              {props.receipt.decision_kind === "acquire_return" ? props.receipt.original_round : ""}
            </dd>
          </div>
          <div>
            <dt>{t("consignment-quarantine-take-work")}</dt>
            <dd>
              <code>
                {props.receipt.decision_kind === "acquire_return" ? props.receipt.take_work_id : ""}
              </code>
            </dd>
          </div>
          <div>
            <dt>{t("consignment-quarantine-take-delivery")}</dt>
            <dd>
              <code>
                {props.receipt.decision_kind === "acquire_return"
                  ? props.receipt.take_delivery_id
                  : ""}
              </code>
            </dd>
          </div>
        </Show>
        <Show
          when={
            props.receipt.decision_kind === "auction_return_case" ||
            props.receipt.decision_kind === "auction_recovery_return"
          }
        >
          <div>
            <dt>{t("consignment-quarantine-return-mail")}</dt>
            <dd>
              <code>
                {props.receipt.decision_kind === "auction_return_case" ||
                props.receipt.decision_kind === "auction_recovery_return"
                  ? props.receipt.return_mail_id
                  : ""}
              </code>
            </dd>
          </div>
          <div>
            <dt>{t("consignment-quarantine-obligation")}</dt>
            <dd>
              <code>
                {props.receipt.decision_kind === "auction_return_case" ||
                props.receipt.decision_kind === "auction_recovery_return"
                  ? (props.receipt.obligation_id ?? "—")
                  : ""}
              </code>
            </dd>
          </div>
        </Show>
        <Show when={props.receipt.decision_kind === "auction_return_case"}>
          <div>
            <dt>{t("consignment-quarantine-evidence")}</dt>
            <dd>
              <code>
                {props.receipt.decision_kind === "auction_return_case"
                  ? props.receipt.return_evidence_id
                  : ""}
              </code>
            </dd>
          </div>
        </Show>
        <Show when={props.receipt.decision_kind === "auction_recovery_return"}>
          <div>
            <dt>{t("consignment-quarantine-recovery")}</dt>
            <dd>
              <code>
                {props.receipt.decision_kind === "auction_recovery_return"
                  ? props.receipt.recovery_id
                  : ""}
              </code>
            </dd>
          </div>
          <div>
            <dt>{t("consignment-quarantine-handoff")}</dt>
            <dd>
              <code>
                {props.receipt.decision_kind === "auction_recovery_return"
                  ? props.receipt.recovery_handoff_id
                  : ""}
              </code>
            </dd>
          </div>
        </Show>
      </dl>
    </section>
  );
}

function boolLabel(value: boolean | null): string {
  if (value === null) return "—";
  return value ? t("delivery-detail-yes") : t("delivery-detail-no");
}

function quarantineErrorMessage(code: string): string {
  switch (code) {
    case "listing_not_found":
      return t("consignment-quarantine-not-found");
    case "invalid_request":
      return t("consignment-quarantine-invalid-request");
    case "stale_version":
      return t("consignment-stale-version");
    case "source_mismatch":
      return t("consignment-quarantine-source-mismatch");
    case "evidence_unproven":
      return t("consignment-quarantine-evidence-unproven");
    case "evidence_conflict":
      return t("consignment-quarantine-evidence-conflict");
    case "disposition_conflict":
      return t("consignment-quarantine-disposition-conflict");
    case "action_not_allowed":
      return t("consignment-quarantine-action-not-allowed");
    case "hold_mismatch":
      return t("consignment-quarantine-hold-mismatch");
    case "outcome_unknown":
      return t("consignment-quarantine-outcome-unknown");
    case "command_id_conflict":
      return t("consignment-command-conflict");
    default:
      return consignmentErrorMessage(code);
  }
}
