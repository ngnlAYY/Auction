// 论坛装备名称标色规则编辑器。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import {
  type RuntimeConfigForm,
  runtimeConfigFormAddEquipmentColor,
  runtimeConfigFormEquipmentColors,
  runtimeConfigFormEquipmentTaxonomy,
  runtimeConfigFormMoveEquipmentColor,
  runtimeConfigFormRemoveEquipmentColor,
  runtimeConfigFormSetEquipmentColorField,
  runtimeEquipmentColorFormLegacyEquipmentType,
} from "../../model";
import { runtimeConfigFormSourceColorLevel } from "../../model/document";
import { COLOR_LEVEL_MAXIMUM, COLOR_LEVEL_MINIMUM, colorLevelPath } from "../../validation";
import { NumberInput } from "../number";
import { equipmentKindsFor, prefixesFor, suffixesFor } from "./options";
import { ColorMultiSelect } from "./selection";

/** 论坛装备名称标色规则的可编辑列表。 */
export function EquipmentColorRulesField(props: {
  readonly form: Cell<RuntimeConfigForm>;
  readonly disabled: boolean;
  readonly onDirty: () => void;
}): JSX.Element {
  const taxonomy = createValueMemo(() => runtimeConfigFormEquipmentTaxonomy(props.form()));
  const rules = createValueMemo(() => runtimeConfigFormEquipmentColors(props.form()));
  const lastIndex = createValueMemo(() => Math.max(rules().length - 1, 0));
  return (
    <Show when={taxonomy()} fallback={<p role="alert">{t("admin-runtime-invalid-form")}</p>}>
      {(currentTaxonomy) => (
        <div class="admin-runtime-color-rules">
          <p class="admin-runtime-section-note">{t("admin-runtime-equipment-colors-note")}</p>
          <For each={rules()} keyed={(rule) => `color-rule-${rule.rowId}`}>
            {(rule, index) => (
              <div class="admin-runtime-color-rule">
                <div class="admin-runtime-color-rule__head">
                  <strong>{t("admin-runtime-color-rule", { number: index() + 1 })}</strong>
                  <div class="admin-runtime-color-rule__actions">
                    <button
                      type="button"
                      class="admin-btn-secondary"
                      disabled={props.disabled || index() === 0}
                      onClick={() => {
                        props.form.update((current) =>
                          runtimeConfigFormMoveEquipmentColor(current, index(), -1),
                        );
                        props.onDirty();
                      }}
                    >
                      {t("admin-runtime-color-up")}
                    </button>
                    <button
                      type="button"
                      class="admin-btn-secondary"
                      disabled={props.disabled || index() >= lastIndex()}
                      onClick={() => {
                        props.form.update((current) =>
                          runtimeConfigFormMoveEquipmentColor(current, index(), 1),
                        );
                        props.onDirty();
                      }}
                    >
                      {t("admin-runtime-color-down")}
                    </button>
                    <button
                      type="button"
                      class="admin-btn-secondary"
                      disabled={props.disabled}
                      onClick={() => {
                        props.form.update((current) =>
                          runtimeConfigFormRemoveEquipmentColor(current, index()),
                        );
                        props.onDirty();
                      }}
                    >
                      {t("admin-runtime-color-remove")}
                    </button>
                  </div>
                </div>
                <div class="admin-runtime-field-grid">
                  <ColorMultiSelect
                    form={props.form}
                    index={index()}
                    field="category"
                    label={t("admin-runtime-color-category")}
                    options={currentTaxonomy().categories}
                    value={rule().category.text}
                    conflictingSingle={rule().category.conflictingSingle}
                    allowAny={true}
                    disabled={props.disabled}
                    onDirty={props.onDirty}
                  />
                  <ColorMultiSelect
                    form={props.form}
                    index={index()}
                    field="equipment_type"
                    label={t("admin-runtime-color-type")}
                    options={equipmentKindsFor(currentTaxonomy(), rule().category.text)}
                    value={rule().equipmentType.text}
                    materialNames={currentTaxonomy().materials.map(
                      (option: { readonly name: string }) => option.name,
                    )}
                    legacyExactType={runtimeEquipmentColorFormLegacyEquipmentType(rule())}
                    conflictingSingle={rule().equipmentType.conflictingSingle}
                    allowAny={true}
                    disabled={props.disabled}
                    onDirty={props.onDirty}
                  />
                  <ColorMultiSelect
                    form={props.form}
                    index={index()}
                    field="quality"
                    label={t("admin-runtime-color-quality")}
                    options={currentTaxonomy().qualities.map(
                      (option: { readonly name: string }) => option.name,
                    )}
                    value={rule().quality.text}
                    conflictingSingle={rule().quality.conflictingSingle}
                    allowAny={true}
                    disabled={props.disabled}
                    onDirty={props.onDirty}
                  />
                  <ColorLevelField
                    form={props.form}
                    index={index()}
                    rowId={rule().rowId}
                    bound="min"
                    label={t("admin-runtime-color-level-min")}
                    value={rule().levelMin}
                    disabled={props.disabled}
                    onDirty={props.onDirty}
                  />
                  <ColorLevelField
                    form={props.form}
                    index={index()}
                    rowId={rule().rowId}
                    bound="max"
                    label={t("admin-runtime-color-level-max")}
                    value={rule().levelMax}
                    disabled={props.disabled}
                    onDirty={props.onDirty}
                  />
                </div>
                <div class="admin-runtime-field-grid admin-runtime-field-grid--lists">
                  <ColorMultiSelect
                    form={props.form}
                    index={index()}
                    field="prefixes"
                    label={t("admin-runtime-color-prefixes")}
                    options={prefixesFor(
                      currentTaxonomy(),
                      rule().category.text,
                      rule().equipmentType.text,
                    )}
                    value={rule().prefixes}
                    disabled={props.disabled}
                    onDirty={props.onDirty}
                  />
                  <ColorMultiSelect
                    form={props.form}
                    index={index()}
                    field="suffixes"
                    label={t("admin-runtime-color-suffixes")}
                    options={suffixesFor(
                      currentTaxonomy(),
                      rule().category.text,
                      rule().equipmentType.text,
                    )}
                    value={rule().suffixes}
                    disabled={props.disabled}
                    onDirty={props.onDirty}
                  />
                </div>
                <div class="admin-runtime-field-grid">
                  <label class="admin-runtime-field">
                    <span>{t("admin-runtime-color-prefix")}</span>
                    <input
                      type="color"
                      disabled={props.disabled}
                      value={rule().prefixColor}
                      onInput={(event) => {
                        props.form.update((current) =>
                          runtimeConfigFormSetEquipmentColorField(
                            current,
                            index(),
                            "prefix_color",
                            event.currentTarget.value,
                          ),
                        );
                        props.onDirty();
                      }}
                    />
                  </label>
                  <label class="admin-runtime-field">
                    <span>{t("admin-runtime-color-suffix")}</span>
                    <input
                      type="color"
                      disabled={props.disabled}
                      value={rule().suffixColor}
                      onInput={(event) => {
                        props.form.update((current) =>
                          runtimeConfigFormSetEquipmentColorField(
                            current,
                            index(),
                            "suffix_color",
                            event.currentTarget.value,
                          ),
                        );
                        props.onDirty();
                      }}
                    />
                  </label>
                  <label class="admin-runtime-field">
                    <span>{t("admin-runtime-color-combined")}</span>
                    <input
                      type="color"
                      disabled={props.disabled}
                      value={rule().combinedColor}
                      onInput={(event) => {
                        props.form.update((current) =>
                          runtimeConfigFormSetEquipmentColorField(
                            current,
                            index(),
                            "combined_color",
                            event.currentTarget.value,
                          ),
                        );
                        props.onDirty();
                      }}
                    />
                  </label>
                </div>
              </div>
            )}
          </For>
          <button
            type="button"
            class="admin-btn-secondary"
            disabled={props.disabled || rules().length >= 200}
            onClick={() => {
              props.form.update((current) => runtimeConfigFormAddEquipmentColor(current));
              props.onDirty();
            }}
          >
            {t("admin-runtime-color-add")}
          </button>
        </div>
      )}
    </Show>
  );
}

/** 单条规则的等级数值输入；`original` 保留文档中的原始数值，供控件沿袭历史错误提示。 */
function ColorLevelField(props: {
  readonly form: Cell<RuntimeConfigForm>;
  readonly index: number;
  readonly rowId: number;
  readonly bound: "min" | "max";
  readonly label: string;
  readonly value: string;
  readonly disabled: boolean;
  readonly onDirty: () => void;
}): JSX.Element {
  const original = createValueMemo(() => {
    const value = runtimeConfigFormSourceColorLevel(props.form(), props.index, props.bound);
    return value === null ? null : value.toString();
  });
  const field = createValueMemo(() => (props.bound === "min" ? "level_min" : "level_max"));
  return (
    <NumberInput
      path={colorLevelPath(props.rowId, props.bound)}
      label={props.label}
      minimum={COLOR_LEVEL_MINIMUM}
      maximum={COLOR_LEVEL_MAXIMUM}
      original={original()}
      value={props.value}
      disabled={props.disabled}
      onInput={(value: string) => {
        props.form.update((current) =>
          runtimeConfigFormSetEquipmentColorField(current, props.index, field(), value),
        );
        props.onDirty();
      }}
    />
  );
}
