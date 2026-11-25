// 挂单详情页的 position 摘要与灭失办理入口：链接到损失案件页；position 仍占用实物时先录入灭失证据，
// 再用证据 ID 与当前读到的 position 版本确认灭失。两个写动作各带命令 ID，未知写只能原样重送；
// 私有说明提交后即清空草稿，不回显（不变量 1、6、7）。

import type {
  AdminPositionV2,
  PositionStateV2,
} from "@auction/contracts/dto/admin/consignment/listing-v2";
import {
  LossProofFieldsSchema,
  type LossProofReceipt,
} from "@auction/contracts/dto/admin/consignment/loss";
import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { Link } from "@auction/web-shared/router";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";
import { is } from "valibot";

import { delistReasonError } from "../../api/consignment/delist";
import { prepareAdminConfirmLoss, prepareAdminLossProof } from "../../api/consignment/loss";
import { adminSessionWritesAllowed } from "../../api/session";
import { useAdminSessionHost } from "../auth/shell";
import type { PendingCommands } from "../pending";
import type { DelistFeedback } from "./actions";
import { consignmentErrorMessage } from "./presentation";

const POSITION_STATE_KEYS = {
  awaiting_take: "consignment-position-state-awaiting-take",
  in_custody: "consignment-position-state-in-custody",
  committed: "consignment-position-state-committed",
  in_transit: "consignment-position-state-in-transit",
  reconcile: "consignment-position-state-reconcile",
  handed_over: "consignment-position-state-handed-over",
  season_ended: "consignment-position-state-season-ended",
  lost: "consignment-position-state-lost",
} satisfies Record<PositionStateV2, string>;

// 与服务端 confirm_position_loss 的来源态一致：只有仍占用实物的四个状态可以办理灭失。
// 其余状态（未收件、已交接、季末、已灭失）不给入口，避免运营录入注定被拒的证据。
const LOSS_ENTRY_STATES: ReadonlySet<PositionStateV2> = new Set([
  "in_custody",
  "committed",
  "in_transit",
  "reconcile",
]);

/** 损失案件页的浏览器路径。 */
export function lossCasePath(positionId: string): string {
  return `/consignment/positions/${positionId}/loss-case`;
}

/** position 摘要、损失案件链接，以及仍占用实物时的“录入证据 → 确认灭失”办理表单。 */
export function PositionLossEntry(props: {
  readonly listingId: string;
  readonly position: AdminPositionV2;
  /** 由详情页持有：重读详情会重建本组件，已取得的证据不能随之丢失。 */
  readonly proof: Cell<LossProofReceipt | null>;
  readonly commands: PendingCommands;
  readonly currentRead: () => boolean;
  readonly onFeedback: (feedback: DelistFeedback | null) => void;
  readonly onConfirmed: (positionId: string) => void;
}): JSX.Element {
  return (
    <section class="admin-consignment-position" aria-labelledby="consignment-position-title">
      <h2 id="consignment-position-title">{t("consignment-position-title")}</h2>
      <dl class="admin-consignment-facts">
        <div>
          <dt>{t("consignment-position-id")}</dt>
          <dd>
            <code>{props.position.position_id}</code>
          </dd>
        </div>
        <div>
          <dt>{t("consignment-position-state")}</dt>
          <dd>{t(POSITION_STATE_KEYS[props.position.state])}</dd>
        </div>
        <div>
          <dt>{t("consignment-loss-generation")}</dt>
          <dd>{props.position.generation === null ? "—" : String(props.position.generation)}</dd>
        </div>
      </dl>
      <Link href={lossCasePath(props.position.position_id)}>
        {t("consignment-position-loss-case-link")}
      </Link>
      <Show when={LOSS_ENTRY_STATES.has(props.position.state)}>
        <LossForms {...props} />
      </Show>
    </section>
  );
}

