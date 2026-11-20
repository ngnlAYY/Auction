// 运行时配置页面的安全表单状态与完整 JSON 投影转换。

import type { EquipmentTaxonomyDto } from "@auction/contracts/dto/admin/equipment";
import type {
  AdminJsonValue,
  AdminRuntimeConfigResponse,
} from "@auction/contracts/dto/admin/rules";
import { structuralEquals } from "@auction/web-shared/reactive/equality";
import { err, ok, type Result } from "@auction/web-shared/result";
import { compareText } from "@auction/web-shared/text";

import { MAX_JSON_SAFE_INTEGER } from "../../../api/validation";
import { inputs as earlyBirdInputs } from "../early-bird";
import {
  LIST_FIELDS as OPERATIONAL_LIST_FIELDS,
  SIGNED_NUMBER_FIELDS as OPERATIONAL_SIGNED_NUMBER_FIELDS,
  TEXT_FIELDS as OPERATIONAL_TEXT_FIELDS,
  UNSIGNED_NUMBER_FIELDS as OPERATIONAL_UNSIGNED_NUMBER_FIELDS,
} from "../operational-fields";
import { runtimeConfigFormToDocument } from "./document";
import type { RuntimeEquipmentColorForm } from "./equipment-colors";
import type { RuntimeMailboxForm } from "./mailboxes";
import {
  colorSelectionFromSource,
  legacyMaterialSelection,
  listText,
  nestedValueFound,
} from "./nested";

export {
  runtimeConfigFormEditedMailboxesExceedLimit,
  runtimeConfigFormSourceColorLevel,
} from "./document";

export {
  type RuntimeEquipmentColorForm,
  runtimeConfigFormAddEquipmentColor,
  runtimeConfigFormEquipmentColors,
  runtimeConfigFormMoveEquipmentColor,
  runtimeConfigFormRemoveEquipmentColor,
  runtimeConfigFormResolveEquipmentColorSelection,
  runtimeConfigFormSetEquipmentColorField,
  runtimeEquipmentColorFormLegacyEquipmentType,
} from "./equipment-colors";
export {
  runtimeConfigFormBoolean,
  runtimeConfigFormEditedInput,
  runtimeConfigFormEditsField,
  runtimeConfigFormInput,
  runtimeConfigFormSetBoolean,
  runtimeConfigFormSetInput,
  runtimeConfigFormSourceInput,
} from "./inputs";
export {
  MAX_DASHBOARD_MAILBOXES,
  type RuntimeMailboxForm,
  runtimeConfigFormAddMailbox,
  runtimeConfigFormMailboxes,
  runtimeConfigFormRemoveMailbox,
  runtimeConfigFormSetMailboxKey,
  runtimeConfigFormSetMailboxWorld,
} from "./mailboxes";

export const TEXT_FIELDS: readonly string[] = [
  "listener.address",
  "listener.group",
  "listener.mode",
  "listener.runtime_directory",
  "listener.socket_path",
  "tracing.endpoint",
  "worker_control.socket_path",
];

export const LIST_FIELDS: readonly string[] = [
  "bot_players",
  "worlds.main.sender_allowlist",
  "worlds.isekai.sender_allowlist",
  "equipment_filter.qualities",
  "equipment_filter.prefixes",
  "equipment_filter.suffixes",
  "equipment_filter.categories",
  "equipment_filter.equipment_types",
  "equipment_source.allowed_hosts",
  "equipment_source.allowed_path_prefixes",
  "security.allowed_origins",
  "security.trusted_proxy_cidrs",
];

export const UNSIGNED_NUMBER_FIELDS: readonly string[] = [
  "equipment_source.maximum_response_bytes",
  "equipment_source.minimum_request_interval_millis",
  "equipment_source.request_timeout_seconds",
  "security.login_rate_limit",
  "security.mutation_rate_limit",
  "security.rate_limit_window_seconds",
  "security.request_timeout_seconds",
  "session.idle_ttl_seconds",
  "session.absolute_ttl_seconds",
  "session.max_public_sessions",
  "tracing.batch_timeout_seconds",
  "tracing.export_timeout_seconds",
  "tracing.max_export_batch_size",
  "tracing.queue_capacity",
  "tracing.sample_ratio_ppm",
  "tracing.shutdown_timeout_seconds",
];

