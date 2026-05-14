import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { execFileSync, execSync } from 'node:child_process';
import type { SandboxConfig } from '../types.js';
import { GroveError } from './project.js';

export interface BuildSandboxInput {
  /** Bound read-only so shared symlinks pointing back into the project resolve. */
  rootDir: string;
  /** Bound read-write — the working directory for the agent. */
  worktreePath: string;
  /** Additional rw bind paths (typically per-worktree gitdirs). Caller resolves. */
  gitDirs?: string[];
  sandbox: SandboxConfig;
  program: string;
  programArgs: string[];
}

export interface SandboxArgv {
  command: string;
  args: string[];
  env: NodeJS.ProcessEnv;
}

/**
 * Build the argv to launch `program` inside a bwrap sandbox.
 *
 * When sandbox is disabled, returns `{ command: program, args: programArgs }`
 * with cwd handled by the caller. Otherwise constructs a bwrap invocation
 * that mounts the worktree rw, the repo root ro (so shared symlinks resolve),
 * essential system + home dirs ro/rw, and execs `program` directly with no
 * intermediate shell.
 */
export function buildSandbox(input: BuildSandboxInput): SandboxArgv {
  if (!input.sandbox.enabled) {
    return {
      command: input.program,
      args: input.programArgs,
      env: passthroughEnv(input.worktreePath),
    };
  }

  const bwrap = resolveBwrap();
  const resolved = resolveTools();
  const home = os.homedir();

  const args: string[] = [];

  // Namespace isolation. Network stays so Claude can call the API.
  args.push(
    '--unshare-user',
    '--unshare-pid',
    '--unshare-uts',
    '--unshare-cgroup',
    '--die-with-parent',
  );

  // System (read-only)
  for (const dir of ['/usr', '/bin', '/lib', '/sbin', '/etc']) {
    args.push('--ro-bind', dir, dir);
  }
  args.push('--proc', '/proc');
  args.push('--dev', '/dev');
  args.push('--tmpfs', '/tmp');

  for (const opt of ['/lib64', '/run', '/mnt/wsl', '/home/linuxbrew']) {
    if (fs.existsSync(opt)) args.push('--ro-bind', opt, opt);
  }

  // Empty home, then selectively re-share.
  args.push('--tmpfs', home);
  shareHomeEntries(args, home);
  shareToolPaths(args, home, resolved);

  // Root ro so shared symlinks (pointing back into the project) resolve.
  // Worktree binding below overlays this for the rw region.
  args.push('--ro-bind', input.rootDir, input.rootDir);

  // Worktree itself (rw)
  args.push('--bind', input.worktreePath, input.worktreePath);

  // Gitdirs (rw) — main .git dir(s) plus per-worktree gitdir(s).
  for (const dir of input.gitDirs ?? []) {
    if (fs.existsSync(dir)) args.push('--bind', dir, dir);
  }

  // Optional extra shares from .groverc
  for (const share of input.sandbox.shareReadOnly) {
    const abs = path.resolve(input.rootDir, share);
    if (fs.existsSync(abs)) args.push('--ro-bind', abs, abs);
  }
  for (const share of input.sandbox.shareReadWrite) {
    const abs = path.resolve(input.rootDir, share);
    if (fs.existsSync(abs)) args.push('--bind', abs, abs);
  }

  args.push('--chdir', input.worktreePath);
  args.push('--clearenv');
  for (const [k, v] of Object.entries(buildSandboxEnv(home, resolved))) {
    args.push('--setenv', k, v);
  }

  args.push('--', input.program, ...input.programArgs);

  return {
    command: bwrap,
    args,
    env: hostEnvForBwrap(),
  };
}

function resolveBwrap(): string {
  const override = process.env.GROVE_BWRAP_PATH;
  const candidate = override || 'bwrap';
  let resolved: string;
  try {
    if (path.isAbsolute(candidate)) {
      fs.accessSync(candidate, fs.constants.X_OK);
      resolved = candidate;
    } else {
      resolved = execSync(`which ${candidate}`, { encoding: 'utf-8' }).trim();
    }
  } catch {
    throw new GroveError(
      `bwrap not found (looked for "${candidate}"). Install bwrap or pass --no-sandbox.`,
    );
  }
  // Smoke test
  try {
    execFileSync(resolved, ['--ro-bind', '/', '/', '--', '/bin/true'], { timeout: 5000, stdio: 'ignore' });
  } catch (err) {
    throw new GroveError(
      `bwrap smoke test failed (user namespaces may be disabled): ${(err as Error).message}`,
    );
  }
  return resolved;
}

