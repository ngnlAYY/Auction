// Admin v2 隔离详情、重检、解除与代管件止售的严格读取和写命令。

import { ConsignmentUuidSchema } from "@auction/contracts/dto/public/consignment/scalar";
import { strictStruct } from "@auction/contracts/wire";
import type { JsonFieldPolicy } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import { type GenericSchema, type InferOutput, is, literal, string } from "valibot";

import {
  type AdminWriteFailure,
  decodeAdminReceipt,
  invalidResponseError,
  responseError,
  writeNotSent,
  writeOutcomeUnknown,
} from "../failure";
import type { AdminSessionHost } from "../session";
import { prepareJsonCommandOnce, sendGet, sendPreparedJsonWrite } from "../transport";

const DEPOSITS = "/api/v2/admin/consignment/deposits";
const LISTINGS = "/api/v2/admin/consignment/listings";

const DETAIL_POLICY: JsonFieldPolicy = {
  uniqueKeys: "all",
  integerPaths: [
    ["data", "version"],
    ["data", "observation_total"],
    ["data", "observations", null, "attachment_count"],
    ["data", "observations", null, "credits"],
    ["data", "observations", null, "cod"],
    ["data", "observations", null, "items", null, "ordinal"],
    ["data", "observations", null, "items", null, "amount"],
    ["data", "release", "outcome", "original_round"],
    ["data", "release", "outcome", "listing_changes", null, "listing_version"],
  ],
};

const HOLD_POLICY: JsonFieldPolicy = {
  uniqueKeys: "all",
  integerPaths: [
    ["data", "listing_version"],
    ["data", "price"],
    ["data", "original_round"],
    ["data", "listings", null, "listing_version"],
  ],
};

type AdminSchemas = typeof import("@auction/contracts/dto/admin/consignment/quarantine");
type DelistSchemas = typeof import("@auction/contracts/dto/admin/consignment");
let adminSchemas: Promise<Result<AdminSchemas, string>> | undefined;
let delistSchemas: Promise<Result<DelistSchemas, string>> | undefined;
/** 只缓存已成功的模块加载；分块暂时失败后下一次调用仍可重试，且把拒绝转成稳定失败。 */
function schemas(): Promise<Result<AdminSchemas, string>> {
  adminSchemas ??= import("@auction/contracts/dto/admin/consignment/quarantine").then(
    (module) => ok(module),
    () => {
      adminSchemas = undefined;
      return err("network_error");
    },
  );
  return adminSchemas;
}

function delistSchemasModule(): Promise<Result<DelistSchemas, string>> {
  delistSchemas ??= import("@auction/contracts/dto/admin/consignment").then(
    (module) => ok(module),
    () => {
      delistSchemas = undefined;
      return err("network_error");
    },
  );
  return delistSchemas;
}

async function readResponse<T>(
  host: AdminSessionHost,
  url: string,
  status: 200 | 201 | 202,
  code: "ok" | "created" | "accepted",
  schema: GenericSchema<unknown, T>,
  policy: JsonFieldPolicy,
  signal: AbortSignal,
): Promise<Result<T, string>> {
  const sent = await sendGet(host, url, signal);
  if (!sent.ok) return err(sent.error);
  const response = sent.value;
  if (response.status !== status) {
    if (response.status < 400) {
      return err(invalidResponseError("admin quarantine status"));
    }
    return err(await responseError(response, signal, host));
  }
  const decoded = decodeAdminReceipt(
    response,
    strictStruct({ code: literal(code), message: string(), data: schema }),
    policy,
  );
  return decoded.ok ? ok(decoded.value.data) : err(decoded.error);
}

/** 读取隔离详情；目标不存在与其它错误保持稳定协议码。 */
export async function fetchAdminQuarantine(
  host: AdminSessionHost,
  quarantineId: string,
  signal: AbortSignal,
): Promise<Result<InferOutput<AdminSchemas["QuarantineDetailSchema"]>, string>> {
  if (!is(ConsignmentUuidSchema, quarantineId)) return err("invalid_request");
  const schemasResult = await schemas();
  if (!schemasResult.ok) return err(schemasResult.error);
  const { QuarantineDetailSchema } = schemasResult.value;
  const result = await readResponse(
    host,
    `${DEPOSITS}/${quarantineId}`,
    200,
    "ok",
    QuarantineDetailSchema,
    DETAIL_POLICY,
    signal,
  );
  return result.ok && result.value.quarantine_id !== quarantineId
    ? err(invalidResponseError("admin quarantine target"))
    : result;
}

type CommandOperation<T> = (
  commandId: string,
  signal: AbortSignal,
) => Promise<Result<T, AdminWriteFailure>>;

