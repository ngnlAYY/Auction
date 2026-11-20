// 运行配置表单的文本输入与布尔接管编辑。

import { boolean as earlyBirdBoolean } from "../early-bird";
import { managementPath } from "../operational-fields";
import type { RuntimeConfigForm } from "./index";
import { nestedValueFound } from "./nested";

export function runtimeConfigFormInput(form: RuntimeConfigForm, path: string): string {
  return recordString(form.inputEdits, path) ?? recordString(form.inputs, path) ?? "";
}

export function runtimeConfigFormSourceInput(form: RuntimeConfigForm, path: string): string | null {
  return recordString(form.inputs, path);
}

export function runtimeConfigFormSetInput(
  form: RuntimeConfigForm,
  path: string,
  value: string,
): RuntimeConfigForm {
  if (!runtimeConfigFormEditsField(form, path)) {
    return form;
  }
  // 非空原始数组也可能回填为空文本。只有用户编辑后清空才写入空数组。
  const origins = nestedValueFound(form.document, path);
  const clearsOrigins =
    path === "security.allowed_origins" &&
    value === "" &&
    origins.ok &&
    Array.isArray(origins.value) &&
    origins.value.length > 0;
  const inputEdits =
    !clearsOrigins && recordString(form.inputs, path) === value
      ? withoutKey(form.inputEdits, path)
      : { ...form.inputEdits, [path]: value };
  return { ...form, inputEdits };
}

export function runtimeConfigFormBoolean(form: RuntimeConfigForm, path: string): boolean {
  return recordBoolean(form.booleanEdits, path) ?? sourceBoolean(form, path);
}

export function runtimeConfigFormSetBoolean(
  form: RuntimeConfigForm,
  path: string,
  value: boolean,
): RuntimeConfigForm {
  if (!runtimeConfigFormEditsField(form, path)) {
    return form;
  }
  let inputEdits = form.inputEdits;
  let booleanEdits = form.booleanEdits;
  // 关闭接管即丢弃组内未保存的输入，之后重新接管也从原始值开始。
  if (!value && managementPath(path) === path) {
    inputEdits = filterRecord(inputEdits, (field) => managementPath(field) !== path);
    booleanEdits = filterRecord(booleanEdits, (field) => managementPath(field) !== path);
  }
  booleanEdits =
    value === sourceBoolean(form, path)
      ? withoutKey(booleanEdits, path)
      : { ...booleanEdits, [path]: value };
  return { ...form, inputEdits, booleanEdits };
}

export function runtimeConfigFormEditedInput(form: RuntimeConfigForm, path: string): string | null {
  return runtimeConfigFormEditsField(form, path) ? recordString(form.inputEdits, path) : null;
}

/** 未接管的分组既不提交草稿输入，也不参与本地字段错误摘要。 */
export function runtimeConfigFormEditsField(form: RuntimeConfigForm, path: string): boolean {
  const managed = managementPath(path);
  return managed === null || path === managed || runtimeConfigFormBoolean(form, managed);
}

function sourceBoolean(form: RuntimeConfigForm, path: string): boolean {
  const earlyBird = earlyBirdBoolean(form.document, path);
  if (earlyBird !== null) {
    return earlyBird;
  }
  const value = nestedValueFound(form.document, path);
  return value.ok && value.value === true;
}

function recordString(record: Readonly<Record<string, string>>, key: string): string | null {
  return Object.hasOwn(record, key) ? (record[key] ?? null) : null;
}

function recordBoolean(record: Readonly<Record<string, boolean>>, key: string): boolean | null {
  return Object.hasOwn(record, key) ? (record[key] ?? null) : null;
}

function withoutKey<T>(record: Readonly<Record<string, T>>, key: string): Record<string, T> {
  const next: Record<string, T> = { ...record };
  delete next[key];
  return next;
}

function filterRecord<T>(
  record: Readonly<Record<string, T>>,
  keep: (key: string) => boolean,
): Record<string, T> {
  const next: Record<string, T> = {};
  for (const key of Object.keys(record)) {
    const value = record[key];
    if (value !== undefined && keep(key)) {
      next[key] = value;
    }
  }
  return next;
}
