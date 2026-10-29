import { createRequire } from 'node:module';
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { resolve, join, sep } from 'node:path';
import { build } from 'esbuild';
import { parse } from 'yaml';

// Package discovery and bundling are identical for every DSH plugin.
const [directory, packageName] = process.argv.slice(2);
const require = createRequire(join(directory, 'package.json'));
const root = join(directory, 'node_modules', ...packageName.split('/'));
const pkg = JSON.parse(await readFile(join(root, 'package.json'), 'utf8'));
const settingsInline = pkg.androidTool?.settingsPresentation === 'inline';
if (!pkg.dsh && !pkg.androidTool) throw new Error('该包未声明插件入口（dsh / androidTool）');
let rows = [{ id: pkg.name, name: pkg.name }];
if (pkg.dsh?.bundle?.patch) {
  const patchPath = resolve(root, pkg.dsh.bundle.patch);
  if (!patchPath.startsWith(root + sep)) throw new Error('插件入口越界');
  const patches = parse(await readFile(patchPath, 'utf8'));
  const inserts = Array.isArray(patches) ? patches.flatMap(patch => patch.insert ?? []) : [];
  if (inserts.length) rows = inserts;
}
await mkdir(join(directory, 'entries'), { recursive: true });
const entries = [];
for (const row of rows) {
  if (row.disabled === true) continue;
  if (typeof row.name !== 'string') throw new Error('插件清单缺少入口名称');
  const entry = require.resolve(row.name);
  entries.push({ id: row.id || row.name, name: row.name, entry, config: row.config ?? {} });
}
// The official browser package is kept intact; its module factory uses the
// host's React/store/context dependencies rather than a replacement renderer.
const clientExport = pkg.exports?.['./client'];
const clientRelative = typeof clientExport === 'string' ? clientExport : clientExport?.default;
const client = clientRelative ? resolve(root, clientRelative) : null;
if (client && !client.startsWith(root + sep)) throw new Error('浏览器入口越界');
const requiredEnvironmentTools = ['node', ...(pkg.androidTool?.requiredEnvironmentTools ?? [])];
await writeFile(join(directory, 'descriptor.json'), JSON.stringify({
  name: pkg.name, version: pkg.version, description: pkg.description ?? '', root,
  entries, client, settingsInline, requiredEnvironmentTools: [...new Set(requiredEnvironmentTools)],
}, null, 2));
await build({ entryPoints: [join(directory, 'client-host.mjs')], outfile: join(directory, 'client-host.js'),
  bundle: true, platform: 'browser', format: 'iife', logLevel: 'silent',
  loader: { '.woff': 'dataurl', '.woff2': 'dataurl', '.ttf': 'dataurl' },
  define: { 'process.env.NODE_ENV': '"production"' } });
process.stdout.write(JSON.stringify({ ok: true, name: pkg.name, version: pkg.version }));