export const SIGNED_NUMBER_FIELDS: readonly string[] = [
  "equipment_filter.level.min",
  "equipment_filter.level.max",
];

/** 递归只读视图；词典由当前表单共享，写入必须经模型更新函数。 */
export type DeepReadonly<T> = T extends string | number | boolean | null | undefined
  ? T
  : T extends readonly (infer Item)[]
    ? readonly DeepReadonly<Item>[]
    : { readonly [Key in keyof T]: DeepReadonly<T[Key]> };

/** 页面只读装备词典；`revision` 与候选数组都由模型独占更新。 */
export type RuntimeConfigEquipmentTaxonomy = DeepReadonly<EquipmentTaxonomyDto>;

/** 页面保留服务端的完整安全投影，并只把可编辑标量转成适合浏览器输入的文本。 */
export interface RuntimeConfigForm {
  readonly document: AdminJsonValue;
  readonly inputs: Readonly<Record<string, string>>;
  readonly inputEdits: Readonly<Record<string, string>>;
  readonly booleanEdits: Readonly<Record<string, boolean>>;
  readonly mailboxes: readonly RuntimeMailboxForm[];
  readonly equipmentColors: readonly RuntimeEquipmentColorForm[];
  readonly nextColorRowId: number;
  readonly hadEquipmentColors: boolean;
  readonly equipmentTaxonomy: RuntimeConfigEquipmentTaxonomy | null;
}

/** 空表单；文档为 JSON `null`，所有编辑集合为空。 */
export function runtimeConfigFormDefault(): RuntimeConfigForm {
  return {
    document: null,
    inputs: {},
    inputEdits: {},
    booleanEdits: {},
    mailboxes: [],
    equipmentColors: [],
    nextColorRowId: 0,
    hadEquipmentColors: false,
    equipmentTaxonomy: null,
  };
}

/** 读取编辑开始时的完整非敏感投影，供变更清单比对；不参与服务端授权。 */
export function runtimeConfigFormSourceDocument(form: RuntimeConfigForm): AdminJsonValue {
  return form.document;
}

