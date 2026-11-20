// 对照 Rust 的编辑模型，核对原始 JSON、显式修复和缺省字段的回传。

import { expect, test } from "bun:test";
import type { AdminJsonValue } from "@auction/contracts/dto/admin/rules";
import { required } from "../../solid-dom-fixture";
import {
  accepted,
  decodeConfigData,
  decodeConfigProjection,
  defaultProjection,
  jsonObject,
  objectAt,
  valueAt,
} from "../config-projection-fixture.test";
import {
  LIST_FIELDS as OPERATIONAL_LIST_FIELDS,
  SIGNED_NUMBER_FIELDS as OPERATIONAL_SIGNED_NUMBER_FIELDS,
  TEXT_FIELDS as OPERATIONAL_TEXT_FIELDS,
  UNSIGNED_NUMBER_FIELDS as OPERATIONAL_UNSIGNED_NUMBER_FIELDS,
} from "../operational-fields";
import { runtimeConfigFormToDocument } from "./document";
import {
  LIST_FIELDS,
  type RuntimeConfigForm,
  runtimeConfigFormEditedInput,
  runtimeConfigFormEditedMailboxesExceedLimit,
  runtimeConfigFormEquipmentColors,
  runtimeConfigFormFromResponse,
  runtimeConfigFormInput,
  runtimeConfigFormRemoveMailbox,
  runtimeConfigFormResolveEquipmentColorSelection,
  runtimeConfigFormSetEquipmentColorField,
  runtimeConfigFormSourceDocument,
  runtimeEquipmentColorFormLegacyEquipmentType,
  SIGNED_NUMBER_FIELDS,
  runtimeConfigFormSetBoolean as setBoolean,
  runtimeConfigFormSetInput as setInput,
  TEXT_FIELDS,
  UNSIGNED_NUMBER_FIELDS,
} from "./index";

function setMissing(root: Record<string, unknown>, path: string, value: AdminJsonValue): void {
  const segments = path.split(".");
  let current = root;
  for (const segment of segments.slice(0, -1)) {
    current[segment] ??= {};
    current = jsonObject(current[segment]);
  }
  const key = required(segments.at(-1), "字段名");
  current[key] ??= value;
}

function fixtureRule() {
  return {
    category: "Cloth",
    equipment_type: "Phase Robe",
    quality: "Legendary",
    level: { min: 0, max: 500 },
    prefixes: ["Radiant"],
    suffixes: [],
    prefix_color: "#FF0000",
    suffix_color: "#00aa00",
    combined_color: "#aa00aa",
  };
}

function fixtureDocument(): Record<string, unknown> {
  const source = {
    metrics: { enabled: false },
    dashboard_mailboxes: [],
    security: {
      allowed_origins_enabled: false,
      allowed_origins: ["", " https://example.invalid ", "\n", ""],
    },
    operations: { schedule: { managed: false, enabled: false } },
    equipment_colors: { rules: [fixtureRule()] },
  };
  for (const path of [...TEXT_FIELDS, ...OPERATIONAL_TEXT_FIELDS]) {
    setMissing(source, path, "");
  }
  for (const path of [...LIST_FIELDS, ...OPERATIONAL_LIST_FIELDS]) {
    if (path !== "bot_players") {
      setMissing(source, path, []);
    }
  }
  for (const path of [
    ...UNSIGNED_NUMBER_FIELDS,
    ...SIGNED_NUMBER_FIELDS,
    ...OPERATIONAL_SIGNED_NUMBER_FIELDS,
    ...OPERATIONAL_UNSIGNED_NUMBER_FIELDS,
  ]) {
    setMissing(source, path, 0);
  }
  return source;
}

function form(document: Record<string, unknown>): RuntimeConfigForm {
  const response = decodeConfigData({
    document,
    fingerprint: `sha256:${"0".repeat(64)}`,
    restart_required: false,
    equipment_taxonomy: {
      revision: "test",
      categories: [],
      qualities: [],
      kinds: [],
      materials: [],
    },
  });
  return accepted(runtimeConfigFormFromResponse(response));
}

function toDocument(current: RuntimeConfigForm): unknown {
  return accepted(runtimeConfigFormToDocument(current));
}

test("unrelated_edit_preserves_original_json_and_does_not_create_optional_fields", () => {
  const source = fixtureDocument();
  let current = form(source);
  current = setInput(current, "security.login_rate_limit", "10");
  current = setBoolean(current, "metrics.enabled", true);
  expect<unknown>(runtimeConfigFormSourceDocument(current)).toEqual(source);
  const expected = structuredClone(source);
  objectAt(expected, "security").login_rate_limit = 10;
  objectAt(expected, "metrics").enabled = true;
  expect(toDocument(current)).toEqual(expected);
});

