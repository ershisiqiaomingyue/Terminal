import { cp, mkdir, readFile, realpath, symlink, writeFile, access } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { dirname, join, relative, resolve, sep } from 'node:path';
import { pathToFileURL } from 'node:url';

const packageName = value => typeof value === 'string' && /^(?:@[a-z0-9._-]+\/)?[a-z0-9._-]+$/i.test(value) && !value.split('/').some(part => part === '.' || part === '..');
async function manifest(path) { return JSON.parse(await readFile(join(path, 'package.json'), 'utf8')); }

// Resolve physical installed packages, including hoisted / pnpm packages whose
// package.json is hidden by exports. Never consult a registry or execute npm.
async function installedPackage(name, start) {
  if (!packageName(name)) throw new Error(`插件依赖名称无效：${name}`);
  let path = await realpath(start);
  while (true) {
    const candidate = join(path, 'node_modules', ...name.split('/'));
    try { await access(join(candidate, 'package.json')); return await realpath(candidate); }
    catch (error) { if (!['ENOENT', 'ENOTDIR'].includes(error.code)) throw error; }
    const parent = dirname(path); if (parent === path) return null; path = parent;
  }
}

export async function localHostPackages({ source, candidates = [], hostDependencies = [] }) {
  const hosts = new Map(), missing = [];
  for (const spec of hostDependencies) {
    const at = spec.lastIndexOf('@'), name = spec.slice(0, at), version = spec.slice(at + 1);
    if (at <= 0 || !packageName(name)) throw new Error('宿主依赖配置无效');
    for (const base of [source, ...candidates]) {
      try {
        const path = await installedPackage(name, base);
        if (path && (await manifest(path)).version === version) { hosts.set(name, path); break; }
      } catch (error) { if (!['ENOENT', 'ENOTDIR'].includes(error.code)) throw error; }
    }
    if (!hosts.has(name)) missing.push(spec);
  }
  return { hosts, missing };
}

export async function copyLocalPlugin(config) {
  const source = await realpath(config.source), directory = resolve(config.directory);
  if (directory === source || directory.startsWith(source + sep)) throw new Error('插件副本不能放在源目录内');
  const pkg = await manifest(source);
  if (!packageName(pkg.name) || (!pkg.dsh && !pkg.androidTool)) throw new Error('本地目录未声明有效插件');
  const { hosts, missing } = await localHostPackages(config);
  if (missing.length) throw new Error(`缺少宿主依赖：${missing.join('、')}`);
  const nodes = new Map();
  async function visit(path) {
    path = await realpath(path);
    if (nodes.has(path)) return nodes.get(path);
    const pkg = await manifest(path);
    if (!packageName(pkg.name)) throw new Error('本地依赖包名无效');
    const id = createHash('sha256').update(path).digest('hex').slice(0, 20);
    const node = { source: path, destination: join(directory, '.local-packages', id), pkg, dependencies: new Map() };
    nodes.set(path, node);
    const required = { ...pkg.dependencies, ...pkg.optionalDependencies, ...pkg.peerDependencies };
    for (const name of Object.keys(required)) {
      const optional = Object.hasOwn(pkg.optionalDependencies || {}, name)
        || (!Object.hasOwn(pkg.dependencies || {}, name) && pkg.peerDependenciesMeta?.[name]?.optional === true);
      const dependency = await installedPackage(name, path) || hosts.get(name);
      if (!dependency) {
        if (optional) continue;
        throw new Error(`本地插件 ${pkg.name} 缺少已安装依赖 ${name}，请在源目录安装完整依赖后重新加载`);
      }
      node.dependencies.set(name, await visit(dependency));
    }
    return node;
  }
  // Build and validate the complete graph before copying. Preserve different
  // versions and cycles without resolving new versions from a registry.
  const root = await visit(source), hostNodes = new Map();
  for (const [name, path] of hosts) hostNodes.set(name, await visit(path));
  await mkdir(directory, { recursive: true });
  for (const node of nodes.values()) {
    await cp(node.source, node.destination, { recursive: true, dereference: true,
      filter: path => {
        const first = relative(node.source, path).split(sep)[0];
        return !['node_modules', '.git', '.runtime'].includes(first);
      } });
  }
  async function link(modules, name, target) {
    const path = join(modules, ...name.split('/'));
    await mkdir(dirname(path), { recursive: true });
    // Junctions target only our copied packages; no dependency points back into
    // the original installation. Windows junctions need no symlink privilege.
    await symlink(target.destination, path, process.platform === 'win32' ? 'junction' : 'dir');
  }
  for (const node of nodes.values()) {
    for (const [name, target] of node.dependencies) await link(join(node.destination, 'node_modules'), name, target);
  }
  for (const [name, target] of hostNodes) await link(join(directory, 'node_modules'), name, target);
  if (!hostNodes.has(pkg.name)) await link(join(directory, 'node_modules'), pkg.name, root);
  // The destination is managed independently; never persist a file: dependency
  // referring to the original plugin or run its lifecycle scripts.
  await writeFile(join(directory, 'package.json'), '{"private":true,"type":"module"}');
  await writeFile(join(directory, 'local-plugin.json'), JSON.stringify({ name: pkg.name, version: pkg.version, packages: nodes.size }));
  return { name: pkg.name, packages: nodes.size };
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    const config = JSON.parse(await readFile(process.argv[2], 'utf8'));
    const result = process.argv[3] === 'check' ? { missing: (await localHostPackages(config)).missing } : await copyLocalPlugin(config);
    await writeFile(process.argv[4], JSON.stringify(result));
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