/** 词典由同次响应单独下发，既不并入业务文档，也不参与其 fingerprint。 */
export function runtimeConfigFormFromResponse(
  response: AdminRuntimeConfigResponse,
): Result<RuntimeConfigForm, null> {
  const taxonomy = response.equipment_taxonomy ?? null;
  if (taxonomy === null) {
    return err(null);
  }
  const clonedTaxonomy = cloneEquipmentTaxonomy(taxonomy);
  if (clonedTaxonomy === null) {
    return err(null);
  }
  const responseDocument = response.document;
  if (!isJsonValue(responseDocument)) {
    return err(null);
  }
  const clonedDocument = cloneJsonDocument(responseDocument);
  if (!clonedDocument.ok) {
    return err(null);
  }
  const document = clonedDocument.value;
  const inputs: Record<string, string> = {};
  for (const path of [...TEXT_FIELDS, ...OPERATIONAL_TEXT_FIELDS]) {
    const found = nestedValueFound(document, path);
    if (!found.ok || typeof found.value !== "string") {
      return err(null);
    }
    inputs[path] = found.value;
  }
  for (const path of [...LIST_FIELDS, ...OPERATIONAL_LIST_FIELDS]) {
    const found = nestedValueFound(document, path);
    if (found.ok) {
      const text = listText(found.value);
      if (!text.ok) {
        return err(null);
      }
      inputs[path] = text.value;
      continue;
    }
    // bot_players 对旧文档缺省时按空列表展示，其余列表仍必须是显式数组。
    if (path === "bot_players") {
      inputs[path] = "";
      continue;
    }
    return err(null);
  }
  // 服务端投影已补齐邮件区块的默认值；任何数字缺失都是响应损坏，不在浏览器补默认值。
  for (const path of [...UNSIGNED_NUMBER_FIELDS, ...OPERATIONAL_UNSIGNED_NUMBER_FIELDS]) {
    const found = nestedValueFound(document, path);
    const value = safeU64(found.ok ? found.value : null);
    if (value === null) {
      return err(null);
    }
    inputs[path] = value.toString();
  }
  for (const path of [...SIGNED_NUMBER_FIELDS, ...OPERATIONAL_SIGNED_NUMBER_FIELDS]) {
    const found = nestedValueFound(document, path);
    const value = safeI64(found.ok ? found.value : null);
    if (value === null) {
      return err(null);
    }
    inputs[path] = value.toString();
  }
  // 旧文档缺省早鸟区块时按默认值展示；未改动不会写回，保持原指纹。
  const earlyBird = earlyBirdInputs(document);
  if (!earlyBird.ok) {
    return err(null);
  }
  for (const [path, value] of earlyBird.value) {
    inputs[path] = value;
  }
  const rawMailboxes = nestedValueFound(document, "dashboard_mailboxes");
  if (!rawMailboxes.ok || !Array.isArray(rawMailboxes.value)) {
    return err(null);
  }
  const mailboxes: RuntimeMailboxForm[] = [];
  for (const raw of rawMailboxes.value) {
    if (!isJsonObject(raw)) {
      return err(null);
    }
    const worldId = objectString(raw, "world_id");
    const mailboxKey = objectString(raw, "mailbox_key");
    if (worldId === null || mailboxKey === null) {
      return err(null);
    }
    mailboxes.push({ worldId, mailboxKey });
  }
  const hadEquipmentColors = nestedValueFound(document, "equipment_colors").ok;
  const rules = equipmentColorRules(document);
  const equipmentColors: RuntimeEquipmentColorForm[] = [];
  if (rules !== null) {
    for (let index = 0; index < rules.length; index += 1) {
      const raw = rules[index];
      if (raw === undefined) {
        return err(null);
      }
      const parsed = equipmentColorFromSource(raw, index, clonedTaxonomy);
      if (!parsed.ok) {
        return err(null);
      }
      equipmentColors.push(parsed.value);
    }
  }
  return ok({
    document,
    inputs,
    inputEdits: {},
    booleanEdits: {},
    mailboxes,
    equipmentColors,
    nextColorRowId: equipmentColors.length,
    hadEquipmentColors,
    equipmentTaxonomy: clonedTaxonomy,
  });
}

export function runtimeConfigFormEquipmentTaxonomy(
  form: RuntimeConfigForm,
): RuntimeConfigEquipmentTaxonomy | null {
  return form.equipmentTaxonomy;
}

/** 刷新候选元数据不改草稿或旧单值标记；历史值始终保留到用户明确编辑。 */
export function runtimeConfigFormRefreshEquipmentTaxonomy(
  form: RuntimeConfigForm,
  taxonomy: EquipmentTaxonomyDto,
): RuntimeConfigForm {
  const cloned = cloneEquipmentTaxonomy(taxonomy);
  if (cloned === null) {
    return form;
  }
  const resolvedKinds = [...cloned.resolved_kinds];
  const previous = form.equipmentTaxonomy;
  if (previous !== null && previous.revision === cloned.revision) {
    // GET 只描述服务端文档；冲突草稿仍可能引用服务端已移除的有限种类。
    // 同版本仅保留草稿仍选中的既有权威候选，新响应同名优先，不在浏览器推断语法。
    for (const kind of previous.resolved_kinds) {
      const selected = form.equipmentColors.some((rule) =>
        rawLines(rule.equipmentType.text).some((value) => eqIgnoreAsciiCase(value, kind.name)),
      );
      if (
        selected &&
        !resolvedKinds.some((current) => eqIgnoreAsciiCase(current.name, kind.name))
      ) {
        resolvedKinds.push(copyResolvedKind(kind));
      }
    }
  }
  const refreshed: EquipmentTaxonomyDto = { ...cloned, resolved_kinds: resolvedKinds };
  if (previous !== null && structuralEquals(previous, refreshed)) {
    return form;
  }
  return { ...form, equipmentTaxonomy: refreshed };
}

