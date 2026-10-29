/**
 * 会话完成的提示音。
 *
 * Codex 没有内置音效：社区清一色是在 notify hook 里借 macOS 的系统音
 * （完成用 `Glass`，等待审批用 `Sosumi`）。Windows 没有同名资源可调，
 * 所以这里用 Web Audio 现场合成一段同类型的「叮—咚」：
 * 两个正弦音上行五度，各自带指数衰减包络，避免出现方波的刺耳感。
 *
 * 不引入音频文件是有意的：省一次网络/资源加载，也不给打包多一份资产。
 */

/** 两个音的频率（A5 → E6，纯五度上行）。 */
const NOTES = [880, 1318.51] as const;
/** 每个音的时长（秒）。 */
const NOTE_SECONDS = 0.18;
/** 起始音量。WebView2 里再大就会破音。 */
const PEAK_GAIN = 0.12;
/** 两音之间的间隔（秒）。 */
const NOTE_GAP = 0.13;

let context: AudioContext | null = null;

/** 复用同一个 AudioContext：浏览器对同时存在的上下文数量有上限。 */
function audioContext(): AudioContext | null {
  if (typeof window === "undefined") return null;
  if (context) return context;
  const Ctor = window.AudioContext
    ?? (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
  if (!Ctor) return null;
  try {
    context = new Ctor();
  } catch {
    return null;
  }
  return context;
}

/**
 * 播放完成提示音。
 *
 * 调用方不需要 catch：自动播放策略会拦下没有用户手势的 `resume()`，
 * 那种情况下静默没有声音即可，弹窗本身已经把信息传达出去了。
 */
export function playCompletionChime(): void {
  const ctx = audioContext();
  if (!ctx) return;
  const play = () => {
    const start = ctx.currentTime + 0.02;
    NOTES.forEach((frequency, index) => {
      const at = start + index * (NOTE_SECONDS + NOTE_GAP);
      const oscillator = ctx.createOscillator();
      const gain = ctx.createGain();
      oscillator.type = "sine";
      oscillator.frequency.setValueAtTime(frequency, at);
      // 头 8ms 起音，其余指数衰减，听起来像被轻敲一下。
      gain.gain.setValueAtTime(0.0001, at);
      gain.gain.exponentialRampToValueAtTime(PEAK_GAIN, at + 0.008);
      gain.gain.exponentialRampToValueAtTime(0.0001, at + NOTE_SECONDS);
      oscillator.connect(gain).connect(ctx.destination);
      oscillator.start(at);
      oscillator.stop(at + NOTE_SECONDS + 0.02);
    });
  };
  if (ctx.state === "running") {
    play();
    return;
  }
  void ctx.resume().then(
    () => { if (ctx.state === "running") play(); },
    () => undefined,
  );
}
