// 本人重新上架页：只在服务端 allowed_actions 含 relist 时给出提交，未决命令原样重放。
//
// 命令记录只含命令 ID、目标与版本（不含任何秘密），写入会话存储后才发送；结果未知时
// 只许原样重送同一记录，不生成新命令（不变量 1、6）。刷新后与重送前先查原命令回执，已有回执即结案，
// 查询 404 才允许原样重送（D9 §6.3）。能否重列由服务端裁定，页面不重建状态机。

import { CommandUuidV4Schema } from "@auction/contracts/dto/public/consignment/command";
import type { OwnedListingV2 } from "@auction/contracts/dto/public/consignment/listing-v2";
import type { OwnedCommandReceiptV2 } from "@auction/contracts/dto/public/consignment/receipt-v2";
import {
  ConsignmentUuidSchema,
  ListingVersionSchema,
} from "@auction/contracts/dto/public/consignment/scalar";
import { decode, strictStruct } from "@auction/contracts/wire";
import { parseJson } from "@auction/web-shared/net/wire";
import { getSessionItem, setSessionItem } from "@auction/web-shared/platform/storage";
import { createUuidV4 } from "@auction/web-shared/platform/uuid";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import { Link, type RoutePageProps } from "@auction/web-shared/router";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, Match, Show, Switch, untrack } from "solid-js";
import type * as v from "valibot";

import { commandErrorOutcomeUncertain } from "../../api/commands";
import { fetchOwnedConsignmentListingV2 } from "../../api/consignment-owned-v2";
import { type ConsignmentPrivateError, validConsignmentUuid } from "../../api/consignment-private";
import {
  commandReceiptMissing,
  fetchOwnedCommandReceiptV2,
} from "../../api/consignment-receipt-v2";
import {
  relistConsignmentListing,
  relistReceiptMatches,
  relistRejected,
} from "../../api/consignment-relist";
import { relistErrorKey } from "./labels";
import {
  consignmentErrorMessage,
  ownedConsignmentHref,
  PrivateConsignmentPage,
  PrivateReadError,
} from "./private-page";
import { ConsignmentEmpty } from "./read-view";

const STORAGE_KEY = "hv.consignment.relist.pending.";
const PENDING = strictStruct({
  command_id: CommandUuidV4Schema,
  expected_user_id: ConsignmentUuidSchema,
  listing_id: ConsignmentUuidSchema,
  expected_version: ListingVersionSchema,
});
type Pending = v.InferOutput<typeof PENDING>;
const STORAGE_FAILURE = { status: 0, code: "pending_storage_unavailable" };
const INVALID_RECEIPT = { status: 200, code: "invalid_response" };

function load(key: string, userId: string): Pending | "corrupt" | null {
  const raw = getSessionItem(key);
  if (raw === null || raw === "null") return null;
  const json = raw.length > 512 ? null : parseJson(raw, { uniqueKeys: "all" });
  const record = json?.ok === true ? decode(PENDING, json.value) : null;
  // 无法证明损坏记录已结案，保留原值并阻止分配新命令 ID。
  return record?.ok === true && record.value.expected_user_id === userId ? record.value : "corrupt";
}

/** 地址中的挂单 ID 先经规范 UUID 校验；非法值不发起请求。 */
export function RelistPage(props: RoutePageProps): JSX.Element {
  const listingId = () => {
    const value = props.params.listing_id ?? "";
    return validConsignmentUuid(value) ? value : null;
  };
  return (
    <PrivateConsignmentPage title={t("consignment-relist")}>
      {(userId, canUse) => (
        <Show
          when={listingId()}
          keyed
          fallback={<ConsignmentEmpty title={t("consignment-not-found")} />}
        >
          {(id) => <Relist userId={userId} listingId={id} canUse={canUse} />}
        </Show>
      )}
    </PrivateConsignmentPage>
  );
}

