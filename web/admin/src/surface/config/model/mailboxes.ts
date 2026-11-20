// 仪表盘邮箱编辑行的不可变更新。

import type { RuntimeConfigForm } from "./index";

export const MAX_DASHBOARD_MAILBOXES = 2;

/** 仪表盘邮箱行的编辑状态。 */
export interface RuntimeMailboxForm {
  readonly worldId: string;
  readonly mailboxKey: string;
}

export function runtimeConfigFormMailboxes(form: RuntimeConfigForm): readonly RuntimeMailboxForm[] {
  return form.mailboxes;
}

export function runtimeConfigFormAddMailbox(form: RuntimeConfigForm): RuntimeConfigForm {
  if (form.mailboxes.length >= MAX_DASHBOARD_MAILBOXES) {
    return form;
  }
  let worldId: string | null = null;
  for (const world of ["main", "isekai"]) {
    if (form.mailboxes.every((mailbox) => mailbox.worldId !== world)) {
      worldId = world;
      break;
    }
  }
  const world = worldId ?? "main";
  return {
    ...form,
    mailboxes: [...form.mailboxes, { worldId: world, mailboxKey: `mooglemail:${world}` }],
  };
}

export function runtimeConfigFormRemoveMailbox(
  form: RuntimeConfigForm,
  index: number,
): RuntimeConfigForm {
  if (index < 0 || index >= form.mailboxes.length) {
    return form;
  }
  return { ...form, mailboxes: form.mailboxes.filter((_, position) => position !== index) };
}

export function runtimeConfigFormSetMailboxWorld(
  form: RuntimeConfigForm,
  index: number,
  value: string,
): RuntimeConfigForm {
  if (index < 0 || index >= form.mailboxes.length) {
    return form;
  }
  return {
    ...form,
    mailboxes: form.mailboxes.map((mailbox, position) =>
      position === index ? { ...mailbox, worldId: value } : mailbox,
    ),
  };
}

export function runtimeConfigFormSetMailboxKey(
  form: RuntimeConfigForm,
  index: number,
  value: string,
): RuntimeConfigForm {
  if (index < 0 || index >= form.mailboxes.length) {
    return form;
  }
  return {
    ...form,
    mailboxes: form.mailboxes.map((mailbox, position) =>
      position === index ? { ...mailbox, mailboxKey: value } : mailbox,
    ),
  };
}