/** 重检只排工作；同一命令 ID 的重试复用同一正文。 */
export async function prepareAdminQuarantineRecheck(
  host: AdminSessionHost,
  quarantineId: string,
  expectedVersion: number,
): Promise<CommandOperation<InferOutput<AdminSchemas["QuarantineRecheckReceiptSchema"]>>> {
  const schemasResult = await schemas();
  if (!schemasResult.ok) {
    const failure = writeNotSent(schemasResult.error);
    return async () => err(failure);
  }
  const { QuarantineRecheckRequestSchema, QuarantineRecheckReceiptSchema } = schemasResult.value;
  const body = (commandId: string) => ({
    command_id: commandId,
    expected_version: expectedVersion,
  });
  const prepare = prepareJsonCommandOnce(body);
  return async (commandId, signal) => {
    if (!is(ConsignmentUuidSchema, quarantineId)) {
      return err(writeNotSent("invalid_request"));
    }
    if (!Number.isSafeInteger(expectedVersion) || expectedVersion < 1) {
      return err(writeNotSent("invalid_request"));
    }
    const request = body(commandId);
    if (!is(QuarantineRecheckRequestSchema, request)) {
      return err(writeNotSent("invalid_request"));
    }
    const prepared = prepare(commandId);
    if (!prepared.ok) return err(prepared.error);
    const sent = await sendPreparedJsonWrite(
      host,
      {
        url: `${DEPOSITS}/${quarantineId}/recheck`,
        method: "POST",
        endpoint: "consignment-delist",
      },
      prepared.value,
      signal,
    );
    if (!sent.ok) return err(sent.error);
    const response = sent.value;
    const decoded = decodeAdminReceipt(
      response,
      strictStruct({
        code: literal("accepted"),
        message: string(),
        data: QuarantineRecheckReceiptSchema,
      }),
      { uniqueKeys: "all", integerPaths: [] },
    );
    if (!decoded.ok) return err(writeOutcomeUnknown(decoded.error, response.status));
    const receipt = decoded.value.data;
    return response.status === 202 &&
      receipt.command_id === commandId &&
      receipt.quarantine_id === quarantineId
      ? ok(receipt)
      : err(writeOutcomeUnknown(invalidResponseError("admin quarantine recheck"), response.status));
  };
}

/** 解除引用具体观察和拟定结果；不得提供 force。 */
export async function prepareAdminQuarantineRelease(
  host: AdminSessionHost,
  quarantineId: string,
  expectedVersion: number,
  observationId: string,
  proposedKind: InferOutput<AdminSchemas["QuarantineDecisionKindSchema"]>,
): Promise<CommandOperation<InferOutput<AdminSchemas["QuarantineReleaseReceiptSchema"]>>> {
  const schemasResult = await schemas();
  if (!schemasResult.ok) {
    const failure = writeNotSent(schemasResult.error);
    return async () => err(failure);
  }
  const { QuarantineReleaseRequestSchema, QuarantineReleaseReceiptSchema } = schemasResult.value;
  const body = (commandId: string) => ({
    command_id: commandId,
    expected_version: expectedVersion,
    observation_id: observationId,
    proposed_kind: proposedKind,
  });
  const prepare = prepareJsonCommandOnce(body);
  return async (commandId, signal) => {
    if (!is(ConsignmentUuidSchema, quarantineId) || !is(ConsignmentUuidSchema, observationId)) {
      return err(writeNotSent("invalid_request"));
    }
    if (!Number.isSafeInteger(expectedVersion) || expectedVersion < 1) {
      return err(writeNotSent("invalid_request"));
    }
    const request = body(commandId);
    if (!is(QuarantineReleaseRequestSchema, request)) {
      return err(writeNotSent("invalid_request"));
    }
    const prepared = prepare(commandId);
    if (!prepared.ok) return err(prepared.error);
    const sent = await sendPreparedJsonWrite(
      host,
      {
        url: `${DEPOSITS}/${quarantineId}/release-quarantine`,
        method: "POST",
        endpoint: "consignment-delist",
      },
      prepared.value,
      signal,
    );
    if (!sent.ok) return err(sent.error);
    const response = sent.value;
    const decoded = decodeAdminReceipt(
      response,
      strictStruct({
        code: literal("ok"),
        message: string(),
        data: QuarantineReleaseReceiptSchema,
      }),
      HOLD_POLICY,
    );
    if (!decoded.ok) return err(writeOutcomeUnknown(decoded.error, response.status));
    const receipt = decoded.value.data;
    return response.status === 200 &&
      receipt.command_id === commandId &&
      receipt.quarantine_id === quarantineId &&
      receipt.decision_kind === proposedKind
      ? ok(receipt)
      : err(writeOutcomeUnknown(invalidResponseError("admin quarantine release"), response.status));
  };
}

export type AdminHoldOutcome =
  | {
      readonly kind: "hold";
      readonly receipt: InferOutput<AdminSchemas["ConsignmentDelistReceiptV2Schema"]>;
    }
  | {
      readonly kind: "closed";
      readonly receipt: InferOutput<AdminSchemas["ConsignmentDelistReceiptV2Schema"]>;
    };

