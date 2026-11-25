// Admin 损失案件页：按 position 查看案件与全部人工结果，追加复核中或已办结。
// 本页只记录状态与说明，没有付款、退款或发行能力；未知写只能原样重试。

import type {
  AdminLossCase,
  AdminLossOutcome,
  LossCaseOutcomeKind,
  LossCaseOutcomeReceipt,
  LossCaseState,
} from "@auction/contracts/dto/admin/consignment/loss";
import { ConsignmentUuidSchema } from "@auction/contracts/dto/public/consignment/scalar";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { Link, type RoutePageProps } from "@auction/web-shared/router";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { Time } from "@auction/web-shared/ui/components/time";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Match, Show, Switch, untrack } from "solid-js";
import { is } from "valibot";

import { delistReasonError } from "../../api/consignment/delist";
import { fetchAdminLossCase, prepareAdminLossOutcome } from "../../api/consignment/loss";
import { adminSessionWritesAllowed } from "../../api/session";
import { useAdminSessionHost } from "../auth/shell";
import { PendingCommandPanel } from "../pending/panel";
import { advanceCommandReload, createCommandReload, usePendingCommands } from "../pending/sources";
import { consignmentErrorMessage } from "./presentation";

const STATE_KEYS = {
  opened: "consignment-loss-state-opened",
  reviewing: "consignment-loss-state-reviewing",
  closed: "consignment-loss-state-closed",
} satisfies Record<LossCaseState, string>;

type Feedback =
  | { readonly kind: "error"; readonly code: string }
  | { readonly kind: "recorded"; readonly receipt: LossCaseOutcomeReceipt };

/** 路由页内切换 position 时卸载旧案件、草稿和回执。 */
export function lossCasePage(props: RoutePageProps): JSX.Element {
  return (
    <For each={[props.params.position_id ?? ""]} keyed={(id) => id}>
      {(id) => <LossCase untrackedId={untrack(id)} />}
    </For>
  );
}

function LossCase(props: { readonly untrackedId: string }): JSX.Element {
  const host = useAdminSessionHost();
  const positionId = untrack(() => props.untrackedId);
  const refresh = createCommandReload();
  const entity = `loss-case:${positionId}`;
  const commands = usePendingCommands(refresh, (candidate) => candidate === entity);
  const feedback = createCell<Feedback | null>(null);
  const preparing = createCell(false);
  const outcome = createCell<LossCaseOutcomeKind>("reviewing");
  const basisId = createCell("");
  const note = createCell("");
  const problem = createCell<"basis" | "required" | "invalid" | null>(null);
  const read = createTaskResource(
    () => refresh(),
    async (generation, context) => ({
      generation,
      result: await fetchAdminLossCase(host, positionId, context.signal),
    }),
  );
  const loaded = createValueMemo(() => {
    const current = read();
    return current?.generation === refresh() ? current.result : null;
  });
  const projection = createValueMemo(() => {
    const result = loaded();
    return result?.ok ? result.value : null;
  });
  const loadError = createValueMemo(() => {
    const result = loaded();
    return result !== null && !result.ok ? result.error : null;
  });
  const canSubmit = (): boolean =>
    projection() !== null &&
    projection()?.state !== "closed" &&
    adminSessionWritesAllowed(host) &&
    !commands.contains(entity) &&
    !commands.anyBusy() &&
    !preparing();

  const submit = async (event: SubmitEvent): Promise<void> => {
    event.preventDefault();
    const current = projection();
    if (current === null || !canSubmit()) return;
    // 不变量 1、6、7：先校验依据与说明；提交后正文只保留在会话宿主固定的命令里。
    const basis = basisId.peek().trim();
    if (!is(ConsignmentUuidSchema, basis)) {
      problem.set("basis");
      return;
    }
    const explanation = note.peek();
    const noteProblem = delistReasonError(explanation);
    problem.set(noteProblem);
    if (noteProblem !== null) return;

    preparing.set(true);
    feedback.set(null);
    try {
      const operation = prepareAdminLossOutcome(host, positionId, {
        caseId: current.case_id,
        expectedCaseVersion: current.case_version,
        outcome: outcome.peek(),
        basisId: basis,
        privateNote: explanation,
      });
      const submitted = commands.submit(
        entity,
        t("consignment-loss-record"),
        operation,
        (receipt) => {
          feedback.set({ kind: "recorded", receipt });
          advanceCommandReload(refresh);
        },
        (code) => {
          // 版本落后由会话宿主了结命令后推进读取世代，页面重读后按新版本重新确认。
          if (!commands.contains(entity)) feedback.set({ kind: "error", code });
        },
      );
      if (submitted) {
        basisId.set("");
        note.set("");
      }
    } finally {
      preparing.set(false);
    }
  };

  return (
    <section class="admin-consignment-detail" aria-labelledby="admin-loss-title">
      <div class="admin-consignment-detail__header">
        <h1 id="admin-loss-title" tabindex="-1">
          {t("consignment-loss-title")}
        </h1>
        <Link href="/consignment">{t("consignment-admin-title")}</Link>
        <Show when={projection()}>
          <button type="button" onClick={() => advanceCommandReload(refresh)}>
            {t("admin-record-refresh")}
          </button>
        </Show>
      </div>
      <p>{t("consignment-loss-no-money")}</p>
      <Show when={feedback()}>{(notice) => <FeedbackPanel feedback={notice()} />}</Show>
      <PendingCommandPanel owner={commands} />
      <Switch
        fallback={<AsyncState kind="loading" title={t("dashboard-legacy-loading")} detail="" />}
      >
        <Match when={loadError()}>
          {(error) => (
            <AsyncState
              kind="error"
              title={consignmentErrorMessage(error())}
              detail=""
              retryLabel={t("admin-retry")}
              onRetry={() => advanceCommandReload(refresh)}
            />
          )}
        </Match>
        <Match when={projection()}>
          {(current) => (
            <>
              <CaseSummary current={current()} />
              <OutcomeList current={current()} />
              <Show
                when={current().state !== "closed"}
                fallback={<p role="status">{t("consignment-loss-closed-note")}</p>}
              >
                <form class="admin-consignment-actions__form" novalidate onSubmit={submit}>
                  <h2>{t("consignment-loss-record")}</h2>
                  <label for="loss-outcome">{t("consignment-loss-outcome-field")}</label>
                  <select
                    id="loss-outcome"
                    value={outcome()}
                    onChange={(event) =>
                      outcome.set(event.currentTarget.value === "closed" ? "closed" : "reviewing")
                    }
                  >
                    <option value="reviewing">{t(STATE_KEYS.reviewing)}</option>
                    <option value="closed">{t(STATE_KEYS.closed)}</option>
                  </select>
                  <label for="loss-basis">{t("consignment-loss-basis-field")}</label>
                  <input
                    id="loss-basis"
                    type="text"
                    autocomplete="off"
                    spellcheck={false}
                    value={basisId()}
                    aria-invalid={problem() === "basis" ? "true" : "false"}
                    onInput={(event) => {
                      basisId.set(event.currentTarget.value);
                      problem.set(null);
                    }}
                  />
                  <label for="loss-note">{t("consignment-loss-note-field")}</label>
                  <textarea
                    id="loss-note"
                    required
                    autocomplete="off"
                    spellcheck={false}
                    rows={2}
                    value={note()}
                    aria-invalid={
                      problem() === "required" || problem() === "invalid" ? "true" : "false"
                    }
                    onInput={(event) => {
                      note.set(event.currentTarget.value);
                      problem.set(null);
                    }}
                  />
                  <Show when={problem()}>
                    {(reason) => (
                      <p class="action-feedback action-feedback--error" role="alert">
                        {problemMessage(reason())}
                      </p>
                    )}
                  </Show>
                  <button class="admin-btn-primary" type="submit" disabled={!canSubmit()}>
                    {t("consignment-loss-submit")}
                  </button>
                </form>
              </Show>
            </>
          )}
        </Match>
      </Switch>
    </section>
  );
}

