#!/usr/bin/env node
// Manual Linux operator entry point. Never installed or started automatically.
import fs from 'node:fs/promises';
import { constants } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { launchOperatorDashboard, loadOperatorIdentity, validateDashboardService } from './support-dashboard.mjs';

const ENTRY = fileURLToPath(import.meta.url);
const SERVICE = path.resolve(path.dirname(ENTRY), '../public/support/service.json');
const pause = milliseconds => new Promise(resolve => setTimeout(resolve, milliseconds));
const exact = (value, keys) => value && typeof value === 'object' && !Array.isArray(value)
  && Object.keys(value).length === keys.length && keys.every(key => Object.hasOwn(value, key));
const validPid = pid => Number.isSafeInteger(pid) && pid > 1;
const alive = pid => { try { process.kill(pid, 0); return true; } catch (error) { return error.code !== 'ESRCH'; } };
const pointerPath = configPath => path.join(path.dirname(configPath), 'dashboard-pointer.json');

async function privateDirectory(directory) {
  const stat = await fs.lstat(directory);
  if (!stat.isDirectory() || stat.isSymbolicLink() || (stat.mode & 0o077)
    || (process.getuid && stat.uid !== process.getuid())) throw Error('private_config_required');
}
async function privateJson(file, maximum = 4096) {
  const handle = await fs.open(file, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const stat = await handle.stat();
    if (!stat.isFile() || (stat.mode & 0o077) || stat.size > maximum
      || (process.getuid && stat.uid !== process.getuid())) throw Error('unsafe_local_state');
    return JSON.parse(await handle.readFile('utf8'));
  } finally { await handle.close(); }
}
async function writeExclusive(file, value) {
  const handle = await fs.open(file, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW, 0o600);
  try { await handle.writeFile(JSON.stringify(value)); await handle.sync(); } finally { await handle.close(); }
}
async function removeMatching(file, expected) {
  try { if (JSON.stringify(await privateJson(file)) === JSON.stringify(expected)) await fs.unlink(file); } catch { /* Never remove unclassified state. */ }
}

async function startLock(directory, timeout = 10_000) {
  const file = path.join(directory, 'dashboard-start.lock'), recovery = path.join(directory, 'dashboard-recovery.lock');
  const owner = { pid: process.pid, nonce: randomBytes(16).toString('hex') }, deadline = Date.now() + timeout;
  do {
    try { await writeExclusive(file, owner); return () => removeMatching(file, owner); }
    catch (error) { if (error.code !== 'EEXIST') throw error; }
    const record = await privateJson(file).catch(error => { if (error.code === 'ENOENT') return null; throw error; });
    if (record && (!exact(record, ['pid', 'nonce']) || !validPid(record.pid) || !/^[a-f0-9]{32}$/.test(record.nonce))) throw Error('unsafe_start_lock');
    if (record && !alive(record.pid)) {
      // Serialize stale reclamation so concurrent manual launches cannot remove
      // a replacement lock created by another live launcher.
      try {
        await writeExclusive(recovery, owner);
        try {
          const current = await privateJson(file).catch(error => { if (error.code === 'ENOENT') return null; throw error; });
          if (current && exact(current, ['pid', 'nonce']) && validPid(current.pid) && /^[a-f0-9]{32}$/.test(current.nonce) && !alive(current.pid)) await removeMatching(file, current);
        } finally { await removeMatching(recovery, owner); }
      } catch (error) { if (error.code !== 'EEXIST') throw error; }
    }
    await pause(100);
  } while (Date.now() < deadline);
  throw Error('dashboard_start_busy');
}

