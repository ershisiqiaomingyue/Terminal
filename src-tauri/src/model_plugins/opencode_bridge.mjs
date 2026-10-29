// OpenCode account discovery plus the real DSH OpenCode Go catalog (MIT).
// Agent scheduling remains in Android Tool; this bridge only transports model requests.
import { createServer } from 'node:http';
import { randomBytes, timingSafeEqual } from 'node:crypto';
import { readFile, stat } from 'node:fs/promises';
import { homedir } from 'node:os';
import { join, resolve, extname } from 'node:path';
import { pathToFileURL } from 'node:url';
import { createInterface } from 'node:readline';
import { DatabaseSync } from 'node:sqlite';

const protocol = (npm) => ({ '@ai-sdk/anthropic': 'anthropic', '@ai-sdk/openai': 'open_ai_responses', '@ai-sdk/openai-compatible': 'open_ai_compatible' })[npm];
const MAX_BODY = 64 * 1024 * 1024;
const cleanError = (error) => String(error?.message || error).replace(/Bearer\s+\S+/gi, 'Bearer [已隐藏]').replace(/sk-[A-Za-z0-9_-]+/g, '[已隐藏]').slice(0, 400);

export async function readOpenCodeCredentials(override = '', env = process.env) {
  const data = env.XDG_DATA_HOME || join(env.USERPROFILE || homedir(), '.local', 'share');
  const root = join(data, 'opencode');
  const database = override ? (extname(override) === '.db' ? override : '') : join(root, 'opencode.db');
  if (database) {
    let db;
    try {
      await stat(database);
      db = new DatabaseSync(database, { readOnly: true });
      db.exec('PRAGMA busy_timeout=2000');
      const table = db.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name='credential'").get();
      if (table) {
        const columns = db.prepare('PRAGMA table_info(credential)').all().map((row) => row.name);
        const order = columns.includes('active') ? 'active DESC, time_created DESC, id DESC' : 'time_created DESC, id DESC';
        const result = {};
        for (const row of db.prepare(`SELECT integration_id, value FROM credential ORDER BY ${order}`).all()) {
          if (row.integration_id && !Object.hasOwn(result, row.integration_id)) result[row.integration_id] = JSON.parse(row.value);
        }
        return result; // The database is authoritative after migration; never resurrect signed-out legacy keys.
      }
    } catch (error) { if (error.code !== 'ENOENT') throw new Error('OpenCode 账号数据库无法读取，请检查文件路径或关闭占用后刷新'); }
    finally { db?.close(); }
  }
  try {
    const filename = override || join(root, 'auth.json');
    if ((await stat(filename)).size > 4 * 1024 * 1024) throw new Error('OpenCode 登录文件过大');
    const result = JSON.parse(await readFile(filename, 'utf8'));
    if (!result || typeof result !== 'object' || Array.isArray(result)) throw new Error('登录文件格式无效');
    return result;
  } catch (error) { if (error.code === 'ENOENT') return {}; throw new Error('OpenCode 登录文件无法读取，请检查路径与文件格式'); }
}

export function parseOpenCodeUsage(value) {
  return ['rolling', 'weekly', 'monthly'].map((key, index) => {
    const row = value?.usage?.[key];
    if (!row || !['ok', 'rate-limited'].includes(row.status) || !Number.isFinite(row.percent) || row.percent < 0 || row.percent > 100 || !Number.isFinite(Date.parse(row.resetsAt))) throw new Error('OpenCode Go 未返回有效额度数据');
    return { label: ['5 小时', '每周', '每月'][index], ...row };
  });
}