/** 服务端词典独立演进不构成待保存修改，非法数字等原始编辑中间态仍参与比较。 */
export function runtimeConfigFormSameEditsAs(
  left: RuntimeConfigForm,
  right: RuntimeConfigForm,
): boolean {
  return (
    structuralEquals(left.document, right.document) &&
    structuralEquals(left.inputEdits, right.inputEdits) &&
    structuralEquals(left.booleanEdits, right.booleanEdits) &&
    structuralEquals(left.mailboxes, right.mailboxes) &&
    structuralEquals(left.equipmentColors, right.equipmentColors) &&
    left.hadEquipmentColors === right.hadEquipmentColors
  );
}

export function runtimeConfigFormSignature(form: RuntimeConfigForm): Result<string, null> {
  const document = runtimeConfigFormToDocument(form);
  if (!document.ok) {
    return err(null);
  }
  try {
    const text = JSON.stringify(canonicalJson(document.value));
    return typeof text === "string" ? ok(text) : err(null);
  } catch {
    // 不可序列化的值只可能来自装配错误，不把异常交给整页失败路径。
    return err(null);
  }
}

function equipmentColorRules(document: AdminJsonValue): readonly AdminJsonValue[] | null {
  const found = nestedValueFound(document, "equipment_colors");
  if (!found.ok || !isJsonObject(found.value) || !Object.hasOwn(found.value, "rules")) {
    return null;
  }
  const rules = found.value.rules;
  return Array.isArray(rules) ? rules : null;
}

function equipmentColorFromSource(
  raw: AdminJsonValue,
  index: number,
  taxonomy: EquipmentTaxonomyDto,
): Result<RuntimeEquipmentColorForm, null> {
  if (!isJsonObject(raw)) {
    return err(null);
  }
  const category = colorSelectionFromSource(raw, "categories", "category");
  if (!category.ok) {
    return err(null);
  }
  const equipmentType = colorSelectionFromSource(raw, "equipment_types", "equipment_type");
  if (!equipmentType.ok) {
    return err(null);
  }
  const quality = colorSelectionFromSource(raw, "qualities", "quality");
  if (!quality.ok) {
    return err(null);
  }
  const prefixValue = nestedValueFound(raw, "prefixes");
  const suffixValue = nestedValueFound(raw, "suffixes");
  const prefixes = listText(prefixValue.ok ? prefixValue.value : null);
  const suffixes = listText(suffixValue.ok ? suffixValue.value : null);
  if (!prefixes.ok || !suffixes.ok) {
    return err(null);
  }
  const prefixColor = objectString(raw, "prefix_color");
  const suffixColor = objectString(raw, "suffix_color");
  const combinedColor = objectString(raw, "combined_color");
  if (prefixColor === null || suffixColor === null || combinedColor === null) {
    return err(null);
  }
  return ok({
    sourceIndex: index,
    rowId: index,
    category: category.value,
    equipmentType: equipmentType.value,
    legacyEquipmentType: legacyMaterialSelection(raw, taxonomy),
    quality: quality.value,
    levelMin: sourceLevel(raw, "min").toString(),
    levelMax: sourceLevel(raw, "max").toString(),
    prefixes: prefixes.value,
    suffixes: suffixes.value,
    prefixColor,
    suffixColor,
    combinedColor,
  });
}

function sourceLevel(object: Readonly<Record<string, AdminJsonValue>>, bound: string): number {
  const level = object.level;
  if (!isJsonObject(level) || !Object.hasOwn(level, bound)) {
    return 0;
  }
  const value = level[bound];
  return typeof value === "number" && isSafeI64(value) ? value : 0;
}

function safeU64(value: AdminJsonValue | null): number | null {
  return typeof value === "number" &&
    Number.isSafeInteger(value) &&
    !Object.is(value, -0) &&
    value >= 0 &&
    value <= MAX_JSON_SAFE_INTEGER
    ? value
    : null;
}