test("clearing_edited_origins_replaces_the_original_blank_entry", () => {
  const source = fixtureDocument();
  objectAt(source, "security").allowed_origins = [""];
  let current = form(source);
  expect(runtimeConfigFormInput(current, "security.allowed_origins")).toBe("");
  expect(toDocument(current)).toEqual(source);
  current = setInput(current, "security.allowed_origins", "https://example.invalid");
  current = setInput(current, "security.allowed_origins", "");
  expect(valueAt(toDocument(current), "security.allowed_origins")).toEqual([]);
});

test("listener_mode_round_trip_preserves_the_original_address_on_unrelated_edit", () => {
  const source = fixtureDocument();
  const listener = objectAt(source, "listener");
  listener.mode = "tcp";
  listener.address = "[0:0:0:0:0:0:0:1]:8080";
  let current = form(source);
  current = setInput(current, "listener.mode", "unix");
  expect(valueAt(toDocument(current), "listener.address")).toBe("");
  current = setInput(current, "listener.mode", "tcp");
  current = setBoolean(current, "metrics.enabled", true);
  const expected = structuredClone(source);
  objectAt(expected, "metrics").enabled = true;
  expect(toDocument(current)).toEqual(expected);
  current = setInput(current, "listener.address", "127.0.0.1:8080");
  current = setInput(current, "listener.mode", "unix");
  current = setInput(current, "listener.mode", "tcp");
  expect(valueAt(toDocument(current), "listener.address")).toBe("127.0.0.1:8080");
});

test("restoring_unix_mode_preserves_the_original_socket_fields", () => {
  const source = fixtureDocument();
  source.listener = {
    mode: "unix",
    address: "",
    group: "auction",
    socket_path: "/run/hv-auction/admin.sock",
    runtime_directory: "/run/hv-auction",
  };
  let current = form(source);
  current = setInput(current, "listener.mode", "tcp");
  for (const field of ["group", "socket_path", "runtime_directory"]) {
    expect(valueAt(toDocument(current), `listener.${field}`)).toBe("");
  }
  current = setInput(current, "listener.mode", "unix");
  current = setBoolean(current, "metrics.enabled", true);
  objectAt(source, "metrics").enabled = true;
  expect(toDocument(current)).toEqual(source);
});

test("editor_opens_and_preserves_values_pending_server_validation", () => {
  const projection = defaultProjection();
  const document = projection.data.document;
  objectAt(document, "security").trusted_proxy_cidrs = ["bad-cidr"];
  objectAt(document, "equipment_source").allowed_path_prefixes = ["/equip/../"];
  objectAt(document, "tracing").queue_capacity = 0;
  let current = accepted(runtimeConfigFormFromResponse(decodeConfigProjection(projection)));
  current = setBoolean(current, "metrics.enabled", true);
  expect(runtimeConfigFormEditedInput(current, "tracing.queue_capacity")).toBeNull();
  const expected = structuredClone(document);
  objectAt(expected, "metrics").enabled = true;
  expect(toDocument(current)).toEqual(expected);
  current = setInput(current, "tracing.queue_capacity", "256");
  expect(runtimeConfigFormEditedInput(current, "tracing.queue_capacity")).toBe("256");
});

test("edited_arrays_and_color_fields_override_only_their_values", () => {
  const source = fixtureDocument();
  jsonObject(valueAt(source, "equipment_colors.rules.0")).prefixes = [" Radiant ", ""];
  let current = form(source);
  current = setInput(
    current,
    "security.allowed_origins",
    " https://one.invalid\n\nhttps://two.invalid ",
  );
  current = runtimeConfigFormSetEquipmentColorField(current, 0, "level_max", "600");
  const expected = structuredClone(source);
  objectAt(expected, "security").allowed_origins = ["https://one.invalid", "https://two.invalid"];
  jsonObject(valueAt(expected, "equipment_colors.rules.0.level")).max = 600;
  expect(toDocument(current)).toEqual(expected);
});

test("color_conflicts_change_representation_only_after_an_explicit_choice", () => {
  const source = fixtureDocument();
  const rule = jsonObject(valueAt(source, "equipment_colors.rules.0"));
  rule.categories = ["Light"];
  rule.equipment_types = ["Cotton"];
  rule.qualities = ["Legendary"];
  let current = form(source);
  current = setBoolean(current, "metrics.enabled", true);
  objectAt(source, "metrics").enabled = true;
  expect(toDocument(current)).toEqual(source);
  current = runtimeConfigFormResolveEquipmentColorSelection(current, 0, "category", false);
  current = runtimeConfigFormResolveEquipmentColorSelection(current, 0, "equipment_type", true);
  current = runtimeConfigFormResolveEquipmentColorSelection(current, 0, "quality", false);
  rule.category = "";
  delete rule.equipment_types;
  rule.quality = "";
  expect(toDocument(current)).toEqual(source);
});

