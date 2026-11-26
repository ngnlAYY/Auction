// 免登录登记的会话级未决记录：结果未知时先查原命令决定，404 才允许重新输入链接原样重送。
// 已结案的结果另存一条，刷新后仍可见；它不占提交入口，开始新登记时才清除。

import type {
  CommandReceipt,
  DirectCommandResult,
} from "@auction/contracts/dto/public/consignment";
import {
  ConsignmentUuidSchema,
  ExternalEquipmentIdSchema,
  ListingPriceSchema,
} from "@auction/contracts/dto/public/consignment/scalar";
import { ListingWorldSchema } from "@auction/contracts/dto/public/consignment/states";
import { decode, strictStruct } from "@auction/contracts/wire";
import { parseJson } from "@auction/web-shared/net/wire";
import { getSessionItem, setSessionItem } from "@auction/web-shared/platform/storage";
import { createUuidV4 } from "@auction/web-shared/platform/uuid";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import { err, ok, type Result } from "@auction/web-shared/result";
import * as v from "valibot";

import {
  directWriteRejected,
  fetchDirectCommand,
  registerDirectListing,
} from "../../api/consignment-direct";
import type { ConsignmentPrivateError } from "../../api/consignment-private";
import { commandReceiptMissing } from "../../api/consignment-receipt-v2";
import { registrationEquipment } from "./registration-input";

const STORAGE_KEY = "hv.consignment.direct.pending";
const OUTCOME_KEY = "hv.consignment.direct.outcome";
const STORAGE_FAILURE = { status: 0, code: "pending_storage_unavailable" };
const INVALID_RECEIPT = { status: 200, code: "invalid_response" };

// 不变量 7：只记安全的装备标识，读取 key 与完整链接从不落盘。
const PENDING = strictStruct({
  command_id: ConsignmentUuidSchema,
  seller_game_name: v.pipe(v.string(), v.maxLength(128)),
  world_id: ListingWorldSchema,
  eid: ExternalEquipmentIdSchema,
  price: ListingPriceSchema,
});
type Pending = v.InferOutput<typeof PENDING>;

// 不变量 7：结案记录只有命令 ID 与挂单 ID，或首次决定的拒绝码；不含自填名与装备链接。
const OUTCOME = v.union([
  strictStruct({
    kind: v.literal("accepted"),
    command_id: ConsignmentUuidSchema,
    listing_id: ConsignmentUuidSchema,
  }),
  strictStruct({
    kind: v.literal("rejected"),
    status: v.pipe(v.number(), v.integer(), v.minValue(0), v.maxValue(599)),
    code: v.pipe(v.string(), v.maxLength(128)),
  }),
]);

/** 表单冻结的语义字段；链接经单独参数传入。 */
export type DirectIntent = Omit<Pending, "command_id">;
/** 已结案的结果：受理回执的命令 ID 与挂单，或首次决定的拒绝。 */
export type DirectOutcome = Result<
  { command_id: string; listing_id: string },
  ConsignmentPrivateError
>;

function load(): Result<Pending | null, ConsignmentPrivateError> {
  const raw = getSessionItem(STORAGE_KEY);
  if (raw === null || raw === "null") return ok(null);
  if (raw.length > 1024) return err(STORAGE_FAILURE);
  const json = parseJson(raw, { uniqueKeys: "all" });
  if (!json.ok) return err(STORAGE_FAILURE);
  const record = decode(PENDING, json.value);
  // 无法证明损坏记录已结案，保留原值并阻止分配新 ID。
  return record.ok ? ok(record.value) : err(STORAGE_FAILURE);
}

function storedOutcome(result: DirectOutcome): string {
  return JSON.stringify(
    result.ok
      ? { kind: "accepted", ...result.value }
      : { kind: "rejected", status: result.error.status, code: result.error.code },
  );
}

function loadOutcome(): Result<DirectOutcome | null, ConsignmentPrivateError> {
  const raw = getSessionItem(OUTCOME_KEY);
  if (raw === null || raw === "null") return ok(null);
  if (raw.length > 1024) return err(STORAGE_FAILURE);
  const json = parseJson(raw, { uniqueKeys: "all" });
  if (!json.ok) return err(STORAGE_FAILURE);
  const record = decode(OUTCOME, json.value);
  if (!record.ok) return err(STORAGE_FAILURE);
  const value = record.value;
  return ok(
    value.kind === "accepted"
      ? ok({ command_id: value.command_id, listing_id: value.listing_id })
      : err({ status: value.status, code: value.code }),
  );
}

function matchesEquipment(record: Pending, url: string): boolean {
  const equipment = registrationEquipment(url);
  return equipment?.world_id === record.world_id && equipment.eid === record.eid;
}

function accepted(record: Pending, receipt: CommandReceipt): DirectOutcome | null {
  return receipt.command_id === record.command_id &&
    receipt.disposition === "registered" &&
    receipt.price === record.price &&
    receipt.listing_id !== null
    ? ok({ command_id: record.command_id, listing_id: receipt.listing_id })
    : null;
}

