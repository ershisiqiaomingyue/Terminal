export function perfettoDurationSeconds(value?: string): number {
  if (value === undefined || !value.trim()) return 10;
  const duration = Number(value);
  if (!Number.isInteger(duration) || duration < 1 || duration > 300) {
    throw new Error("Perfetto 采集时长必须为 1 到 300 秒的整数");
  }
  return duration;
}
