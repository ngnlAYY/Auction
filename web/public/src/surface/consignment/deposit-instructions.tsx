// 本人寄件指引页：none/active 两态、复制预约码、签发与过期刷新。

import type { DepositInstructions } from "@auction/contracts/dto/public/consignment/deposit";
import type { OwnedListingV2 } from "@auction/contracts/dto/public/consignment/listing-v2";
import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { Link, type RoutePageProps } from "@auction/web-shared/router";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { Time } from "@auction/web-shared/ui/components/time";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Match, Show, Switch } from "solid-js";
import { fetchDepositInstructions } from "../../api/consignment-deposit";
import { fetchOwnedConsignmentListingV2 } from "../../api/consignment-owned-v2";
import { type ConsignmentPrivateError, validConsignmentUuid } from "../../api/consignment-private";
import { consignmentWorldLabel } from "./filters";
import { type ConsignmentIntent, useConsignmentPending } from "./pending";
import { PendingCommand } from "./pending-view";
import { consignmentErrorMessage, PrivateConsignmentPage, PrivateReadError } from "./private-page";
import { ConsignmentEmpty } from "./read-view";
import { useConsignmentRefresh } from "./refresh";

interface Loaded {
  readonly listing: OwnedListingV2;
  readonly instructions: DepositInstructions;
}

/** 地址中的挂单 ID 先经规范 UUID 校验；非法值不发起请求。 */
export function DepositInstructionsPage(props: RoutePageProps): JSX.Element {
  const listingId = () => {
    const value = props.params.listing_id ?? "";
    return validConsignmentUuid(value) ? value : null;
  };
  return (
    <PrivateConsignmentPage title={t("consignment-deposit-instructions")}>
      {(userId, canUse) => (
        <Show
          when={listingId()}
          keyed
          fallback={<ConsignmentEmpty title={t("consignment-not-found")} />}
        >
          {(id) => <Instructions userId={userId} listingId={id} canUse={canUse} />}
        </Show>
      )}
    </PrivateConsignmentPage>
  );
}

function Instructions(props: {
  readonly userId: string;
  readonly listingId: string;
  readonly canUse: () => boolean;
}): JSX.Element {
  const read = createTaskResource(props.canUse, async (authorized, context) => {
    if (!authorized) return null;
    const listing = await fetchOwnedConsignmentListingV2(
      props.userId,
      props.listingId,
      context.signal,
    );
    if (!listing.ok) return { ok: false as const, error: listing.error };
    const instructions = await fetchDepositInstructions(
      props.userId,
      props.listingId,
      context.signal,
    );
    if (!instructions.ok) return { ok: false as const, error: instructions.error };
    return {
      ok: true as const,
      value: { listing: listing.value, instructions: instructions.value } satisfies Loaded,
    };
  });
  const feedback = createCell<ConsignmentPrivateError | null>(null);
  const copy = createCell<"copied" | "failed" | null>(null);
  const pending = useConsignmentPending(props.canUse, (result, record) => {
    if (record.operation === "register" || record.listing_id !== props.listingId) return;
    if (result.ok) {
      feedback.set(null);
      copy.set(null);
      read.clear();
      read.restart();
      return;
    }
    feedback.set(result.error);
    if (result.error.code === "stale_version" || result.error.code === "listing_not_active") {
      read.clear();
      read.restart();
    }
  });
  const loaded = () => {
    const result = read();
    return result?.ok ? result.value : null;
  };
  const failure = () => {
    const result = read();
    return result?.ok === false ? result.error : null;
  };
  const relevantPending = () => {
    const record = pending.record();
    return (
      record !== null && record.operation !== "register" && record.listing_id === props.listingId
    );
  };
  useConsignmentRefresh(
    () => props.canUse() && (loaded() !== null || relevantPending()),
    () => {
      if (!props.canUse()) return;
      read.clear();
      read.restart();
      if (relevantPending() && !pending.missing()) pending.check();
    },
  );
  const submitReservation = () => {
    const current = loaded();
    if (current === null || pending.blocked() || !props.canUse()) return;
    feedback.set(null);
    copy.set(null);
    pending.submit({
      operation: "issue_reservation",
      listing_id: props.listingId,
      expected_version: current.listing.version,
    } satisfies ConsignmentIntent);
  };

  return (
    <>
      <Link href={`/consignment/me/${props.listingId}`}>{t("consignment-detail")}</Link>
      <PendingCommand pending={pending} canUse={props.canUse()} listingId={props.listingId} />
      <Show when={feedback()}>{(error) => <p role="alert">{depositErrorMessage(error())}</p>}</Show>
      <Switch fallback={<AsyncState kind="loading" title={t("data-loading")} detail="" />}>
        <Match when={failure()}>
          {(error) => <PrivateReadError error={error()} retry={read.restart} />}
        </Match>
        <Match when={loaded()}>
          {(current) => (
            <section class="public-consignment-account__detail">
              <InstructionBody
                instructions={current().instructions}
                disabled={!props.canUse() || pending.blocked()}
                copy={copy}
                onIssue={submitReservation}
                onRefresh={() => {
                  read.clear();
                  read.restart();
                }}
              />
            </section>
          )}
        </Match>
      </Switch>
    </>
  );
}