export async function inspectDashboardPointer(file, service, { fetchImpl = fetch } = {}) {
  const pointer = await privateJson(file);
  if (!exact(pointer, ['pid', 'stateFile']) || !validPid(pointer.pid) || typeof pointer.stateFile !== 'string' || !path.isAbsolute(pointer.stateFile)
    || path.basename(pointer.stateFile) !== 'dashboard.json') throw Error('unsafe_local_state');
  const directory = path.dirname(pointer.stateFile), tempRoot = await fs.realpath(os.tmpdir());
  if (path.normalize(pointer.stateFile) !== pointer.stateFile || path.dirname(directory) !== path.resolve(os.tmpdir())
    || !/^pst-support-dashboard-[A-Za-z0-9]{6}$/.test(path.basename(directory))) throw Error('unsafe_local_state');
  try { await privateDirectory(directory); } catch (error) { if (error.code === 'ENOENT') return { pointer, state: null, available: false }; throw error; }
  const realDirectory = await fs.realpath(directory);
  if (path.dirname(realDirectory) !== tempRoot || !/^pst-support-dashboard-[A-Za-z0-9]{6}$/.test(path.basename(realDirectory))) throw Error('unsafe_local_state');
  let state;
  try { state = await privateJson(pointer.stateFile); } catch (error) { if (error.code === 'ENOENT') return { pointer, state: null, available: false }; throw error; }
  if (!exact(state, ['pid', 'dashboardURL']) || state.pid !== pointer.pid || typeof state.dashboardURL !== 'string') throw Error('unsafe_local_state');
  const match = /^http:\/\/127\.0\.0\.1:([1-9]\d{3,4})\/#([a-f0-9]{64})$/.exec(state.dashboardURL);
  if (!match || Number(match[1]) < 1024 || Number(match[1]) > 65535) throw Error('unsafe_local_state');
  if (!alive(pointer.pid)) return { pointer, state, available: false };
  const url = new URL(state.dashboardURL);
  try {
    const response = await fetchImpl(url.origin + '/api/service', { headers: { Origin: url.origin, Authorization: 'Bearer ' + match[2] },
      credentials: 'omit', redirect: 'error', cache: 'no-store', signal: AbortSignal.timeout(1500) });
    if (!response.ok || Number(response.headers.get('content-length')) > 4096) return { pointer, state, available: false };
    const reader = response.body?.getReader(); if (!reader) return { pointer, state, available: false };
    let size = 0; const chunks = [];
    try {
      for (;;) { const { done, value } = await reader.read(); if (done) break; size += value.byteLength; if (size > 4096) throw Error(); chunks.push(value); }
    } finally { await reader.cancel().catch(() => {}); reader.releaseLock(); }
    const result = JSON.parse(Buffer.concat(chunks).toString('utf8'));
    return { pointer, state, available: exact(result, ['service']) && JSON.stringify(validateDashboardService(result.service)) === JSON.stringify(service) };
  } catch { return { pointer, state, available: false }; }
}

const childEnvironment = () => Object.fromEntries(['PATH', 'HOME', 'LANG', 'LC_ALL', 'XDG_CONFIG_HOME', 'TMPDIR']
  .filter(key => typeof process.env[key] === 'string').map(key => [key, process.env[key]]));
export function spawnDashboardServer(configPath, { spawnImpl = spawn } = {}) {
  const child = spawnImpl(process.execPath, [ENTRY, '--serve', '--config', configPath], { detached: true, stdio: 'ignore', env: childEnvironment() });
  return new Promise((resolve, reject) => { child.once('error', () => reject(Error('dashboard_start_failed'))); child.once('spawn', () => { child.unref(); resolve(); }); });
}
export function openDashboardBrowser(url, { spawnImpl = spawn } = {}) {
  if (!/^http:\/\/127\.0\.0\.1:[1-9]\d{3,4}\/#(?:[a-f0-9]{64})$/.test(url)
    || Number(new URL(url).port) < 1024 || Number(new URL(url).port) > 65535) throw Error('unsafe_local_state');
  const env = { ...childEnvironment(), ...Object.fromEntries(['DISPLAY', 'WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS', 'XDG_RUNTIME_DIR', 'XDG_SESSION_TYPE', 'XDG_CURRENT_DESKTOP', 'DESKTOP_SESSION']
    .filter(key => typeof process.env[key] === 'string').map(key => [key, process.env[key]])) };
  const child = spawnImpl('/usr/bin/xdg-open', [url], { stdio: 'ignore', env });
  return new Promise((resolve, reject) => { child.once('error', () => reject(Error('browser_unavailable'))); child.once('close', code => code === 0 ? resolve() : reject(Error('browser_unavailable'))); });
}