function lossErrorMessage(code: string): string {
  return code === "stale_version" ? t("consignment-loss-stale") : consignmentErrorMessage(code);
}

function problemMessage(reason: "basis" | "required" | "invalid"): string {
  if (reason === "basis") return t("consignment-loss-basis-invalid");
  return reason === "required"
    ? t("consignment-reason-required")
    : consignmentErrorMessage("invalid_reason");
}

function CaseSummary(props: { readonly current: AdminLossCase }): JSX.Element {
  return (
    <dl class="admin-consignment-facts">
      <Fact label="consignment-loss-case-id" value={props.current.case_id} code />
      <Fact label="consignment-loss-state" value={t(STATE_KEYS[props.current.state])} />
      <Fact label="consignment-loss-version" value={String(props.current.case_version)} />
      <Fact label="consignment-loss-generation" value={String(props.current.generation)} />
      <Fact label="consignment-loss-proof-source" value={props.current.proof_source_kind} />
      <Fact label="consignment-loss-proof-digest" value={props.current.proof_digest} code />
      <Fact label="consignment-loss-listing" value={props.current.listing_id} code />
      <Fact label="consignment-loss-seller" value={props.current.seller_user_id} code />
    </dl>
  );
}

function Fact(props: {
  readonly label: string;
  readonly value: string;
  readonly code?: boolean;
}): JSX.Element {
  return (
    <div>
      <dt>{t(props.label)}</dt>
      <dd>{props.code === true ? <code>{props.value}</code> : props.value}</dd>
    </div>
  );
}

function OutcomeList(props: { readonly current: AdminLossCase }): JSX.Element {
  return (
    <section aria-labelledby="admin-loss-outcomes">
      <h2 id="admin-loss-outcomes">{t("consignment-loss-outcomes")}</h2>
      <ul>
        <For each={props.current.outcomes} keyed={(item) => item.outcome_id}>
          {(item) => <OutcomeRow item={item()} />}
        </For>
      </ul>
    </section>
  );
}

function OutcomeRow(props: { readonly item: AdminLossOutcome }): JSX.Element {
  return (
    <li>
      <span>{String(props.item.case_version)}</span> <span>{t(STATE_KEYS[props.item.state])}</span>{" "}
      <Time value={props.item.occurred_at} />
      <Show when={props.item.basis_id}>
        {(basis) => (
          <span>
            {t("consignment-loss-basis-field")} <code>{basis()}</code>
          </span>
        )}
      </Show>
      <Show when={props.item.private_note}>{(text) => <p>{text()}</p>}</Show>
    </li>
  );
}

function FeedbackPanel(props: { readonly feedback: Feedback }): JSX.Element {
  return (
    <Switch>
      <Match when={props.feedback.kind === "error" && props.feedback}>
        {(failure) => (
          <p class="action-feedback action-feedback--error" role="alert">
            {lossErrorMessage((failure() as { readonly code: string }).code)}
          </p>
        )}
      </Match>
      <Match when={props.feedback.kind === "recorded" && props.feedback}>
        {(done) => (
          <p class="action-feedback" role="status">
            {t("consignment-loss-recorded", {
              version: String(
                (done() as { readonly receipt: LossCaseOutcomeReceipt }).receipt.case_version,
              ),
            })}
          </p>
        )}
      </Match>
    </Switch>
  );
}