function safeI64(value: AdminJsonValue | null): number | null {
  return typeof value === "number" && isSafeI64(value) ? value : null;
}

function isSafeI64(value: number): boolean {
  return (
    Number.isSafeInteger(value) && !Object.is(value, -0) && Math.abs(value) <= MAX_JSON_SAFE_INTEGER
  );
}

/** 按 Rust `BTreeMap` 的 UTF-8 字节序递归排列对象键；数组顺序保持不变。 */
function canonicalJson(value: AdminJsonValue): AdminJsonValue {
  if (Array.isArray(value)) {
    return value.map(canonicalJson);
  }
  if (!isJsonObject(value)) {
    return value;
  }
  const entries: [string, AdminJsonValue][] = [];
  for (const key of Object.keys(value).sort(compareText)) {
    const child = value[key];
    if (child !== undefined) {
      entries.push([key, canonicalJson(child)]);
    }
  }
  return Object.fromEntries(entries);
}

/** 保留旧词典中的有限种类；深拷贝避免刷新后的表单仍与上一次响应共享引用。 */
function copyResolvedKind(
  kind: RuntimeConfigEquipmentTaxonomy["resolved_kinds"][number],
): EquipmentTaxonomyDto["resolved_kinds"][number] {
  return {
    name: kind.name,
    category: kind.category,
    material: kind.material,
    slot: kind.slot,
    prefixes: [...kind.prefixes],
    suffixes: [...kind.suffixes],
  };
}

/** 表单独占候选文档副本；克隆失败按响应损坏拒绝，不交给整页失败路径。 */
function cloneJsonDocument(document: AdminJsonValue): Result<AdminJsonValue, null> {
  try {
    const cloned: unknown = structuredClone(document);
    return isJsonValue(cloned) ? ok(cloned) : err(null);
  } catch {
    return err(null);
  }
}

/** 表单独占词典副本；刷新入参之后被修改不得影响已载入的草稿。 */
function cloneEquipmentTaxonomy(taxonomy: EquipmentTaxonomyDto): EquipmentTaxonomyDto | null {
  try {
    return structuredClone(taxonomy);
  } catch {
    return null;
  }
}

function objectString(
  object: Readonly<Record<string, AdminJsonValue>>,
  key: string,
): string | null {
  if (!Object.hasOwn(object, key)) {
    return null;
  }
  const value = object[key];
  return typeof value === "string" ? value : null;
}

function isJsonObject(value: unknown): value is Readonly<Record<string, AdminJsonValue>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** 传输层已按 JSON 解析；这里只把 `unknown` 收窄成可递归编辑的文档类型。 */
function isJsonValue(value: unknown): value is AdminJsonValue {
  if (
    value === null ||
    typeof value === "string" ||
    typeof value === "boolean" ||
    typeof value === "number"
  ) {
    return true;
  }
  if (Array.isArray(value)) {
    return value.every(isJsonValue);
  }
  return isJsonObject(value) && Object.values(value).every(isJsonValue);
}

/** Rust `str::lines`，不做空白修剪；用于旧材质名称的逐行比较。 */
function rawLines(value: string): string[] {
  const result: string[] = [];
  let start = 0;
  for (let index = 0; index < value.length; index += 1) {
    if (value.charAt(index) !== "\n") {
      continue;
    }
    let line = value.slice(start, index);
    if (line.endsWith("\r")) {
      line = line.slice(0, -1);
    }
    result.push(line);
    start = index + 1;
  }
  if (start < value.length) {
    result.push(value.slice(start));
  }
  return result;
}

/** `eq_ignore_ascii_case` 只折叠 ASCII 大小写；非 ASCII 字符必须完全相同。 */
function eqIgnoreAsciiCase(left: string, right: string): boolean {
  if (left.length !== right.length) {
    return false;
  }
  for (let index = 0; index < left.length; index += 1) {
    if (asciiLower(left.charCodeAt(index)) !== asciiLower(right.charCodeAt(index))) {
      return false;
    }
  }
  return true;
}

function asciiLower(code: number): number {
  return code >= 0x41 && code <= 0x5a ? code + 0x20 : code;
}
