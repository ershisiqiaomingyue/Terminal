// Observe npm's HTTP response bodies without consuming streams, changing cache
// behavior, or recording URLs/credentials. Loaded only in the npm process.
const fs = require('node:fs');
const { performance } = require('node:perf_hooks');
const target = process.env.ANDROID_TOOL_NPM_PROGRESS;
if (target) {
  let downloadedBytes = 0, completedRequests = 0, activeRequests = 0;
  const samples = [{ time: performance.now(), bytes: 0 }];
  const write = () => {
    const now = performance.now();
    while (samples.length > 1 && samples[1].time <= now - 1000) samples.shift();
    const bytesPerSecond = (downloadedBytes - samples[0].bytes) / Math.max((now - samples[0].time) / 1000, 0.001);
    samples.push({ time: now, bytes: downloadedBytes });
    try {
      fs.writeFileSync(target, JSON.stringify({ downloadedBytes, bytesPerSecond, completedRequests, activeRequests }));
    } catch { /* Telemetry must never fail an installation. */ }
  };
  for (const protocol of [require('node:http'), require('node:https')]) {
    const request = protocol.request;
    protocol.request = function (...args) {
      const pending = Reflect.apply(request, this, args);
      pending.prependOnceListener('response', response => {
        activeRequests++;
        const push = response.push;
        response.push = function (chunk, encoding) {
          if (chunk !== null) downloadedBytes += typeof chunk === 'string' ? Buffer.byteLength(chunk, encoding) : chunk.length;
          return Reflect.apply(push, this, [chunk, encoding]);
        };
        response.once('close', () => {
          activeRequests--;
          if (response.complete) completedRequests++;
        });
      });
      return pending;
    };
  }
  const timer = setInterval(write, 250);
  timer.unref();
  write();
  process.once('exit', write);
}