function stored(record: Pending, found: DirectCommandResult): DirectOutcome | null {
  const { result } = found;
  return result.receipt.disposition === "rejected"
    ? err({ status: result.http_status, code: result.code })
    : accepted(record, result.receipt);
}

/** 页内宿主，离页时取消任务；新页先查安全记录的原决定。 */
export function useDirectPending(onOutcome: (result: DirectOutcome) => void) {
  const restored = load();
  const pending = createCell(restored.ok ? restored.value : null);
  const busy = createCell(false);
  const missing = createCell(false);
  const error = createCell<ConsignmentPrivateError | null>(restored.ok ? null : restored.error);
  const tasks = createTaskScope();

  function finish(result: DirectOutcome) {
    // 不变量 1：先存结案结果再清除未决标记；任一步失败都保留未决记录，刷新后重查原决定，
    // 不会把旧标记误作可继续的新命令。
    if (
      !setSessionItem(OUTCOME_KEY, storedOutcome(result)) ||
      !setSessionItem(STORAGE_KEY, "null")
    ) {
      error.set(STORAGE_FAILURE);
      return;
    }
    pending.set(null);
    error.set(null);
    onOutcome(result);
  }

  function check() {
    const record = pending.peek();
    if (record === null || busy.peek() || !tasks.alive()) return;
    busy.set(true);
    missing.set(false);
    error.set(null);
    tasks.spawn(async (context) => {
      const result = await fetchDirectCommand(record.command_id, context.signal);
      if (!context.alive()) return;
      busy.set(false);
      if (result.ok) {
        const outcome = stored(record, result.value);
        if (outcome === null) error.set(INVALID_RECEIPT);
        else finish(outcome);
      } else if (commandReceiptMissing(result.error)) {
        // 不变量 1：404 只代表当次未见，仅授权原样重送；记录仍占用提交入口。
        missing.set(true);
      } else {
        error.set(result.error);
      }
    });
  }

  function transmit(record: Pending, url: string, retry: boolean) {
    busy.set(true);
    missing.set(false);
    error.set(null);
    tasks.spawn(async (context) => {
      // 不变量 1、7：重送只读取冻结记录；明文链接只交给这一次传输。
      const result = await registerDirectListing(
        {
          command_id: record.command_id,
          seller_game_name: record.seller_game_name,
          equipment_url: url,
          price: record.price,
        },
        context.signal,
      );
      if (!context.alive()) return;
      busy.set(false);
      if (result.ok) {
        const outcome = accepted(record, result.value);
        if (outcome === null) error.set(INVALID_RECEIPT);
        else finish(outcome);
      } else if (!retry && directWriteRejected(result.error)) {
        // 首次提交的可信拒绝了结命令；重送被拒只证明这一次被拦截，原结果仍需查询。
        finish(result);
      } else {
        error.set(result.error);
      }
    });
  }

  if (restored.ok && restored.value !== null) check();
  else {
    // 刷新后回到已结案结果；损坏的结案记录不占提交入口，只提示存储不可用。
    const outcome = loadOutcome();
    if (!outcome.ok) error.set(outcome.error);
    else if (outcome.value !== null) onOutcome(outcome.value);
  }

  return {
    record: () => pending(),
    blocked: () => !restored.ok || pending() !== null,
    busy: () => busy(),
    missing: () => missing(),
    error: () => error(),
    check,
    /** 开始新登记前清除已结案结果；清除失败时不离开结果视图。 */
    dismiss(): boolean {
      if (!setSessionItem(OUTCOME_KEY, "null")) {
        error.set(STORAGE_FAILURE);
        return false;
      }
      error.set(null);
      return true;
    },
    submit(input: DirectIntent, url: string): boolean {
      if (pending.peek() !== null || !tasks.alive()) return false;
      const existing = load();
      if (!existing.ok || existing.value !== null) {
        if (!existing.ok) error.set(existing.error);
        return false;
      }
      const record = decode(PENDING, { ...input, command_id: createUuidV4() });
      if (!record.ok || !matchesEquipment(record.value, url)) return false;
      // 新登记取代上一次的结案结果；清不掉就不发送，避免刷新后显示过期结果。
      if (!setSessionItem(OUTCOME_KEY, "null")) {
        error.set(STORAGE_FAILURE);
        return false;
      }
      // 不变量 1、7：发送前写入严格白名单。无法记录时不发送不可恢复的写操作。
      if (!setSessionItem(STORAGE_KEY, JSON.stringify(record.value))) {
        error.set(STORAGE_FAILURE);
        return false;
      }
      pending.set(record.value);
      transmit(record.value, url, false);
      return true;
    },
    retry(url: string): boolean {
      const record = pending.peek();
      if (record === null || busy.peek() || !missing.peek() || !tasks.alive()) return false;
      if (!matchesEquipment(record, url)) {
        error.set({ status: 400, code: "invalid_equipment_url" });
        return false;
      }
      transmit(record, url, true);
      return true;
    },
  };
}

/** 免登录登记页共享的恢复状态。 */
export type DirectPending = ReturnType<typeof useDirectPending>;
