#!/usr/bin/env node
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import crypto from 'node:crypto';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { createSupportServer, validSupportOrigin } from './support-server.mjs';

const root = path.dirname(fileURLToPath(import.meta.url));
const options = new Set(process.argv.slice(2));
const allowed = new Set(['--pinggy', '--accept-new-host-key', '--help']);
const PRIVATE_MODE = 0o077;
let service, helper, stateDir, statePath, closing = false, ready = false, publishing = false, tail = '';
let startupTimer;
const event = (name, extra = {}) => process.stdout.write(JSON.stringify({ event: name, ...extra }) + '\n');

export function pinggyArguments({ port, knownHosts, acceptNew = false }) {
  if (!Number.isInteger(port) || port < 1024 || port > 65535 || typeof knownHosts !== 'string' || !path.isAbsolute(knownHosts)) {
    throw new TypeError('Invalid tunnel parameters');
  }
  return [
    '-F', '/dev/null', '-T', '-p', '443', '-l', 'http',
    '-o', 'BatchMode=yes', '-o', 'PreferredAuthentications=none',
    '-o', 'IdentityAgent=none', '-o', 'IdentitiesOnly=yes', '-o', 'IdentityFile=none',
    '-o', 'ForwardAgent=no', '-o', 'ForwardX11=no', '-o', 'PermitLocalCommand=no',
    '-o', `StrictHostKeyChecking=${acceptNew ? 'accept-new' : 'yes'}`,
    '-o', `UserKnownHostsFile=${knownHosts}`, '-o', 'GlobalKnownHostsFile=/dev/null',
    '-o', 'UpdateHostKeys=no', '-o', 'ExitOnForwardFailure=yes',
    '-o', 'ConnectTimeout=12', '-o', 'ConnectionAttempts=1',
    '-o', 'ServerAliveInterval=15', '-o', 'ServerAliveCountMax=2', '-o', 'LogLevel=ERROR',
    '-R', `0:127.0.0.1:${port}`, 'free.pinggy.io', 'x:noreverseproxy',
  ];
}

async function stop(reason = 'stopped') {
  if (closing) return;
  closing = true;
  clearTimeout(startupTimer);
  if (service) await service.close();
  if (helper && helper.exitCode === null && helper.signalCode === null) {
    const exit = new Promise(resolve => { helper.once('close', resolve); helper.once('error', resolve); });
    const force = setTimeout(() => { if (helper.exitCode === null && helper.signalCode === null) helper.kill('SIGKILL'); }, 4000);
    helper.kill('SIGTERM');
    await exit;
    clearTimeout(force);
  }
  if (statePath) await fs.unlink(statePath).catch(() => {});
  if (stateDir) await fs.rmdir(stateDir).catch(() => {});
  tail = '';
  event('support_closed', { reason });
}

async function knownHostFile() {
  const dataRoot = process.env.XDG_DATA_HOME || path.join(os.homedir(), '.local', 'share');
  if (!path.isAbsolute(dataRoot)) throw new Error('private_host_directory_required');
  const directory = path.join(dataRoot, 'pst-quickview-support');
  await fs.mkdir(directory, { recursive: true, mode: 0o700 });
  const directoryStat = await fs.lstat(directory);
  if (!directoryStat.isDirectory() || directoryStat.isSymbolicLink() || (directoryStat.mode & PRIVATE_MODE)
    || (process.getuid && directoryStat.uid !== process.getuid())) throw new Error('private_host_directory_required');
  const file = path.join(directory, 'known_hosts');
  try {
    const stat = await fs.lstat(file);
    if (!stat.isFile() || stat.isSymbolicLink() || (stat.mode & PRIVATE_MODE)
      || (process.getuid && stat.uid !== process.getuid())) throw new Error('private_host_file_required');
    if (stat.size === 0 && !options.has('--accept-new-host-key')) throw new Error('host_trust_required');
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
    if (!options.has('--accept-new-host-key')) throw new Error('host_trust_required');
    await fs.writeFile(file, '', { mode: 0o600, flag: 'wx' });
  }
  return file;
}

