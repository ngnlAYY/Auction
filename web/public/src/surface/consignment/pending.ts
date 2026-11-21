// 寄售未决命令的同身份宿主、会话级安全记录与回执恢复。

import type {
  CommandReceipt,
  StoredCommandResult,
} from "@auction/contracts/dto/public/consignment";
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
import { fetchConsignmentCommand } from "../../api/consignment-owned";
import type { ConsignmentPrivateError } from "../../api/consignment-private";
import {
  consignmentReceiptMatches,
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
]);

type Pending = v.InferOutput<typeof PENDING>;
type InputOf<T> = T extends Pending ? Omit<T, keyof typeof IDENTITY> : never;
/** 写入队列接收固定语义字段；登记链接经单独的一次性参数传入。 */
export type ConsignmentIntent = InputOf<Pending>;
/** 已确认的写入结果；账本拒绝沿用原有 HTTP 状态和安全码。 */
export type ConsignmentOutcome = Result<CommandReceipt, ConsignmentPrivateError>;

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

function send(record: Pending, url: string | null, signal: AbortSignal) {
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
    );
  }
  const { listing_id, expected_version } = record;
  return operation === "reprice"
    ? repriceConsignmentListing(
        expected_user_id,
        listing_id,
        { command_id, expected_version, price: record.price },
        signal,
      )
    : withdrawConsignmentListing(
        expected_user_id,
        listing_id,
        { command_id, expected_version },
        signal,
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

  function finish(result: ConsignmentOutcome) {
    // 不变量 1：清除标记失败时保留恢复入口，避免刷新后把旧标记误作可继续的新命令。
    if (!setSessionItem(key, "null")) {
      error.set(STORAGE_FAILURE);
      return;
    }
    pending.set(null);
    error.set(null);
    onOutcome(result);
  }

  function canApply(result: Result<unknown, ConsignmentPrivateError>): boolean {
    if (untrack(canUse)) return true;
    // 会话守卫会先暂停权限复核；仍存活的宿主只可展示本地认证提示。
    if (!result.ok && (result.error.status === 401 || result.error.code === "session_changed")) {
      error.set(result.error);
    }
    return false;
  }

  function check() {
    const record = pending.peek();
    if (record === null || busy.peek() || !tasks.alive() || !untrack(canUse)) return;
    busy.set(true);
    missing.set(false);
    error.set(null);
    tasks.spawn(async (context) => {
      const result = await fetchConsignmentCommand(userId, record.command_id, context.signal);
      if (!context.alive()) return;
      busy.set(false);
      if (!canApply(result)) return;
      if (result.ok) {
        const original = originalOutcome(record, result.value);
        if (original === null) error.set(INVALID_RECEIPT);
        else finish(original);
      } else if (result.error.status === 404 && result.error.code === "command_not_found") {
        // 404 仅授权原样重送；记录仍占用本账号的提交入口。
        missing.set(true);
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
      const result = await send(record, url, context.signal);
      if (!context.alive()) return;
      busy.set(false);
      if (!canApply(result)) return;
      if (
        !result.ok &&
        (retry ||
          commandErrorOutcomeUncertain(result.error) ||
          result.error.code === "outcome_unknown" ||
          result.error.status === 401)
      ) {
        error.set(result.error);
      } else {
        finish(result);
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
      transmit(record, url, true);
      return true;
    },
  };
}

/** 页面提供同一份恢复状态给命令入口与恢复面板。 */
export type ConsignmentPending = ReturnType<typeof useConsignmentPending>;
