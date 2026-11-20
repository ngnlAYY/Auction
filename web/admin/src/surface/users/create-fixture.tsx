// 真实创建对话框、未决面板和回执对话框共用的来源页夹具。

import { createCell } from "@auction/web-shared/reactive/cell";
import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";
import type { AdminSessionHost } from "../../api/session";
import { AdminSessionHostContext } from "../auth/shell";
import {
  PendingCommandHostContext,
  PendingCommandsContext,
  usePendingCommandGate,
  usePendingCommandHost,
} from "../pending";
import { PendingCommandPanel } from "../pending/panel";
import { createCommandReload, usePendingCommandsWithGate } from "../pending/sources";
import { AdminUserCreateForm, AdminUserGeneratedKeyDialog } from "./create";

/** 保留来源页、会话和门禁，在测试时单独卸载创建对话框。 */
export function UserCreateFixture(props: { readonly host: AdminSessionHost }): JSX.Element {
  const pending = usePendingCommandHost(props.host);
  return (
    <AdminSessionHostContext value={props.host}>
      <PendingCommandHostContext value={pending}>
        <UserCreateSource />
      </PendingCommandHostContext>
    </AdminSessionHostContext>
  );
}

function UserCreateSource(): JSX.Element {
  const open = createCell(true);
  const notice = createCell<string | null>(null);
  const reload = createCommandReload();
  const gate = usePendingCommandGate();
  const owner = usePendingCommandsWithGate(reload, gate, () => true);
  return (
    <PendingCommandsContext value={owner}>
      <button type="button" onClick={() => open.set(false)}>
        关闭创建框
      </button>
      <button type="button" disabled={gate.blocked()}>
        开始下一次
      </button>
      <Show when={open()}>
        <AdminUserCreateForm gate={gate} open={open} notice={notice} />
      </Show>
      <Show when={notice()}>{(name) => <p>{name()}</p>}</Show>
      <PendingCommandPanel owner={owner} />
      <Show when={owner.generatedKey()}>
        {(generated) => (
          <AdminUserGeneratedKeyDialog
            generated={generated().key}
            onClose={() => owner.acknowledgeKey(generated().commandId)}
          />
        )}
      </Show>
    </PendingCommandsContext>
  );
}
