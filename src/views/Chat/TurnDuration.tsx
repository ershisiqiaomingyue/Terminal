import { useEffect, useState } from "react";
import type { ChatMessage } from "./types";

function formatTurnDuration(startedAt: number, finishedAt: number) {
  const seconds = Math.max(0, Math.floor((finishedAt - startedAt) / 1_000));
  if (seconds < 1) return "<1秒";
  const hours = Math.floor(seconds / 3_600);
  const minutes = Math.floor((seconds % 3_600) / 60);
  const remainder = seconds % 60;
  if (hours) return `${hours}小时 ${minutes}分钟 ${remainder}秒`;
  if (minutes) return `${minutes}分钟 ${remainder}秒`;
  return `${remainder}秒`;
}

export function TurnDuration({ message }: { message: ChatMessage }) {
  const [now, setNow] = useState(() => Date.now());
  const running = Boolean(message.pending && !message.finishedAt);

  useEffect(() => {
    if (!running) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [running]);

  if (!message.startedAt) return null;
  const endAt = running ? now : message.finishedAt;
  if (!endAt) return <div className="message-duration" aria-label="已处理">已处理</div>;
  const elapsed = formatTurnDuration(message.startedAt, endAt);
  return <div className="message-duration" aria-label={running ? `正在处理 ${elapsed}` : `已处理 ${elapsed}`}>
    {running ? `正在处理 ${elapsed}` : `已处理 ${elapsed}`}
  </div>;
}