interface ResolvedTools {
  claude: string | null;
  node: string | null;
  git: string | null;
  pnpm: string | null;
}

function resolveTools(): ResolvedTools {
  const find = (bin: string): string | null => {
    try {
      const p = execSync(`which ${bin}`, { encoding: 'utf-8' }).trim();
      return p ? fs.realpathSync(p) : null;
    } catch {
      return null;
    }
  };
  return {
    claude: find('claude'),
    node: find('node'),
    git: find('git'),
    pnpm: find('pnpm'),
  };
}

function shareHomeEntries(args: string[], home: string): void {
  const ro: string[] = [];
  const rw: string[] = [];

  // Claude credentials and session state (rw)
  for (const rel of ['.claude', '.claude.json']) {
    const p = path.join(home, rel);
    if (fs.existsSync(p)) rw.push(p);
  }

  // Identity and tool auth (ro)
  for (const rel of ['.gitconfig', '.config/git', '.config/glab-cli', '.config/gh', '.ssh']) {
    const p = path.join(home, rel);
    if (fs.existsSync(p)) ro.push(p);
  }

  // SSH agent socket
  const sshAuthSock = process.env.SSH_AUTH_SOCK;
  if (sshAuthSock && fs.existsSync(sshAuthSock)) {
    ro.push(path.dirname(sshAuthSock));
  }

  for (const p of ro) args.push('--ro-bind', p, p);
  for (const p of rw) args.push('--bind', p, p);
}

function shareToolPaths(args: string[], home: string, resolved: ResolvedTools): void {
  // ~/.local/bin (claude symlink), ~/.local/share/claude (install dir)
  for (const rel of ['.local/bin', '.local/share/claude']) {
    const p = path.join(home, rel);
    if (fs.existsSync(p)) args.push('--ro-bind', p, p);
  }

  // node prefix — e.g. nvm install dir
  if (resolved.node) {
    const nodePrefix = path.resolve(path.dirname(resolved.node), '..');
    if (fs.existsSync(nodePrefix) && nodePrefix.startsWith(home)) {
      args.push('--ro-bind', nodePrefix, nodePrefix);
    }
  }

  // pnpm global dir
  const pnpmHome = process.env.PNPM_HOME || path.join(home, '.local/share/pnpm');
  if (fs.existsSync(pnpmHome)) args.push('--ro-bind', pnpmHome, pnpmHome);
}

export function resolveWorktreeGitDir(worktreePath: string): string | null {
  const gitFile = path.join(worktreePath, '.git');
  if (!fs.existsSync(gitFile)) return null;
  try {
    const content = fs.readFileSync(gitFile, 'utf-8').trim();
    const match = content.match(/^gitdir:\s*(.+)$/);
    if (!match || !match[1]) return null;
    return path.resolve(worktreePath, match[1]);
  } catch {
    return null;
  }
}

function buildSandboxEnv(home: string, resolved: ResolvedTools): Record<string, string> {
  const env: Record<string, string> = {
    HOME: home,
    USER: process.env.USER || 'user',
    TERM: process.env.TERM || 'xterm-256color',
    SHELL: '/bin/bash',
    LANG: process.env.LANG || 'en_US.UTF-8',
    PATH: buildSandboxPath(home, resolved),
  };
  if (process.env.ANTHROPIC_API_KEY) env.ANTHROPIC_API_KEY = process.env.ANTHROPIC_API_KEY;
  if (process.env.SSH_AUTH_SOCK) env.SSH_AUTH_SOCK = process.env.SSH_AUTH_SOCK;
  if (process.env.COLORTERM) env.COLORTERM = process.env.COLORTERM;
  return env;
}

function buildSandboxPath(home: string, resolved: ResolvedTools): string {
  const dirs = new Set<string>([
    '/usr/local/sbin',
    '/usr/local/bin',
    '/usr/sbin',
    '/usr/bin',
    '/sbin',
    '/bin',
    path.join(home, '.local/bin'),
    process.env.PNPM_HOME || path.join(home, '.local/share/pnpm'),
  ]);
  for (const v of Object.values(resolved)) {
    if (v) dirs.add(path.dirname(v));
  }
  if (fs.existsSync('/home/linuxbrew/.linuxbrew/bin')) {
    dirs.add('/home/linuxbrew/.linuxbrew/bin');
  }
  return Array.from(dirs).join(':');
}

function hostEnvForBwrap(): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = {};
  for (const k of ['PATH', 'HOME', 'USER', 'TERM']) {
    const v = process.env[k];
    if (v) env[k] = v;
  }
  return env;
}

function passthroughEnv(worktreePath: string): NodeJS.ProcessEnv {
  return { ...process.env, PWD: worktreePath };
}
