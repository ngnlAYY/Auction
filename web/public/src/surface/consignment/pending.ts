// 寄售未决命令的同身份宿主、会话级安全记录与回执恢复。
// v1 命令查 v1 账本；预约与 v2 撤回查本人 v2 命令回执（D9 §6.3）：有回执即结案，404 才允许原样重送。

import type {
  CommandReceipt,
  StoredCommandResult,
} from "@auction/contracts/dto/public/consignment";
import type { DepositReservationReceipt } from "@auction/contracts/dto/public/consignment/deposit";
import type { WithdrawReceiptV2 } from "@auction/contracts/dto/public/consignment/listing-v2";
import type { OwnedCommandReceiptV2 } from "@auction/contracts/dto/public/consignment/receipt-v2";
import {
  ConsignmentUuidSchema,
  ExternalEquipmentIdSchema,
  ListingFeeBpsSchema,
  ListingPriceSchema,
  ListingVersionSchema,
} from "@auction/contracts/dto/public/consignment/scalar";
import {
  ListingModeSchema,
  ListingWorldSchema,
} from "@auction/contracts/dto/public/consignment/states";
import { decode, strictStruct } from "@auction/contracts/wire";
import { parseJson } from "@auction/web-shared/net/wire";
import { getSessionItem, setSessionItem } from "@auction/web-shared/platform/storage";
import { createUuidV4 } from "@auction/web-shared/platform/uuid";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import { err, ok, type Result } from "@auction/web-shared/result";
import { createEffect, untrack, useContext } from "solid-js";
import * as v from "valibot";
import { commandErrorOutcomeUncertain } from "../../api/commands";
import {
  depositWriteRejected,
  issueDepositReservation,
  withdrawDepositListing,
} from "../../api/consignment-deposit";
import { fetchConsignmentCommand } from "../../api/consignment-owned";
import type { ConsignmentPrivateError } from "../../api/consignment-private";
import {
  commandReceiptMissing,
  fetchOwnedCommandReceiptV2,
} from "../../api/consignment-receipt-v2";
import {
  consignmentReceiptMatches,
  consignmentWriteRejected,
  registerConsignmentListing,
  repriceConsignmentListing,
  withdrawConsignmentListing,
} from "../../api/consignment-write";
import { PublicCommandIdentityContext } from "../commands/pending";
import { registrationEquipment } from "./registration-input";

const STORAGE_KEY = "hv.consignment.pending.";
const IDENTITY = { command_id: ConsignmentUuidSchema, expected_user_id: ConsignmentUuidSchema };
const TARGET = { listing_id: ConsignmentUuidSchema, expected_version: ListingVersionSchema };

const PENDING = v.union([
  v.pipe(
    strictStruct({
      ...IDENTITY,
      operation: v.literal("register"),
      mode: ListingModeSchema,
      world_id: ListingWorldSchema,
      eid: ExternalEquipmentIdSchema,
      price: ListingPriceSchema,
      expected_fee_bps: ListingFeeBpsSchema,
    }),
    v.check((value) => value.mode !== "direct" || value.expected_fee_bps === 0),
  ),
  strictStruct({
    ...IDENTITY,
    ...TARGET,
    operation: v.literal("reprice"),
    price: ListingPriceSchema,
  }),
  strictStruct({ ...IDENTITY, ...TARGET, operation: v.literal("withdraw") }),
  strictStruct({ ...IDENTITY, ...TARGET, operation: v.literal("issue_reservation") }),
  strictStruct({ ...IDENTITY, ...TARGET, operation: v.literal("withdraw_deposit") }),
]);

type Pending = v.InferOutput<typeof PENDING>;
type DepositPending = Extract<Pending, { operation: "issue_reservation" | "withdraw_deposit" }>;
type InputOf<T> = T extends Pending ? Omit<T, keyof typeof IDENTITY> : never;
/** 写入队列接收固定语义字段；登记链接经单独的一次性参数传入。 */
export type ConsignmentIntent = InputOf<Pending>;
/** 已确认的写入结果；账本拒绝沿用原有 HTTP 状态和安全码。 */
export type ConsignmentOutcome = Result<
  CommandReceipt | DepositReservationReceipt | WithdrawReceiptV2,
  ConsignmentPrivateError
>;

const STORAGE_FAILURE = { status: 0, code: "pending_storage_unavailable" };
const INVALID_RECEIPT = { status: 200, code: "invalid_response" };

