// 文件操作请求体的构造规则。抽成纯函数是为了能被单元测试覆盖：
// 曾经这里只给 rename 传 newName，新建文件/文件夹传的是 null，
// 于是后端 mutation_name() 直接返回「请输入新名称」，新建功能整体不可用。
export type MutationActionName = "rename" | "delete" | "move" | "create-file" | "create-directory";

export type MutationRequestInput = {
  root: string;
  path: string;
  action: MutationActionName;
  name: string;
  recursive: boolean;
  overwrite: boolean;
  expectedVersion?: string | null;
  acknowledged?: boolean;
  destinationDirectory?: string | null;
};

/** 重命名与新建需要把输入框里的名字一起发给后端；删除与移动不需要。 */
export function actionNeedsName(action: MutationActionName) {
  return action === "rename" || action === "create-file" || action === "create-directory";
}

export function buildMutationRequest(input: MutationRequestInput) {
  return {
    root: input.root,
    path: input.path,
    action: input.action,
    newName: actionNeedsName(input.action) ? input.name : null,
    recursive: input.recursive,
    expectedVersion: input.expectedVersion ?? null,
    acknowledged: input.acknowledged ?? false,
    destinationDirectory: input.destinationDirectory ?? null,
    overwrite: input.overwrite,
  };
}
