// 在真实命令宿主下挂载凭据页，或单独驱动凭据行的读世代与卸载。

import { createCell } from "@auction/web-shared/reactive/cell";
import { ok, type Result } from "@auction/web-shared/result";
import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";
import type { AdminCredential } from "../../api/credentials";
import type { AdminSessionHost } from "../../api/session";
import { AdminSessionHostContext } from "../auth/shell";
import { useAdminPagination, useVersionedAdminPage } from "../pagination";
import {
  PendingCommandHostContext,
  type PendingCommands,
  PendingCommandsContext,
  usePendingCommandHost,
} from "../pending";
import { PendingCommandNotices } from "../pending/panel";
import { createCommandReload, usePendingCommands } from "../pending/sources";
import { page as CredentialsPage } from "./page";
import { CredentialRow } from "./table";

/** 选择整页入口，或按 Rust 的版本读取夹具驱动一条真实凭据行。 */
export interface CredentialTableFixtureProps {
  readonly host: AdminSessionHost;
  readonly row?: {
    readonly initial: AdminCredential;
    readonly read: () => Promise<Result<AdminCredential, string>>;
    readonly onSubmitWithReceipt?: (entity: string, action: string) => void;
  };
}

/** 挂载命令回执与凭据组件，使移除行或重读列表时外层会话保持存活。 */
export function CredentialTableFixture(props: CredentialTableFixtureProps): JSX.Element {
  const pending = usePendingCommandHost(props.host);
  return (
    <AdminSessionHostContext value={props.host}>
      <PendingCommandHostContext value={pending}>
        <PendingCommandNotices host={pending} />
        {props.row === undefined ? <CredentialsPage /> : <CredentialRowSource {...props.row} />}
      </PendingCommandHostContext>
    </AdminSessionHostContext>
  );
}

function CredentialRowSource(props: NonNullable<CredentialTableFixtureProps["row"]>): JSX.Element {
  const reload = createCommandReload();
  const owner = usePendingCommands(reload, (entity) => entity.startsWith("credential:"));
  const observedOwner: PendingCommands = {
    ...owner,
    submitWithReceipt(entity, action, operation, onSuccess, onFailure) {
      // 在宿主检查实体是否已有命令前记录调用，随后仍由真实宿主执行。
      props.onSubmitWithReceipt?.(entity, action);
      return owner.submitWithReceipt(entity, action, operation, onSuccess, onFailure);
    },
  };
  const pagination = useAdminPagination("credential-version-test", ["page_size"], null, 100);
  const rows = useVersionedAdminPage(
    pagination,
    () => Number(reload()),
    () => 1,
    () => (reload.peek() === 0n ? Promise.resolve(ok(props.initial)) : props.read()),
  );
  const row = () => {
    const current = rows();
    return current?.ok === true ? current.value : null;
  };
  const visible = createCell(true);
  return (
    <PendingCommandsContext value={observedOwner}>
      <button type="button" onClick={() => reload.set(1n)}>
        使凭据投影失效
      </button>
      <button type="button" onClick={() => visible.set(false)}>
        凭据翻页
      </button>
      <Show when={visible() ? row() : null}>
        {(current) => (
          <table>
            <tbody>
              <CredentialRow
                credential={current()[1]}
                loadedGeneration={BigInt(current()[0])}
                reloadGeneration={reload}
                onDelete={() => {}}
              />
            </tbody>
          </table>
        )}
      </Show>
      <Show when={owner.anyBusy()}>
        <p>已经发送凭据写入</p>
      </Show>
    </PendingCommandsContext>
  );
}