function LossForms(props: Parameters<typeof PositionLossEntry>[0]): JSX.Element {
  const host = useAdminSessionHost();
  const entity = () => `consignment:${props.listingId}`;
  const preparing = createCell(false);
  const sourceKind = createCell("");
  const observationRef = createCell("");
  const lastKnownRef = createCell("");
  const note = createCell("");
  const reason = createCell("");
  const problem = createCell<"proof" | "required" | "invalid" | null>(null);
  // 证据只对取得它的 position 有效。
  const proof = (): LossProofReceipt | null => {
    const current = props.proof();
    return current?.position_id === props.position.position_id ? current : null;
  };
  const canSubmit = (): boolean =>
    props.currentRead() &&
    adminSessionWritesAllowed(host) &&
    !props.commands.contains(entity()) &&
    !props.commands.anyBusy() &&
    !preparing();

  const submitProof = (event: SubmitEvent): void => {
    event.preventDefault();
    if (!canSubmit()) return;
    const positionId = props.position.position_id;
    const input = {
      sourceKind: sourceKind.peek(),
      observationRef: observationRef.peek(),
      lastKnownRef: lastKnownRef.peek(),
      privateNote: note.peek(),
    };
    const noteProblem = delistReasonError(input.privateNote);
    if (noteProblem !== null) {
      problem.set(noteProblem);
      return;
    }
    // 不变量 6、7：写前按线协议闭集校验，装备链接与 key 形状不发送。
    const valid = is(LossProofFieldsSchema, {
      source_kind: input.sourceKind,
      observation_ref: input.observationRef,
      last_known_ref: input.lastKnownRef,
      private_note: input.privateNote,
    });
    if (!valid) {
      problem.set("proof");
      return;
    }
    problem.set(null);
    preparing.set(true);
    props.onFeedback(null);
    const target = entity();
    const commands = props.commands;
    const feedback = props.onFeedback;
    const store = props.proof;
    try {
      // 不变量 1：命令 ID 由会话宿主分配，结果未知时以原 ID 与原正文重送。
      const submitted = commands.submit(
        target,
        t("consignment-loss-proof-submit"),
        prepareAdminLossProof(host, positionId, input),
        (receipt) => store.set(receipt),
        (code) => {
          if (!commands.contains(target)) feedback({ kind: "error", code });
        },
      );
      if (submitted) {
        sourceKind.set("");
        observationRef.set("");
        lastKnownRef.set("");
        note.set("");
      }
    } finally {
      preparing.set(false);
    }
  };

  const submitConfirm = (event: SubmitEvent): void => {
    event.preventDefault();
    const evidence = proof();
    if (evidence === null || !canSubmit()) return;
    const explanation = reason.peek();
    const reasonProblem = delistReasonError(explanation);
    problem.set(reasonProblem);
    if (reasonProblem !== null) return;
    preparing.set(true);
    props.onFeedback(null);
    const target = entity();
    const commands = props.commands;
    const feedback = props.onFeedback;
    const confirmed = props.onConfirmed;
    const positionId = props.position.position_id;
    try {
      // 不变量 1、6：版本取自当前读到的 position；stale_version 只提示重读，不自动重试。
      const submitted = commands.submit(
        target,
        t("consignment-loss-confirm-submit"),
        prepareAdminConfirmLoss(host, positionId, {
          expectedVersion: props.position.version,
          proofId: evidence.proof_id,
          reason: explanation,
        }),
        () => confirmed(positionId),
        (code) => {
          if (!commands.contains(target)) feedback({ kind: "error", code });
        },
      );
      if (submitted) reason.set("");
    } finally {
      preparing.set(false);
    }
  };

  return (
    <div class="admin-consignment-actions">
      <h3>{t("consignment-loss-entry-title")}</h3>
      <p>{t("consignment-loss-entry-hint")}</p>
      <Show
        when={proof()}
        fallback={
          <form class="admin-consignment-actions__form" novalidate onSubmit={submitProof}>
            <label for="loss-proof-source">{t("consignment-loss-proof-source")}</label>
            <input
              id="loss-proof-source"
              autocomplete="off"
              spellcheck={false}
              value={sourceKind()}
              onInput={(event) => {
                sourceKind.set(event.currentTarget.value);
                problem.set(null);
              }}
            />
            <label for="loss-proof-observation">{t("consignment-loss-proof-observation")}</label>
            <input
              id="loss-proof-observation"
              autocomplete="off"
              spellcheck={false}
              value={observationRef()}
              onInput={(event) => {
                observationRef.set(event.currentTarget.value);
                problem.set(null);
              }}
            />
            <label for="loss-proof-last-known">{t("consignment-loss-proof-last-known")}</label>
            <input
              id="loss-proof-last-known"
              autocomplete="off"
              spellcheck={false}
              value={lastKnownRef()}
              onInput={(event) => {
                lastKnownRef.set(event.currentTarget.value);
                problem.set(null);
              }}
            />
            <label for="loss-proof-note">{t("consignment-loss-note-field")}</label>
            <textarea
              id="loss-proof-note"
              autocomplete="off"
              spellcheck={false}
              rows={2}
              value={note()}
              onInput={(event) => {
                note.set(event.currentTarget.value);
                problem.set(null);
              }}
            />
            <Problem problem={problem()} />
            <button class="admin-btn-primary" type="submit" disabled={!canSubmit()}>
              {t("consignment-loss-proof-submit")}
            </button>
          </form>
        }
      >
        {(evidence) => (
          <form class="admin-consignment-actions__form" novalidate onSubmit={submitConfirm}>
            <p role="status">
              {t("consignment-loss-proof-recorded")} <code>{evidence().proof_id}</code>
            </p>
            <label for="loss-confirm-reason">{t("consignment-loss-confirm-reason")}</label>
            <textarea
              id="loss-confirm-reason"
              autocomplete="off"
              spellcheck={false}
              rows={2}
              value={reason()}
              onInput={(event) => {
                reason.set(event.currentTarget.value);
                problem.set(null);
              }}
            />
            <Problem problem={problem()} />
            <button class="admin-btn-danger" type="submit" disabled={!canSubmit()}>
              {t("consignment-loss-confirm-submit")}
            </button>
          </form>
        )}
      </Show>
    </div>
  );
}

function Problem(props: {
  readonly problem: "proof" | "required" | "invalid" | null;
}): JSX.Element {
  const message = (): string | null => {
    switch (props.problem) {
      case "proof":
        return t("consignment-loss-proof-invalid");
      case "required":
        return t("consignment-reason-required");
      case "invalid":
        return consignmentErrorMessage("invalid_reason");
      default:
        return null;
    }
  };
  return (
    <Show when={message()}>
      {(text) => (
        <p class="action-feedback action-feedback--error" role="alert">
          {text()}
        </p>
      )}
    </Show>
  );
}