function InstructionBody(props: {
  readonly instructions: DepositInstructions;
  readonly disabled: boolean;
  readonly copy: Cell<"copied" | "failed" | null>;
  readonly onIssue: () => void;
  readonly onRefresh: () => void;
}): JSX.Element {
  const active = () => (props.instructions.state === "active" ? props.instructions : null);
  const writeCode = async () => {
    const current = active();
    if (current === null) return;
    try {
      await navigator.clipboard.writeText(current.body);
      props.copy.set("copied");
    } catch {
      props.copy.set("failed");
    }
  };

  return (
    <Switch>
      <Match when={props.instructions.state === "none"}>
        <h2>{t("consignment-deposit-none")}</h2>
        <p>{t("consignment-deposit-not-sent")}</p>
        <button type="button" disabled={props.disabled} onClick={props.onIssue}>
          {t("consignment-deposit-issue")}
        </button>
      </Match>
      <Match when={active()}>
        {(current) => (
          <ActiveInstructions
            instructions={current()}
            disabled={props.disabled}
            copy={props.copy}
            onCopy={writeCode}
            onIssue={props.onIssue}
            onRefresh={props.onRefresh}
          />
        )}
      </Match>
    </Switch>
  );
}

function ActiveInstructions(props: {
  readonly instructions: Extract<DepositInstructions, { state: "active" }>;
  readonly disabled: boolean;
  readonly copy: Cell<"copied" | "failed" | null>;
  readonly onCopy: () => void;
  readonly onIssue: () => void;
  readonly onRefresh: () => void;
}): JSX.Element {
  const expired = () => Date.parse(props.instructions.expires_at) <= Date.now();
  return (
    <>
      <h2>{t("consignment-deposit-active-title")}</h2>
      <Show
        when={!expired()}
        fallback={
          <>
            <p role="alert">{t("consignment-deposit-expired")}</p>
            <button type="button" onClick={props.onRefresh}>
              {t("consignment-deposit-refresh")}
            </button>
          </>
        }
      >
        <dl class="public-consignment-account__facts">
          <div>
            <dt>{t("consignment-deposit-world")}</dt>
            <dd>{consignmentWorldLabel(props.instructions.world)}</dd>
          </div>
          <div>
            <dt>{t("consignment-deposit-bot")}</dt>
            <dd>{props.instructions.bot}</dd>
          </div>
          <div>
            <dt>{t("consignment-deposit-subject")}</dt>
            <dd>
              <code>{props.instructions.subject}</code>
            </dd>
          </div>
          <div>
            <dt>{t("consignment-deposit-body")}</dt>
            <dd>
              <code class="public-consignment-account__code">{props.instructions.body}</code>
            </dd>
          </div>
          <div>
            <dt>{t("consignment-deposit-expires")}</dt>
            <dd>
              <Time value={props.instructions.expires_at} />
            </dd>
          </div>
        </dl>
        <p>{t("consignment-deposit-not-sent")}</p>
        <div class="public-consignment-account__actions">
          <button type="button" disabled={props.disabled} onClick={props.onCopy}>
            {t("consignment-deposit-copy")}
          </button>
          <button type="button" disabled={props.disabled} onClick={props.onIssue}>
            {t("consignment-deposit-reissue")}
          </button>
          <button type="button" onClick={props.onRefresh}>
            {t("consignment-deposit-refresh")}
          </button>
        </div>
        <Show when={props.copy()}>
          {(status) => (
            <p role="status">
              {status() === "copied"
                ? t("consignment-deposit-copied")
                : t("consignment-deposit-copy-failed")}
            </p>
          )}
        </Show>
      </Show>
    </>
  );
}

function depositErrorMessage(error: ConsignmentPrivateError): string {
  switch (error.code) {
    case "invalid_request":
      return t("consignment-deposit-invalid-request");
    case "mail_unavailable":
      return t("consignment-deposit-mail-unavailable");
    case "participant_ineligible":
      return t("consignment-deposit-participant-ineligible");
    case "bot_party":
      return t("consignment-deposit-bot-party");
    case "season_closed":
      return t("consignment-deposit-season-closed");
    case "action_not_allowed":
      return t("consignment-deposit-action-not-allowed");
    case "consignment_disabled":
      return t("consignment-disabled");
    default:
      return consignmentErrorMessage(error);
  }
}
