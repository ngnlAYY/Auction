// 未决命令的回执查询和手动原样重送入口。

import { createCell } from "@auction/web-shared/reactive/cell";
import { Link } from "@auction/web-shared/router";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { onCleanup, Show } from "solid-js";

import type { ConsignmentPending } from "./pending";
import { consignmentErrorMessage, ownedConsignmentHref } from "./private-page";

interface RecoveryForm {
  readonly registration?: boolean;
  readonly listingId?: string;
  readonly canRetryRegistration?: boolean;
}

function recoveryHref(record: NonNullable<ReturnType<ConsignmentPending["record"]>>): string {
  return record.operation === "register"
    ? "/consignment/new"
    : ownedConsignmentHref(record.listing_id);
}

/** 已确认身份的恢复入口；需要重新输入的链接从不成为原记录的一部分。 */
export function PendingCommand(
  props: RecoveryForm & {
    readonly pending: ConsignmentPending;
    readonly canUse: boolean;
  },
): JSX.Element {
  const inPlace = () => {
    const record = props.pending.record();
    if (record === null) return false;
    return record.operation === "register"
      ? props.registration === true
      : record.listing_id === props.listingId;
  };

  return (
    <>
      <Show when={props.pending.record()}>
        {(record) => (
          <section
            class="public-consignment-account__pending"
            aria-label={t("consignment-pending")}
          >
            <p role="status">{t("consignment-pending")}</p>
            <button
              type="button"
              disabled={props.pending.busy() || !props.canUse}
              onClick={() => props.pending.check()}
            >
              {t("data-retry")}
            </button>
            <Show when={props.pending.missing()}>
              <Show
                when={inPlace()}
                fallback={
                  <Link href={recoveryHref(record())}>
                    {t(
                      record().operation === "register" ? "consignment-new" : "consignment-detail",
                    )}
                  </Link>
                }
              >
                <Show
                  when={record().operation === "register"}
                  fallback={
                    <button
                      type="button"
                      disabled={!props.canUse || props.pending.busy()}
                      onClick={() => props.pending.retry()}
                    >
                      {t("session-duration-retry")}
                    </button>
                  }
                >
                  <Show when={props.canRetryRegistration}>
                    <ReenterUrl
                      disabled={!props.canUse || props.pending.busy()}
                      onRetry={props.pending.retry}
                    />
                  </Show>
                </Show>
              </Show>
            </Show>
          </section>
        )}
      </Show>
      <Show when={props.pending.error()}>
        {(error) => <p role="alert">{consignmentErrorMessage(error())}</p>}
      </Show>
    </>
  );
}

function ReenterUrl(props: {
  readonly disabled: boolean;
  readonly onRetry: (url: string) => boolean;
}): JSX.Element {
  let input: HTMLInputElement | null = null;
  const url = createCell("");
  const clear = () => {
    url.set("");
    if (input !== null) input.value = "";
  };
  onCleanup(clear);
  return (
    <form
      class="public-consignment-account__form"
      autocomplete="off"
      onSubmit={(event) => {
        event.preventDefault();
        if (!props.disabled && url.peek() !== "" && props.onRetry(url.peek())) clear();
      }}
    >
      <label>
        {t("consignment-reenter-url")}
        <input
          type="password"
          autocomplete="off"
          spellcheck={false}
          ref={(element) => {
            input = element;
          }}
          value={url()}
          onInput={(event) => url.set(event.currentTarget.value)}
        />
      </label>
      <button type="submit" disabled={props.disabled || url() === ""}>
        {t("session-duration-retry")}
      </button>
    </form>
  );
}
