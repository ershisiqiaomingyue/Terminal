export function browserAttachmentInput(filename: string, dataUrl: string) {
  const match = /^data:([^;,]*)(;base64)?,([\s\S]*)$/.exec(dataUrl);
  if (!match) throw new Error(`附件“${filename}”的数据格式无效，请重新添加。`);
  const data = match[2]
    ? Array.from(atob(match[3]), (character) => character.charCodeAt(0))
    : Array.from(new TextEncoder().encode(decodeURIComponent(match[3])));
  return { filename, mediaType: match[1] || "application/octet-stream", data };
}