async function main() {
  if ([...options].some(option => !allowed.has(option)) || (!options.has('--pinggy') && !options.has('--help'))) {
    throw new Error('usage');
  }
  if (options.has('--help')) {
    process.stdout.write('Usage: node scripts/support-session.mjs --pinggy [--accept-new-host-key]\n'
      + 'Starts a temporary approved PST QuickView support session. Keep this process running; Ctrl+C stops it.\n'
      + 'First use requires the official free.pinggy.io SSH host key in the dedicated private known_hosts file.\n'
      + '--accept-new-host-key explicitly trusts the first key presented by that host. It is trust on first use,\n'
      + 'not independent verification, and never accepts a changed key. Existing SSH settings and keys are unused.\n'
      + 'An ephemeral private state file contains the invitation and local controller URL; keep it private.\n');
    return;
  }
  const knownHosts = await knownHostFile();
  if (closing) return;
  const response = await fetch('https://am.i.mullvad.net/json', { redirect: 'error', signal: AbortSignal.timeout(8000) });
  const vpn = await response.json();
  if (closing) return;
  const { publicIPv4 } = await import('../public/support/protocol.js');
  if (!response.ok || vpn.mullvad_exit_ip !== true || !publicIPv4(vpn.ip)) throw new Error('vpn_not_verified');
  const [html, controller, protocol, peer, css] = await Promise.all([
    'controller.html', 'controller.js', 'protocol.js', 'peer.js', 'support.css',
  ].map(name => fs.readFile(path.join(root, '../public/support', name), 'utf8')));
  if (closing) return;
  service = await createSupportServer({ assets: { html, controller, protocol, peer, css }, onStop: ({ reason }) => stop(reason) });
  if (closing) { await service.close(); return; }
  const key = crypto.randomBytes(32).toString('base64url');
  const controllerOrigin = `http://127.0.0.1:${service.controllerPort}`;
  const args = pinggyArguments({ port: service.guestPort, knownHosts, acceptNew: options.has('--accept-new-host-key') });
  helper = spawn('/usr/bin/ssh', args, { cwd: root, env: { PATH: process.env.PATH, HOME: process.env.HOME, LANG: 'C.UTF-8' }, stdio: ['ignore', 'pipe', 'pipe'] });
  helper.on('error', () => { event('support_helper_failed'); void stop('helper_unavailable'); });
  helper.on('close', () => { if (!closing) { event('support_helper_closed'); void stop('helper_closed'); } });
  async function consume(chunk) {
    if (closing || ready) return;
    tail = (tail + chunk.toString()).slice(-16000);
    if (publishing) return;
    const match = tail.match(/https:\/\/[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.run\.pinggy-free\.link\b/);
    if (!match || !validSupportOrigin(match[0])) return;
    publishing = true;
    try {
      const origin = match[0];
      service.setGuestOrigin(origin);
      let reachable = false;
      for (let attempt = 0; attempt < 8 && !closing; attempt++) {
        try {
          const page = await fetch(origin + '/join', { redirect: 'error', signal: AbortSignal.timeout(5000) });
          if (page.ok && (await page.text()).includes('<title>PST QuickView support invitation</title>')) { reachable = true; break; }
        } catch { /* Retry without retaining URLs, addresses, or helper output. */ }
        await new Promise(resolve => setTimeout(resolve, 1000));
      }
      if (!reachable || closing) throw new Error('setup');
      const common = { v: 2, sid: service.sid, key, expires: service.expiresAt, expectedExit: vpn.ip };
      const encode = value => Buffer.from(JSON.stringify(value)).toString('base64url');
      const guestURL = origin + '/join#' + encode({ ...common, role: 'guest', token: service.guestToken, origin });
      const controllerURL = controllerOrigin + '/#' + encode({ ...common, role: 'controller', token: service.controllerToken, origin: controllerOrigin, invite: guestURL });
      stateDir = await fs.mkdtemp(path.join(os.tmpdir(), 'pst-quickview-support-'));
      await fs.chmod(stateDir, 0o700);
      statePath = path.join(stateDir, 'session.json');
      await fs.writeFile(statePath, JSON.stringify({ pid: process.pid, helperPid: helper.pid, controllerURL, guestURL, expiresAt: service.expiresAt }), { mode: 0o600, flag: 'wx' });
      if (closing) { await fs.unlink(statePath).catch(() => {}); await fs.rmdir(stateDir).catch(() => {}); return; }
      tail = ''; ready = true; clearTimeout(startupTimer);
      event('support_ready', { stateFile: statePath, lifetimeMinutes: 30 });
    } catch { event('support_setup_failed'); void stop('setup_failed'); }
  }
  helper.stdout.on('data', chunk => { void consume(chunk); });
  helper.stderr.on('data', chunk => { void consume(chunk); });
  startupTimer = setTimeout(() => { if (!ready && !closing) { event('support_setup_timeout'); void stop('setup_timeout'); } }, 60000);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  process.on('SIGINT', () => { void stop('stopped'); });
  process.on('SIGTERM', () => { void stop('stopped'); });
  main().catch(error => {
    const known = ['usage', 'host_trust_required', 'private_host_directory_required', 'private_host_file_required', 'vpn_not_verified'];
    event('support_start_failed', { reason: known.includes(error.message) ? error.message : 'unavailable' });
    if (error.message === 'host_trust_required') process.stderr.write('Verify the official Pinggy host key into the dedicated private known_hosts file, or explicitly choose trust on first use with --accept-new-host-key.\n');
    process.exitCode = 1;
    void stop('startup_failed');
  });
}
