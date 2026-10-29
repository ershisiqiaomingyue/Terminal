export type DownloadAccess = { elevated: boolean; sudoPassword?: string };
export type DownloadInfo = { name: string; bytes: number; requiresConfirmation: boolean };
export const DOWNLOAD_PERMISSION_CODE = "[ssh_download_permission_required]";

export function downloadNeedsAuthorization(error: unknown): boolean {
  return String(error).includes(DOWNLOAD_PERMISSION_CODE);
}

export function downloadErrorMessage(error: string): string {
  return error.replace(DOWNLOAD_PERMISSION_CODE, "").trim();
}

/** One explicit grant for this download; denial and elevated failures never loop. */
export async function inspectAuthorizedDownload(
  inspect: (access: DownloadAccess) => Promise<DownloadInfo>,
  authorize: () => Promise<DownloadAccess | null>,
  permissionRequired = false,
): Promise<{ info: DownloadInfo; access: DownloadAccess } | null> {
  let access: DownloadAccess = { elevated: false };
  if (!permissionRequired) {
    try { return { info: await inspect(access), access }; }
    catch (error) { if (!downloadNeedsAuthorization(error)) throw error; }
  }
  const granted = await authorize();
  if (!granted) return null;
  access = granted;
  try { return { info: await inspect(access), access }; }
  catch (error) { access.sudoPassword = undefined; throw error; }
}