test("excess_mailboxes_can_be_removed_without_rewriting_the_remaining_rows", () => {
  const source = fixtureDocument();
  const rows = [
    { world_id: "main", mailbox_key: "main-box" },
    { world_id: "isekai", mailbox_key: "isekai-box" },
    { world_id: "main", mailbox_key: "extra-1" },
    { world_id: "isekai", mailbox_key: "extra-2" },
  ];
  source.dashboard_mailboxes = rows;
  let current = form(source);
  expect(runtimeConfigFormEditedMailboxesExceedLimit(current)).toBe(false);
  expect(toDocument(current)).toEqual(source);
  current = runtimeConfigFormRemoveMailbox(current, 3);
  expect(runtimeConfigFormEditedMailboxesExceedLimit(current)).toBe(true);
  current = runtimeConfigFormRemoveMailbox(current, 2);
  expect(runtimeConfigFormEditedMailboxesExceedLimit(current)).toBe(false);
  source.dashboard_mailboxes = rows.slice(0, 2);
  expect(toDocument(current)).toEqual(source);
});

test("restoring_a_legacy_material_keeps_its_original_spelling_and_exact_match", () => {
  const projection = defaultProjection();
  objectAt(projection.data.document, "equipment_colors").rules = [
    { ...fixtureRule(), equipment_type: "phase" },
  ];
  let current = accepted(runtimeConfigFormFromResponse(decodeConfigProjection(projection)));
  current = runtimeConfigFormSetEquipmentColorField(current, 0, "equipment_type", "Cotton");
  expect(
    runtimeEquipmentColorFormLegacyEquipmentType(
      required(runtimeConfigFormEquipmentColors(current)[0], "颜色规则"),
    ),
  ).toBeNull();
  const updated = jsonObject(valueAt(toDocument(current), "equipment_colors.rules.0"));
  expect(updated.equipment_type).toBe("");
  expect(updated.equipment_types).toEqual(["Cotton"]);
  current = runtimeConfigFormSetEquipmentColorField(current, 0, "equipment_type", "");
  current = runtimeConfigFormSetEquipmentColorField(current, 0, "equipment_type", "Phase");
  expect(
    runtimeEquipmentColorFormLegacyEquipmentType(
      required(runtimeConfigFormEquipmentColors(current)[0], "颜色规则"),
    ),
  ).toBe("phase");
  expect(toDocument(current)).toEqual(projection.data.document);
});

test("turning_off_management_discards_all_nested_edits", () => {
  for (const initiallyManaged of [true, false]) {
    const source = fixtureDocument();
    objectAt(source, "operations.schedule").managed = initiallyManaged;
    let current = form(source);
    current = setBoolean(current, "operations.schedule.managed", true);
    current = setBoolean(current, "operations.schedule.enabled", true);
    current = setBoolean(current, "operations.schedule.early_bird.enabled", true);
    current = setInput(current, "operations.schedule.preview_time", "18:00");
    current = setInput(current, "operations.schedule.worlds", "main");
    current = setInput(current, "operations.schedule.early_bird.live_minutes", "");
    expect(runtimeConfigFormToDocument(current).ok).toBe(false);
    current = setBoolean(current, "operations.schedule.managed", false);
    const expected = structuredClone(source);
    objectAt(expected, "operations.schedule").managed = false;
    expect(toDocument(current)).toEqual(expected);
    current = setBoolean(current, "operations.schedule.managed", true);
    objectAt(expected, "operations.schedule").managed = true;
    expect(toDocument(current)).toEqual(expected);
  }
});

test("managed_group_submits_its_edits_and_preserves_early_bird_defaults", () => {
  let current = form(fixtureDocument());
  current = setBoolean(current, "operations.schedule.managed", true);
  current = setBoolean(current, "operations.schedule.enabled", true);
  current = setBoolean(current, "operations.schedule.early_bird.enabled", true);
  current = setInput(current, "operations.schedule.early_bird.live_minutes", "360");
  const document = toDocument(current);
  expect(valueAt(document, "operations.schedule.enabled")).toBe(true);
  const bird = objectAt(document, "operations.schedule.early_bird");
  expect(bird.enabled).toBe(true);
  expect(bird.live_minutes).toBe(360);
  expect(bird.session_days).toEqual([1, 3, 5, 7]);
});