/** 代管件设止售；未代管件由同一 v2 命令返回 closed 回执。 */
export async function prepareAdminHold(
  host: AdminSessionHost,
  listingId: string,
  expectedVersion: number,
  reason: string,
): Promise<CommandOperation<AdminHoldOutcome>> {
  const [schemaResult, delistResult] = await Promise.all([schemas(), delistSchemasModule()]);
  if (!schemaResult.ok) {
    const failure = writeNotSent(schemaResult.error);
    return async () => err(failure);
  }
  if (!delistResult.ok) {
    const failure = writeNotSent(delistResult.error);
    return async () => err(failure);
  }
  const { ConsignmentDelistReceiptV2Schema } = schemaResult.value;
  const { AdminDelistListingSchema } = delistResult.value;
  const body = (commandId: string) => ({
    command_id: commandId,
    expected_version: expectedVersion,
    reason,
  });
  const prepare = prepareJsonCommandOnce(body);
  return async (commandId, signal) => {
    if (!is(ConsignmentUuidSchema, listingId)) {
      return err(writeNotSent("invalid_listing_id"));
    }
    const request = body(commandId);
    if (!is(AdminDelistListingSchema, request)) {
      return err(writeNotSent("invalid_request"));
    }
    const prepared = prepare(commandId);
    if (!prepared.ok) return err(prepared.error);
    const sent = await sendPreparedJsonWrite(
      host,
      { url: `${LISTINGS}/${listingId}/delist`, method: "POST", endpoint: "consignment-delist" },
      prepared.value,
      signal,
    );
    if (!sent.ok) return err(sent.error);
    const response = sent.value;
    const decoded = decodeAdminReceipt(
      response,
      strictStruct({
        code: literal("ok"),
        message: string(),
        data: ConsignmentDelistReceiptV2Schema,
      }),
      HOLD_POLICY,
    );
    if (!decoded.ok) return err(writeOutcomeUnknown(decoded.error, response.status));
    const data = decoded.value.data;
    if (response.status !== 200 || data.command_id !== commandId || data.listing_id !== listingId) {
      return err(writeOutcomeUnknown(invalidResponseError("admin hold receipt"), response.status));
    }
    if (data.listing_version !== expectedVersion + 1) {
      return err(writeOutcomeUnknown(invalidResponseError("admin hold version"), response.status));
    }
    return ok(
      data.disposition === "closed"
        ? {
            kind: "closed" as const,
            receipt: data as InferOutput<AdminSchemas["ConsignmentDelistReceiptV2Schema"]>,
          }
        : {
            kind: "hold" as const,
            receipt: data as InferOutput<AdminSchemas["ConsignmentDelistReceiptV2Schema"]>,
          },
    );
  };
}

/** 解除指定运营止售。 */
export async function prepareAdminReleaseHold(
  host: AdminSessionHost,
  listingId: string,
  expectedVersion: number,
  holdId: string,
): Promise<CommandOperation<InferOutput<AdminSchemas["ConsignmentHoldReceiptSchema"]>>> {
  const schemasResult = await schemas();
  if (!schemasResult.ok) {
    const failure = writeNotSent(schemasResult.error);
    return async () => err(failure);
  }
  const { ReleaseConsignmentHoldRequestSchema, ConsignmentHoldReceiptSchema } = schemasResult.value;
  const body = (commandId: string) => ({
    command_id: commandId,
    expected_version: expectedVersion,
    hold_id: holdId,
  });
  const prepare = prepareJsonCommandOnce(body);
  return async (commandId, signal) => {
    if (!is(ConsignmentUuidSchema, listingId) || !is(ConsignmentUuidSchema, holdId)) {
      return err(writeNotSent("invalid_request"));
    }
    const request = body(commandId);
    if (!is(ReleaseConsignmentHoldRequestSchema, request)) {
      return err(writeNotSent("invalid_request"));
    }
    const prepared = prepare(commandId);
    if (!prepared.ok) return err(prepared.error);
    const sent = await sendPreparedJsonWrite(
      host,
      {
        url: `${LISTINGS}/${listingId}/release-hold`,
        method: "POST",
        endpoint: "consignment-delist",
      },
      prepared.value,
      signal,
    );
    if (!sent.ok) return err(sent.error);
    const response = sent.value;
    const decoded = decodeAdminReceipt(
      response,
      strictStruct({
        code: literal("ok"),
        message: string(),
        data: ConsignmentHoldReceiptSchema,
      }),
      HOLD_POLICY,
    );
    if (!decoded.ok) return err(writeOutcomeUnknown(decoded.error, response.status));
    const receipt = decoded.value.data;
    return response.status === 200 &&
      receipt.command_id === commandId &&
      receipt.listing_id === listingId &&
      receipt.hold_id === holdId &&
      receipt.listing_version === expectedVersion + 1
      ? ok(receipt)
      : err(writeOutcomeUnknown(invalidResponseError("admin release hold"), response.status));
  };
}
