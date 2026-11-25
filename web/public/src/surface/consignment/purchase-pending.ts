// 购买命令的同身份宿主：发送前冻结原请求，结果未知只允许原样重放；刷新后与重放前先查原回执。

import {
  type PurchaseReceiptV2,
  purchaseReceiptAnswers,
} from "@auction/contracts/dto/public/consignment/purchase-v2";
import type { OwnedCommandReceiptV2 } from "@auction/contracts/dto/public/consignment/receipt-v2";
import {
  ConsignmentUuidSchema,
  ListingPriceSchema,
  ListingVersionSchema,
} from "@auction/contracts/dto/public/consignment/scalar";
import { decode, strictStruct } from "@auction/contracts/wire";
import { parseJson } from "@auction/web-shared/net/wire";
import { getSessionItem, setSessionItem } from "@auction/web-shared/platform/storage";
import { createUuidV4 } from "@auction/web-shared/platform/uuid";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import { err, ok, type Result } from "@auction/web-shared/result";
import { createEffect, untrack, useContext } from "solid-js";
import type * as v from "valibot";

import {
  type ConsignmentOrderError,
  purchaseBusinessRejected,
  purchaseConsignmentListing,
  purchaseWriteRejected,
} from "../../api/consignment-orders";
import {
  commandReceiptMissing,
  fetchOwnedCommandReceiptV2,
} from "../../api/consignment-receipt-v2";
import { PublicCommandIdentityContext } from "../commands/pending";

const STORAGE_KEY = "hv.consignment.purchase.pending.";
const PENDING = strictStruct({
  command_id: ConsignmentUuidSchema,
  expected_user_id: ConsignmentUuidSchema,
  listing_id: ConsignmentUuidSchema,
  expected_version: ListingVersionSchema,
  expected_price: ListingPriceSchema,
});
const STORAGE_FAILURE = { status: 0, code: "pending_storage_unavailable" };
const INVALID_RECEIPT = { status: 200, code: "invalid_response" };

type Pending = v.InferOutput<typeof PENDING>;
/** 购买的业务输入；命令 ID 由宿主在发送前分配并随记录冻结。 */
export interface PurchaseIntent {
  readonly listing_id: string;
  readonly expected_version: number;
  readonly expected_price: number;
}

function load(key: string, userId: string): Result<Pending | null, ConsignmentOrderError> {
  const raw = getSessionItem(key);
  if (raw === null || raw === "null") return ok(null);
  if (raw.length > 512) return err(STORAGE_FAILURE);
  const json = parseJson(raw, { uniqueKeys: "all" });
  if (!json.ok) return err(STORAGE_FAILURE);
  const record = decode(PENDING, json.value);
  // 无法证明损坏记录已结案，保留原值并阻止分配新 ID。
  return record.ok && record.value.expected_user_id === userId
    ? ok(record.value)
    : err(STORAGE_FAILURE);
}

/**
 * 页内购买宿主，离页或失去授权时取消任务。
 * 不变量 1：记录写入成功后才发送；除确定拒绝与成功外一律保留记录，只能原样重放。
 * 恢复的记录先查原命令回执（D9 §6.3）：已有回执即结案，查询 404 才允许原样重放。
 */
export function usePurchasePending(
  canUse: () => boolean,
  onPurchased: (receipt: PurchaseReceiptV2) => void,
) {
  const identity = useContext(PublicCommandIdentityContext);
  const userId = identity?.userId ?? "";
  const key = STORAGE_KEY + userId;
  const restored = load(key, userId);
  const pending = createCell(restored.ok ? restored.value : null);
  const busy = createCell(false);
  const error = createCell<ConsignmentOrderError | null>(restored.ok ? null : restored.error);
  const tasks = createTaskScope();

  function clearPending(): boolean {
    if (pending.peek() === null) return true;
    // 清除标记失败时保留恢复入口，避免刷新后把旧标记误作可继续的新命令。
    if (!setSessionItem(key, "null")) {
      error.set(STORAGE_FAILURE);
      return false;
    }
    pending.set(null);
    return true;
  }

  // 账本回执是首次决定：成交回执必须回答原记录，确定拒绝按原状态与码结案；答非所问保留记录。
  function settleReceipt(record: Pending, receipt: OwnedCommandReceiptV2) {
    if (receipt.operation !== "purchase") {
      error.set(INVALID_RECEIPT);
      return;
    }
    if (receipt.disposition === "rejected") {
      if (clearPending()) error.set({ status: receipt.http_status, code: receipt.code });
      return;
    }
    const request = {
      command_id: record.command_id,
      expected_version: record.expected_version,
      expected_price: record.expected_price,
    };
    if (receipt.listing_id !== record.listing_id || !purchaseReceiptAnswers(request, receipt)) {
      error.set(INVALID_RECEIPT);
      return;
    }
    if (clearPending()) onPurchased(receipt);
  }

  /** 查原命令回执；`replay` 为真时查询 404 后才原样重放同一记录。 */
  function check(replay: boolean) {
    const record = pending.peek();
    if (record === null || busy.peek() || !tasks.alive() || !untrack(canUse)) return;
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
      // 404 只代表当次未见：刷新后不自动重送，人工重试才原样重放。
      else if (!commandReceiptMissing(result.error)) error.set(result.error);
      else if (replay) transmit(record, true);
    });
  }

  createEffect(canUse, (ready) => {
    if (ready) check(false);
  });

  function transmit(record: Pending, retry: boolean) {
    busy.set(true);
    error.set(null);
    tasks.spawn(async (context) => {
      const result = await purchaseConsignmentListing(
        record.expected_user_id,
        record.listing_id,
        {
          command_id: record.command_id,
          expected_version: record.expected_version,
          expected_price: record.expected_price,
        },
        context.signal,
        retry ? undefined : clearPending,
      );
      if (!context.alive()) return;
      busy.set(false);
      if (result.ok) {
        if (clearPending()) onPurchased(result.value);
        return;
      }
      const rejected = retry
        ? purchaseBusinessRejected(result.error)
        : purchaseWriteRejected(result.error);
      if (rejected && !clearPending()) return;
      error.set(result.error);
    });
  }

  return {
    record: () => pending(),
    blocked: () => !restored.ok || pending() !== null,
    busy: () => busy(),
    error: () => error(),
    submit(input: PurchaseIntent): boolean {
      if (pending.peek() !== null || !tasks.alive() || !untrack(canUse)) return false;
      const existing = load(key, userId);
      if (!existing.ok || existing.value !== null) {
        if (!existing.ok) error.set(existing.error);
        return false;
      }
      const record = decode(PENDING, {
        ...input,
        command_id: createUuidV4(),
        expected_user_id: userId,
      });
      if (!record.ok) return false;
      // 发送前写入严格白名单；无法记录时不发送不可恢复的写操作。
      if (!setSessionItem(key, JSON.stringify(record.value))) {
        error.set(STORAGE_FAILURE);
        return false;
      }
      pending.set(record.value);
      transmit(record.value, false);
      return true;
    },
    /** 只读取冻结记录，从不分配新命令 ID；重放前先查原回执。 */
    retry(): boolean {
      const record = pending.peek();
      if (record === null || busy.peek() || !tasks.alive() || !untrack(canUse)) return false;
      check(true);
      return true;
    },
  };
}

/** 页面提供同一份恢复状态给购买入口与恢复面板。 */
export type PurchasePending = ReturnType<typeof usePurchasePending>;