function Relist(props: {
  readonly userId: string;
  readonly listingId: string;
  readonly canUse: () => boolean;
}): JSX.Element {
  const key = STORAGE_KEY + props.userId;
  const stored = load(key, props.userId);
  const read = createTaskResource(props.canUse, async (authorized, context) => {
    if (!authorized) return null;
    return fetchOwnedConsignmentListingV2(props.userId, props.listingId, context.signal);
  });
  const pending = createCell<Pending | null>(
    stored !== "corrupt" && stored?.listing_id === props.listingId ? stored : null,
  );
  const busy = createCell(false);
  const error = createCell<ConsignmentPrivateError | null>(
    stored === "corrupt" ? STORAGE_FAILURE : null,
  );
  const done = createCell(false);
  const tasks = createTaskScope();
  // 他处的未决记录或损坏记录同样占用本账号的提交入口。
  const blocked = () =>
    stored === "corrupt" || (stored !== null && stored.listing_id !== props.listingId);

  const clear = (): boolean => {
    if (!setSessionItem(key, "null")) {
      // 不变量 1：清除标记失败时保留恢复入口，避免刷新后把旧记录误作可继续的新命令。
      error.set(STORAGE_FAILURE);
      return false;
    }
    pending.set(null);
    return true;
  };

  function transmit(record: Pending, replay: boolean) {
    busy.set(true);
    error.set(null);
    tasks.spawn(async (context) => {
      const result = await relistConsignmentListing(
        record.expected_user_id,
        record.listing_id,
        { command_id: record.command_id, expected_version: record.expected_version },
        context.signal,
        replay ? undefined : () => void clear(),
      );
      if (!context.alive()) return;
      busy.set(false);
      if (result.ok) {
        if (clear()) done.set(true);
        read.clear();
        read.restart();
      } else if (relistRejected(result.error, replay)) {
        if (clear()) error.set(result.error);
        read.clear();
        read.restart();
      } else {
        // 结果未知：只保留原记录，等待同一命令的原样重送。
        error.set(result.error);
      }
    });
  }

  // 账本回执是首次决定：重列回执必须回答原记录，确定拒绝按原状态与码结案；答非所问保留记录。
  function settleReceipt(record: Pending, receipt: OwnedCommandReceiptV2) {
    if (receipt.operation !== "relist") {
      error.set(INVALID_RECEIPT);
      return;
    }
    if (receipt.disposition === "rejected") {
      if (clear()) error.set({ status: receipt.http_status, code: receipt.code });
    } else if (
      !relistReceiptMatches(receipt, record.listing_id, {
        command_id: record.command_id,
        expected_version: record.expected_version,
      })
    ) {
      error.set(INVALID_RECEIPT);
      return;
    } else if (clear()) {
      done.set(true);
    }
    read.clear();
    read.restart();
  }

  /** 查原命令回执；`replay` 为真时查询 404 后才原样重送同一记录。 */
  function check(replay: boolean) {
    const record = pending.peek();
    if (record === null || busy.peek() || !tasks.alive() || !untrack(props.canUse)) return;
    busy.set(true);
    error.set(null);
    tasks.spawn(async (context) => {
      const result = await fetchOwnedCommandReceiptV2(
        record.expected_user_id,
        record.command_id,
        context.signal,
      );
      if (!context.alive()) return;
      busy.set(false);
      if (result.ok) settleReceipt(record, result.value);
      // 404 只代表当次未见：刷新后不自动重送，人工重试才原样重送。
      else if (!commandReceiptMissing(result.error)) error.set(result.error);
      else if (replay) transmit(record, true);
    });
  }

  createEffect(props.canUse, (ready) => {
    if (ready) check(false);
  });

  const loaded = () => {
    const result = read();
    return result?.ok ? result.value : null;
  };
  const failure = () => {
    const result = read();
    return result?.ok === false ? result.error : null;
  };
  const submit = () => {
    const current = loaded();
    if (
      current === null ||
      busy.peek() ||
      pending.peek() !== null ||
      blocked() ||
      !current.allowed_actions.includes("relist") ||
      !untrack(props.canUse)
    ) {
      return;
    }
    const record = decode(PENDING, {
      command_id: createUuidV4(),
      expected_user_id: props.userId,
      listing_id: props.listingId,
      expected_version: current.version,
    });
    // 不变量 1：发送前写入严格白名单；无法记录时不发送不可恢复的写操作。
    if (!record.ok || !setSessionItem(key, JSON.stringify(record.value))) {
      error.set(STORAGE_FAILURE);
      return;
    }
    pending.set(record.value);
    transmit(record.value, false);
  };
  const retry = () => check(true);

  return (
    <>
      <Link href={ownedConsignmentHref(props.listingId)}>{t("consignment-detail")}</Link>
      <Show when={pending()}>
        <section class="public-consignment-account__pending" aria-label={t("consignment-pending")}>
          <p role="status">{t("consignment-pending")}</p>
          <button type="button" disabled={busy() || !props.canUse()} onClick={retry}>
            {t("session-duration-retry")}
          </button>
        </section>
      </Show>
      <Show when={error()}>{(current) => <p role="alert">{relistMessage(current())}</p>}</Show>
      <Show when={done()}>
        <p role="status">{t("consignment-relist-done")}</p>
      </Show>
      <Switch fallback={<AsyncState kind="loading" title={t("data-loading")} detail="" />}>
        <Match when={failure()}>
          {(current) => <PrivateReadError error={current()} retry={read.restart} />}
        </Match>
        <Match when={loaded()}>
          {(current) => (
            <RelistSummary
              listing={current()}
              disabled={busy() || pending() !== null || blocked() || !props.canUse()}
              onRelist={submit}
            />
          )}
        </Match>
      </Switch>
    </>
  );
}

function RelistSummary(props: {
  readonly listing: OwnedListingV2;
  readonly disabled: boolean;
  readonly onRelist: () => void;
}): JSX.Element {
  return (
    <section class="public-consignment-account__detail">
      <dl class="public-consignment-account__facts">
        <div>
          <dt>{t("consignment-price")}</dt>
          <dd>{formatCreditAmount(props.listing.price)}</dd>
        </div>
      </dl>
      <Show
        when={props.listing.allowed_actions.includes("relist")}
        fallback={<p>{t("consignment-relist-unavailable")}</p>}
      >
        <p>{t("consignment-relist-explain")}</p>
        <button type="button" disabled={props.disabled} onClick={props.onRelist}>
          {t("consignment-relist")}
        </button>
      </Show>
    </section>
  );
}

function relistMessage(error: ConsignmentPrivateError): string {
  const key = relistErrorKey(error.code);
  if (key !== null) return t(key);
  // 结果未知的错误统一提示重送原请求，避免暗示命令已失败。
  if (commandErrorOutcomeUncertain(error)) return t("consignment-pending");
  return consignmentErrorMessage(error);
}
