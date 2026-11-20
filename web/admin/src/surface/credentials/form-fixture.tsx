// 装配凭据导入表单所需的真实宿主上下文和重读世代。

import { createCell } from "@auction/web-shared/reactive/cell";
import type { JSX } from "@solidjs/web";
import type { AdminSessionHost } from "../../api/session";
import { AdminSessionHostContext } from "../auth/shell";
import { CredentialCreateForm } from "./form";

/** 在显式传入的会话宿主内挂载真实导入表单。 */
export function CredentialFormFixture(props: { readonly host: AdminSessionHost }): JSX.Element {
  const reload = createCell(0n);
  return (
    <AdminSessionHostContext value={props.host}>
      <CredentialCreateForm reloadGeneration={reload} />
    </AdminSessionHostContext>
  );
}