export async function ensureDashboard({ configPath, servicePath = SERVICE, spawnServer = spawnDashboardServer, waitMs = 10_000 } = {}) {
  if (!configPath || !path.isAbsolute(configPath)) throw Error('private_config_required');
  await privateDirectory(path.dirname(configPath));
  const service = validateDashboardService(JSON.parse(await fs.readFile(servicePath, 'utf8')));
  await loadOperatorIdentity(configPath, service);
  const release = await startLock(path.dirname(configPath), waitMs);
  try {
    const file = pointerPath(configPath);
    let current;
    try { current = await inspectDashboardPointer(file, service); }
    catch (error) { if (!['ENOENT', 'ECONNREFUSED'].includes(error.code)) throw error; }
    if (current?.available) return { stateFile: current.pointer.stateFile, dashboardURL: current.state.dashboardURL, reused: true };
    // A failed authenticated handshake is not authority to kill any process.
    // Only this private pointer is replaced; unknown process/state is untouched.
    if (current) await removeMatching(file, current.pointer);
    await spawnServer(configPath);
    const deadline = Date.now() + waitMs;
    do {
      try {
        current = await inspectDashboardPointer(file, service);
        if (current.available) return { stateFile: current.pointer.stateFile, dashboardURL: current.state.dashboardURL, reused: false };
      } catch (error) { if (error.code !== 'ENOENT') throw error; }
      await pause(100);
    } while (Date.now() < deadline);
    throw Error('dashboard_start_failed');
  } finally { await release(); }
}

export async function serveDashboard({ configPath, servicePath = SERVICE } = {}) {
  const dashboard = await launchOperatorDashboard({ configPath, servicePath });
  const file = pointerPath(configPath), pointer = { pid: process.pid, stateFile: dashboard.stateFile };
  try { await writeExclusive(file, pointer); } catch { await dashboard.close(); throw Error('dashboard_start_failed'); }
  let stopped = false;
  return { stateFile: dashboard.stateFile, async close() { if (stopped) return; stopped = true; await removeMatching(file, pointer); await dashboard.close(); } };
}

if (process.argv[1] && path.resolve(process.argv[1]) === ENTRY) {
  const args = process.argv.slice(2), serve = args[0] === '--serve';
  const startOnly = args[0] === '--start-only';
  if (serve || startOnly) args.shift();
  const base = process.env.XDG_CONFIG_HOME || path.join(os.homedir(), '.config');
  const configPath = args.length === 2 && args[0] === '--config' ? args[1] : args.length === 0 ? path.join(base, 'pst-quickview-support/operator.json') : null;
  let running, stopping = false;
  const close = async () => { stopping = true; await running?.close(); };
  if (serve) {
    process.once('SIGINT', () => { void close(); }); process.once('SIGTERM', () => { void close(); });
    process.once('uncaughtException', () => { process.exitCode = 1; void close(); });
    process.once('unhandledRejection', () => { process.exitCode = 1; void close(); });
  }
  (async () => {
    if (!configPath) throw Error('usage');
    if (serve) { running = await serveDashboard({ configPath }); if (stopping) await running.close(); return; }
    const result = await ensureDashboard({ configPath });
    if (!startOnly) await openDashboardBrowser(result.dashboardURL);
    process.stdout.write(JSON.stringify({ event: startOnly ? 'dashboard_ready' : 'dashboard_open_requested', stateFile: result.stateFile }) + '\n');
  })().catch(async () => { process.stderr.write('PST QuickView Support could not open. Check the private operator setup or try again after the current launcher finishes.\n'); process.exitCode = 1; await close(); });
}