export async function createOpenCodeBridge({ api, config = {}, onSnapshot = () => {}, readCredentials = readOpenCodeCredentials, fetcher = fetch }) {
  const secret = randomBytes(32).toString('base64url');
  let runtimes = [], accounts = [], closed = false, refreshFlight;
  const active = new Set();
  const catalog = new api.OpencodeGoCatalog(api.DEFAULT_BASE_URL, 300_000, () => {}, () => {});
  const snapshot = () => ({ accounts, providers: runtimes.map((r) => ({
    id: r.id, name: r.name, brand: 'opencode', kind: r.kind,
    baseUrl: `${origin}/${r.lane}/v1`, model: r.models[0].id, models: r.models, token: secret,
  })) });
  const sameAccount = async (r) => {
    const current = (await readCredentials(config.authFileCn))[r.accountId];
    if (!['api', 'key'].includes(current?.type) || current.key !== r.key) throw new Error('OpenCode 账号已切换或退出，请刷新后重试');
  };
  const quota = async (providerId) => {
    const r = runtimes.find((item) => item.id === providerId);
    if (!r) throw new Error('OpenCode 账号不可用');
    if (r.accountId !== 'opencode-go') return { status: 'unsupported', source: 'opencode', message: 'OpenCode Zen 未提供可用余额接口，请在官方账户页面查看', checkedAt: String(Math.floor(Date.now()/1000)) };
    try {
      await sameAccount(r);
      const response = await fetcher(`${r.base}/usage`, { headers: { Authorization: `Bearer ${r.key}`, Accept: 'application/json' }, signal: AbortSignal.timeout(15_000), redirect: 'error' });
      if (!response.ok) throw new Error(`OpenCode Go 额度查询失败（HTTP ${response.status}）`);
      const windows = parseOpenCodeUsage(await response.json());
      await sameAccount(r);
      const message = windows.map((w) => `${w.label}已用 ${w.percent}%`).join(' · ');
      const account = accounts.find((a) => a.id === r.accountId);
      account.message = message; account.creditError = '';
      return { status: 'available', source: 'opencode-go', remainingAmount: 100 - Math.max(...windows.map((w) => w.percent)), unit: '%', message, extra: JSON.stringify(windows), checkedAt: String(Math.floor(Date.now()/1000)) };
    } catch (error) {
      const account = accounts.find((a) => a.id === r.accountId);
      if (account) account.creditError = cleanError(error);
      throw error;
    } finally { if (!closed) onSnapshot(snapshot()); }
  };
  async function refresh() {
    if (refreshFlight) return refreshFlight;
    refreshFlight = (async () => {
      const credentials = await readCredentials(config.authFileCn);
      const next = [], nextAccounts = [];
      let metadata;
      for (const accountId of ['opencode-go', 'opencode']) {
        const credential = credentials[accountId];
        const account = { id: accountId, name: accountId === 'opencode-go' ? 'OpenCode Go' : 'OpenCode Zen', state: 'signed-out', account: '', modelCount: 0, credits: null, unlimited: false, message: '请先在 OpenCode 登录对应账号', creditError: '' };
        nextAccounts.push(account);
        if (!credential) continue;
        if (!['api', 'key'].includes(credential.type) || !credential.key?.trim()) {
          account.state = 'error'; account.message = '此账号使用 OAuth，当前 Go 连接插件不能复用；请使用 OpenCode Go 的 API 登录'; continue;
        }
        account.state = 'signed-in'; account.account = '本机已登录';
        try {
          let models, base;
          if (accountId === 'opencode-go') {
            const result = await catalog.snapshot(true);
            if (!result.live) throw new Error('模型目录暂时无法同步，请检查网络后刷新');
            models = [...result.models.values()].map((m) => ({ id: m.id, name: m.name, protocol: ({'anthropic-messages':'anthropic','openai-responses':'open_ai_responses','openai-completions':'open_ai_compatible'})[m.api], contextWindow: m.contextWindow, supportsTools: true, supportsVision: m.input.includes('image'), supportsReasoning: m.reasoning, supportsUsage: true }));
            base = api.DEFAULT_BASE_URL;
          } else {
            const response = await fetcher('https://models.dev/api.json', {signal: AbortSignal.timeout(30_000)});
            if (!response.ok) throw new Error('OpenCode 模型元数据暂时不可用');
            metadata = (await response.json()).opencode;
            const parsed = new URL(metadata.api);
            if (parsed.protocol !== 'https:' || parsed.hostname !== 'opencode.ai') throw new Error('OpenCode 官方模型地址无效');
            base = metadata.api.replace(/\/$/, '');
            const listing = await fetcher(`${base}/models`, {headers:{Authorization:`Bearer ${credential.key}`},signal:AbortSignal.timeout(15_000),redirect:'error'});
            if (!listing.ok) throw new Error(`模型目录查询失败（HTTP ${listing.status}）`);
            const ids = api.readLiveModelIds(await listing.json());
            models = ids.flatMap((id) => {
              const m = metadata.models[id], kind = protocol(m?.provider?.npm || metadata.npm);
              return m && kind ? [{id, name:m.name || id, protocol:kind, contextWindow:m.limit?.context, supportsTools:m.tool_call, supportsVision:m.modalities?.input?.includes('image'), supportsReasoning:m.reasoning, supportsUsage:true}] : [];
            });
          }
          account.modelCount = models.length; account.message = models.length ? '模型目录已同步' : '账号没有可用模型';
          for (const kind of new Set(models.map((m) => m.protocol))) {
            const selected = models.filter((m) => m.protocol === kind);
            const lane = `${accountId}-${kind}`;
            next.push({ id:`plugin:opencode:${lane}`, name:account.name, lane, accountId, kind, models:selected, base, key:credential.key });
          }
        } catch (error) { account.state = 'error'; account.message = cleanError(error); }
      }
      if (closed) return snapshot();
      // Check again before publishing a catalog fetched for a previous account.
      const latest = await readCredentials(config.authFileCn);
      runtimes = next.filter((r) => latest[r.accountId]?.key === r.key);
      accounts = nextAccounts;
      for (const account of accounts) if (latest[account.id]?.key !== credentials[account.id]?.key) { account.state = 'signed-out'; account.modelCount = 0; account.message = '账号已切换，请刷新'; }
      await Promise.allSettled(runtimes.filter((r, i, all) => r.accountId === 'opencode-go' && all.findIndex((a) => a.accountId === r.accountId) === i).map((r) => quota(r.id)));
      onSnapshot(snapshot()); return snapshot();
    })().finally(() => { refreshFlight = undefined; });
    return refreshFlight;
  }
  const handle = async (req, res) => {
    const fail = (status, message) => { res.writeHead(status, {'Content-Type':'application/json'});res.end(JSON.stringify({error:{message}})); };
    try {
      const expected = Buffer.from(`Bearer ${secret}`), actual = Buffer.from(req.headers.authorization || req.headers['x-api-key'] || '');
      // Anthropic sends the local credential in x-api-key without Bearer.
      const candidate = req.headers['x-api-key'] ? Buffer.from(`Bearer ${req.headers['x-api-key']}`) : actual;
      if (!/^127\.0\.0\.1:\d+$/.test(req.headers.host || '') || req.headers.origin) return fail(403,'仅允许本机请求');
      if (candidate.length !== expected.length || !timingSafeEqual(candidate,expected)) return fail(401,'本地插件认证失败');
      const match = /^\/([^/]+)\/v1\/(models|chat\/completions|responses|messages)$/.exec(req.url || '');
      const r = runtimes.find((item) => item.lane === match?.[1]);
      if (!r) return fail(404,'插件接口不存在');
      if (match[2] === 'models' && req.method === 'GET') {
        await sameAccount(r);
        res.writeHead(200, {'Content-Type':'application/json'});
        return res.end(JSON.stringify({object:'list',data:r.models.map((m)=>({id:m.id,object:'model',owned_by:'opencode'}))}));
      }
      if (req.method !== 'POST') return fail(404,'插件接口不存在');
      const route = {anthropic:'messages',open_ai_responses:'responses',open_ai_compatible:'chat/completions'}[r.kind];
      if (match[2] !== route) return fail(400,'模型协议不匹配');
      const chunks=[];let size=0;
      for await (const chunk of req) { size += chunk.length;if(size>MAX_BODY)return fail(413,'消息体过大');chunks.push(chunk); }
      const body=JSON.parse(Buffer.concat(chunks).toString('utf8'));
      if (!r.models.some((m)=>m.id===body.model))return fail(400,'模型当前不可用');
      await sameAccount(r);
      const controller=new AbortController();active.add(controller);
      const disconnect=()=>{if(!res.writableEnded)controller.abort();};res.on('close',disconnect);
      try {
        const headers={'Content-Type':'application/json',Authorization:`Bearer ${r.key}`,'x-opencode-session':req.headers['x-android-tool-session'] || randomBytes(16).toString('hex')};
        if(r.kind==='anthropic')Object.assign(headers,{'x-api-key':r.key,'anthropic-version':'2023-06-01'});
        if(Array.isArray(body.messages))body.messages=body.messages.map((m)=>m.role==='developer'?{...m,role:'system'}:m);
        const upstream=await fetcher(`${r.base}/${route}`,{method:'POST',headers,body:JSON.stringify(body),signal:controller.signal,redirect:'error'});
        res.writeHead(upstream.status,{'Content-Type':upstream.headers.get('content-type') || 'application/json'});
        if(upstream.body)for await(const chunk of upstream.body){if(controller.signal.aborted)break;if(!res.write(chunk))await new Promise((done)=>{const resume=()=>{res.off('drain',resume);res.off('close',resume);done();};res.once('drain',resume);res.once('close',resume);});}
        res.end();
      } finally {active.delete(controller);res.off('close',disconnect);}
    } catch(error){if(!res.headersSent)fail(502,cleanError(error));else res.destroy();}
  };
  const server=createServer((req,res)=>{void handle(req,res);});
  await new Promise((done,reject)=>{server.once('error',reject);server.listen(0,'127.0.0.1',done);});
  const origin=`http://127.0.0.1:${server.address().port}`;
  await refresh();
  let previous=JSON.stringify(await readCredentials(config.authFileCn));
  const timer=setInterval(()=>{void (async()=>{const current=JSON.stringify(await readCredentials(config.authFileCn));if(current!==previous){previous=current;await refresh();}})().catch(()=>{});},30_000);timer.unref();
  return {snapshot,refresh,quota,async close(){closed=true;clearInterval(timer);for(const c of active)c.abort();await new Promise((done)=>{server.close(done);server.closeAllConnections();});}};
}

async function main(){
  const directory=resolve(process.argv[2]),config=JSON.parse(process.argv[4] || '{}');
  const send=(value)=>process.stdout.write(`${JSON.stringify(value)}\n`);
  const api=await import(pathToFileURL(join(directory,'node_modules/dsh-opencode-go/lib/index.js')).href);
  const bridge=await createOpenCodeBridge({api,config,onSnapshot:(data)=>send({type:'snapshot',data})});
  send({type:'ready'});
  const reader=createInterface({input:process.stdin});
  reader.on('line',(line)=>{void(async()=>{let request;try{request=JSON.parse(line);const data=request.method==='refresh'?await bridge.refresh():request.method==='quota'?await bridge.quota(request.providerId):bridge.snapshot();send({id:request.id,data});}catch(error){send({id:request?.id,error:cleanError(error)});}})();});
  reader.on('close',()=>{void bridge.close().finally(()=>process.exit(0));});
}
if(process.argv[1] && import.meta.url===pathToFileURL(resolve(process.argv[1])).href)main().catch(()=>{process.stderr.write('OpenCode 插件启动失败，请检查运行环境或登录文件\n');process.exit(1);});