function load(key: string, userId: string): Result<Pending | null, ConsignmentPrivateError> {
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

function matchesEquipment(record: Pending, url: string | null): boolean {
  if (record.operation !== "register") return url === null;
  if (url === null) return false;
  const equipment = registrationEquipment(url);
  return (
    equipment !== null && equipment.world_id === record.world_id && equipment.eid === record.eid
  );
}

function originalOutcome(record: Pending, result: StoredCommandResult): ConsignmentOutcome | null {
  // v2 记录只查 v2 回执（depositOutcome）；出现在这里说明调用方配错，保留记录。
  if (record.operation === "issue_reservation" || record.operation === "withdraw_deposit") {
    return null;
  }
  const { receipt } = result;
  if (receipt.operation !== record.operation) return null;
  if (receipt.disposition === "rejected") {
    if (
      record.operation !== "register" &&
      receipt.listing_id !== null &&
      receipt.listing_id !== record.listing_id
    )
      return null;
    return err({ status: result.http_status, code: result.code });
  }
  return consignmentReceiptMatches(receipt, record.command_id, record) ? ok(receipt) : null;
}

/** v2 回执须回答原记录：同一操作、同一命令与挂单、版本恰好推进一；答非所问返回 null 保留记录。 */
function depositOutcome(
  record: DepositPending,
  receipt: OwnedCommandReceiptV2,
): ConsignmentOutcome | null {
  const operation = record.operation === "withdraw_deposit" ? "withdraw" : "issue_reservation";
  if (receipt.disposition === "rejected") {
    return receipt.operation === operation
      ? err({ status: receipt.http_status, code: receipt.code })
      : null;
  }
  if (receipt.operation !== "issue_reservation" && receipt.operation !== "withdraw") return null;
  return receipt.operation === operation &&
    receipt.command_id === record.command_id &&
    receipt.listing_id === record.listing_id &&
    receipt.listing_version === record.expected_version + 1
    ? ok(receipt)
    : null;
}

/** 按记录所属账本查原回执：结案结果、答非所问（null）或查询失败。 */
async function lookup(
  record: Pending,
  userId: string,
  signal: AbortSignal,
): Promise<Result<ConsignmentOutcome | null, ConsignmentPrivateError>> {
  if (record.operation === "issue_reservation" || record.operation === "withdraw_deposit") {
    const result = await fetchOwnedCommandReceiptV2(userId, record.command_id, signal);
    return result.ok ? ok(depositOutcome(record, result.value)) : result;
  }
  const result = await fetchConsignmentCommand(userId, record.command_id, signal);
  return result.ok ? ok(originalOutcome(record, result.value)) : result;
}

function send(
  record: Pending,
  url: string | null,
  signal: AbortSignal,
  onSessionGuardRejected: (() => void) | undefined,
) {
  const { command_id, expected_user_id, operation } = record;
  if (operation === "register") {
    if (url === null) return Promise.resolve(err({ status: 400, code: "invalid_equipment_url" }));
    return registerConsignmentListing(
      expected_user_id,
      {
        command_id,
        equipment_url: url,
        mode: record.mode,
        price: record.price,
        expected_fee_bps: record.expected_fee_bps,
      },
      signal,
      onSessionGuardRejected,
    );
  }
  if (operation === "issue_reservation") {
    return issueDepositReservation(
      expected_user_id,
      record.listing_id,
      { command_id, expected_version: record.expected_version },
      signal,
      onSessionGuardRejected,
    );
  }
  if (operation === "withdraw_deposit") {
    return withdrawDepositListing(
      expected_user_id,
      record.listing_id,
      { command_id, expected_version: record.expected_version },
      signal,
      onSessionGuardRejected,
    );
  }
  const { listing_id, expected_version } = record;
  return operation === "reprice"
    ? repriceConsignmentListing(
        expected_user_id,
        listing_id,
        { command_id, expected_version, price: record.price },
        signal,
        onSessionGuardRejected,
      )
    : withdrawConsignmentListing(
        expected_user_id,
        listing_id,
        { command_id, expected_version },
        signal,
        onSessionGuardRejected,
      );
}

/** 页内命令宿主，离页或失去授权时取消任务。新页先查安全记录的原回执。 */
export function useConsignmentPending(
  canUse: () => boolean,
  onOutcome: (result: ConsignmentOutcome) => void,
) {
  const identity = useContext(PublicCommandIdentityContext);
  const userId = identity?.userId ?? "";
  const key = STORAGE_KEY + userId;
  const restored = load(key, userId);
  const pending = createCell(restored.ok ? restored.value : null);
  const busy = createCell(false);
  const missing = createCell(false);
  const error = createCell<ConsignmentPrivateError | null>(restored.ok ? null : restored.error);
  const tasks = createTaskScope();

  function clearPending(): boolean {
    if (pending.peek() === null) return true;
    // 不变量 1：清除标记失败时保留恢复入口，避免刷新后把旧标记误作可继续的新命令。
    if (!setSessionItem(key, "null")) {
      error.set(STORAGE_FAILURE);
      return false;
    }
    pending.set(null);
    return true;
  }

  function finish(result: ConsignmentOutcome, showOutcome = true) {
    if (!clearPending()) return;
    if (showOutcome) {
      error.set(null);
      onOutcome(result);
    }
  }

  function canApply(result: Result<unknown, ConsignmentPrivateError>): boolean {
    if (untrack(canUse)) return true;
    // 会话守卫会先暂停权限复核；仍存活的宿主只可展示本地认证提示。
    if (!result.ok && (result.error.status === 401 || result.error.code === "session_changed")) {
      error.set(result.error);
    }
    return false;
  }

  /** 查原回执；`replay` 为真时查询 404 后才原样重送同一记录（只用于 v2 记录的人工重送）。 */
  function check(replay = false) {
    const record = pending.peek();
    if (record === null || busy.peek() || !tasks.alive() || !untrack(canUse)) return;
    busy.set(true);
    missing.set(false);
    error.set(null);
    tasks.spawn(async (context) => {
      const result = await lookup(record, userId, context.signal);
      if (!context.alive()) return;
      busy.set(false);
      if (!canApply(result)) return;
      if (result.ok) {
        if (result.value === null) error.set(INVALID_RECEIPT);
        else finish(result.value);
      } else if (commandReceiptMissing(result.error)) {
        // 不变量 1：404 只代表当次未见，仅授权原样重送；记录仍占用本账号的提交入口。
        missing.set(true);
        if (replay) transmit(record, null, true);
      } else {
        error.set(result.error);
      }
    });
  }

  createEffect(canUse, (ready) => {
    if (ready) check();
  });

  function transmit(record: Pending, url: string | null, retry: boolean) {
    busy.set(true);
    missing.set(false);
    error.set(null);
    tasks.spawn(async (context) => {
      // 不变量 1、7：重送只读取冻结记录；明文链接只交给这一次传输。
      // 重送被守卫拦截时，原命令仍需恢复；首次发送才在 API 通知会话前了结。
      const result = await send(record, url, context.signal, retry ? undefined : clearPending);
      if (!context.alive()) return;
      busy.set(false);
      // 不变量 1：前置拒绝只能证明本次重送被拦截，原结果仍需查账本。
      const rejected =
        !result.ok &&
        (record.operation === "issue_reservation" || record.operation === "withdraw_deposit"
          ? depositWriteRejected(record.operation, result.error) &&
            (!retry || result.error.code !== "command_not_sent")
          : !retry && consignmentWriteRejected(record.operation, result.error));
      const canDisplay = canApply(result);
      if (rejected) {
        // 会话守卫可立即暂停界面；仍存活的原账号宿主可以结束已确认拒绝的命令。
        finish(result, canDisplay);
      } else if (canDisplay) {
        if (result.ok) finish(result);
        else {
          // v2 结果未知时开放重送入口；重送前仍先查原回执，404 才真正重送（见 retry）。
          if (
            (record.operation === "issue_reservation" || record.operation === "withdraw_deposit") &&
            commandErrorOutcomeUncertain(result.error)
          ) {
            missing.set(true);
          }
          error.set(result.error);
        }
      }
    });
  }

  return {
    record: () => pending(),
    blocked: () => !restored.ok || pending() !== null,
    busy: () => busy(),
    missing: () => missing(),
    error: () => error(),
    check,
    submit(input: ConsignmentIntent, url: string | null = null): boolean {
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
      if (!record.ok || !matchesEquipment(record.value, url)) return false;
      // 不变量 1、7：发送前写入严格白名单。无法记录时不发送不可恢复的写操作。
      if (!setSessionItem(key, JSON.stringify(record.value))) {
        error.set(STORAGE_FAILURE);
        return false;
      }
      pending.set(record.value);
      transmit(record.value, url, false);
      return true;
    },
    retry(url: string | null = null): boolean {
      const record = pending.peek();
      if (record === null || busy.peek() || !missing.peek() || !tasks.alive() || !untrack(canUse))
        return false;
      if (!matchesEquipment(record, url)) {
        error.set({ status: 400, code: "invalid_equipment_url" });
        return false;
      }
      // 不变量 1：v2 记录每次重送前都先查原回执，已有回执即结案、零写入。
      if (record.operation === "issue_reservation" || record.operation === "withdraw_deposit") {
        check(true);
      } else {
        transmit(record, url, true);
      }
      return true;
    },
  };
}

/** 页面提供同一份恢复状态给命令入口与恢复面板。 */
export type ConsignmentPending = ReturnType<typeof useConsignmentPending>;
