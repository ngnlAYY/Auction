// 非敏感配置候选的逐项变更清单，按 Worker 热字段与重启字段展示实际差异。

import type { AdminJsonValue } from "@auction/contracts/dto/admin/rules";
import { structuralEquals } from "@auction/web-shared/reactive/equality";
import { compareText } from "@auction/web-shared/text";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createMemo, For, Show } from "solid-js";

import { ROOT as EARLY_BIRD_ROOT, defaultBlock as earlyBirdDefaultBlock } from "./early-bird";
import { isHot } from "./hot-fields";
import { controlLabel, fieldId } from "./validation";

/** 一项实际发生变化的配置路径及其前后值。 */
export interface ConfigChange {
  readonly path: string;
  readonly before: AdminJsonValue;
  readonly after: AdminJsonValue;
}

/** 非热字段的差异要等到 Worker 重启后才生效。 */
export function configChangeNeedsRestart(change: ConfigChange): boolean {
  return !isHot(change.path);
}

/** 按路径递归比较两份配置，返回按路径排序的差异。 */
export function configChanges(
  before: AdminJsonValue,
  after: AdminJsonValue,
): readonly ConfigChange[] {
  const changes: ConfigChange[] = [];
  collectChanges("", before, after, changes);
  return changes;
}

function collectChanges(
  path: string,
  before: AdminJsonValue,
  after: AdminJsonValue,
  changes: ConfigChange[],
): void {
  const previous = editableValue(path, before);
  const candidate = editableValue(path, after);
  if (structuralEquals(previous, candidate)) {
    return;
  }
  if (isJsonObject(previous) && isJsonObject(candidate)) {
    const keys = [...new Set([...Object.keys(previous), ...Object.keys(candidate)])].sort(
      compareText,
    );
    for (const key of keys) {
      const field = path === "" ? key : `${path}.${key}`;
      collectChanges(field, previous[key] ?? null, candidate[key] ?? null, changes);
    }
  } else {
    changes.push({ path, before: previous, after: candidate });
  }
}

function editableValue(path: string, value: AdminJsonValue): AdminJsonValue {
  // 物理文档省略空的 bot_players，表单则把它展示为空列表。
  if (path === "bot_players" && Array.isArray(value) && value.length === 0) {
    return null;
  }
  // 第一次编辑早鸟字段会补齐旧文档缺省的区块。评审以表单展示的默认值作为比较基线。
  if (path === EARLY_BIRD_ROOT && (value === null || isJsonObject(value))) {
    const block: Record<string, AdminJsonValue> = {};
    for (const [key, item] of Object.entries(earlyBirdDefaultBlock())) {
      if (isAdminJsonValue(item)) {
        block[key] = item;
      }
    }
    if (isJsonObject(value)) {
      Object.assign(block, value);
    }
    return block;
  }
  return value;
}

function fieldLabel(path: string): string {
  if (typeof document !== "undefined") {
    const field = document.getElementById(fieldId(path));
    if (field !== null) {
      return controlLabel(field, path);
    }
  }
  if (path === "dashboard_mailboxes") {
    return t("admin-runtime-section-mailboxes");
  }
  return path;
}

function displayValue(value: AdminJsonValue): string {
  if (value === true) {
    return t("admin-runtime-enabled");
  }
  if (value === false) {
    return t("admin-runtime-disabled");
  }
  if (typeof value === "string") {
    return value === "" ? t("admin-runtime-review-empty") : value;
  }
  if (value === null) {
    return t("admin-runtime-review-empty");
  }
  return rustJsonText(value);
}

// serde_json 默认以 BTreeMap 保存对象，`Value::to_string()` 的键按字典序输出。
// 展示用的自由 JSON 也按同一顺序序列化，避免浏览器与 Rust 的差异清单显示不同文本。
function rustJsonText(value: AdminJsonValue): string {
  if (Array.isArray(value)) {
    return `[${value.map(rustJsonText).join(",")}]`;
  }
  if (isJsonObject(value)) {
    const fields = Object.keys(value)
      .sort(compareText)
      .map((key) => `${JSON.stringify(key)}:${rustJsonText(value[key] ?? null)}`);
    return `{${fields.join(",")}}`;
  }
  const text = JSON.stringify(value);
  return typeof text === "string" ? text : "null";
}

function isJsonObject(value: AdminJsonValue): value is Readonly<Record<string, AdminJsonValue>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isAdminJsonValue(value: unknown): value is AdminJsonValue {
  if (value === null || typeof value === "string" || typeof value === "boolean") {
    return true;
  }
  if (typeof value === "number") {
    return Number.isFinite(value);
  }
  if (Array.isArray(value)) {
    return value.every(isAdminJsonValue);
  }
  if (typeof value === "object") {
    return Object.values(value).every(isAdminJsonValue);
  }
  return false;
}

/** 变更清单：先列热读字段，再列必须重启才生效的字段。 */
export function ConfigChangeList(props: {
  readonly changes: readonly ConfigChange[];
  readonly pendingRestart: boolean;
}): JSX.Element {
  const partitioned = createMemo(() => {
    const restart: ConfigChange[] = [];
    const hot: ConfigChange[] = [];
    for (const change of props.changes) {
      (configChangeNeedsRestart(change) ? restart : hot).push(change);
    }
    return { restart, hot };
  });
  const hot = () => partitioned().hot;
  const restart = () => partitioned().restart;
  return (
    <>
      <p class="admin-panel__note">{t("admin-runtime-review-impact")}</p>
      <Show when={hot().length > 0}>
        <h3>{t("admin-runtime-review-hot")}</h3>
        <p class="admin-panel__note">{t("admin-runtime-hot-field-title")}</p>
        <Show when={props.pendingRestart}>
          <p class="admin-panel__note">{t("admin-runtime-review-pending-restart")}</p>
        </Show>
        <ChangeRows changes={hot()} />
      </Show>
      <Show when={restart().length > 0}>
        <h3>{t("admin-runtime-review-restart")}</h3>
        <Show when={restart().some((change) => change.path.startsWith("equipment_colors"))}>
          <p class="admin-panel__note">{t("admin-runtime-review-colors-impact")}</p>
        </Show>
        <Show when={restart().some((change) => change.path.startsWith("operations.economics."))}>
          <p class="admin-panel__note">{t("admin-runtime-review-economics-impact")}</p>
        </Show>
        <ChangeRows changes={restart()} />
      </Show>
    </>
  );
}

function ChangeRows(props: { readonly changes: readonly ConfigChange[] }): JSX.Element {
  return (
    <dl class="admin-config-changes">
      <For each={props.changes} keyed={(change) => change.path}>
        {(change) => (
          <div class="admin-config-changes__entry">
            <dt>{fieldLabel(change().path)}</dt>
            <dd>
              <span class="admin-config-changes__label">{t("admin-runtime-review-before")}</span>
              <pre>{displayValue(change().before)}</pre>
            </dd>
            <dd>
              <span class="admin-config-changes__label">{t("admin-runtime-review-after")}</span>
              <pre>{displayValue(change().after)}</pre>
            </dd>
          </div>
        )}
      </For>
    </dl>
  );
}
