import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { execFileSync, execSync } from 'node:child_process';
import type { SandboxConfig } from '../types.js';
import { GroveError } from './project.js';
import { resolveSharedOverlay } from './shared.js';

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
  /**
   * When set, merge into `<worktree>/.claude/settings.local.json` before
   * launch (macOS: enables/disables Claude Code's built-in sandbox).
   */
  localSettings?: Record<string, unknown>;
}

/**
 * Build the argv to launch `program` inside a sandbox.
 *
 * Linux wraps the process in bwrap (a mount namespace exposing only what is
 * bound). macOS launches Claude directly and drives Claude Code's *built-in*
 * sandbox instead, via a settings.local.json patch the caller applies.
 * When sandbox is disabled, returns `{ command: program, args: programArgs }`
 * with cwd handled by the caller.
 */
export function buildSandbox(input: BuildSandboxInput): SandboxArgv {
  // macOS always takes the native path: even --no-sandbox must write
  // sandbox.enabled=false so settings from a previous sandboxed launch
  // don't keep applying.
  if (process.platform === 'darwin') return buildNativeSandbox(input);

  if (!input.sandbox.enabled) {
    return {
      command: input.program,
      args: input.programArgs,
      env: passthroughEnv(input.worktreePath),
    };
  }

  if (process.platform === 'linux') return buildBwrap(input);
  throw new GroveError(
    `Sandboxing is not supported on ${process.platform}. Pass --no-sandbox to run without it.`,
  );
}

/**
 * Linux: construct a bwrap invocation that mounts the worktree rw, the repo
 * root ro (so shared symlinks resolve), essential system + home dirs ro/rw,
 * and execs `program` directly with no intermediate shell.
 */
function buildBwrap(input: BuildSandboxInput): SandboxArgv {
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

  // Optional extra shares from .groverc. On disk each share is a symlink chain
  // <worktree>/<entry> → .grove/shared/<entry> → <root>/<entry>; here a tmpfs
  // overlays .grove/shared and each source is bound at its chain path, so
  // `realpath` on a shared entry stays inside the worktree AND the mountpoints
  // bwrap creates land in the tmpfs instead of persisting on the host fs.
  const overlay = resolveSharedOverlay({
    rootDir: input.rootDir,
    worktreePath: input.worktreePath,
    shareReadOnly: input.sandbox.shareReadOnly,
    shareReadWrite: input.sandbox.shareReadWrite,
  });
  if (overlay) {
    args.push('--tmpfs', overlay.tmpfsDir);
    for (const spec of overlay.binds) {
      args.push(spec.writable ? '--bind' : '--ro-bind', spec.source, spec.dest);
    }
  }

  args.push('--chdir', input.worktreePath);
  args.push('--clearenv');
  for (const [k, v] of Object.entries(buildSandboxEnv(home, resolved, input.worktreePath))) {
    args.push('--setenv', k, v);
  }

  args.push('--', input.program, ...input.programArgs);

  return {
    command: bwrap,
    args,
    env: hostEnvForBwrap(),
  };
}

/**
 * macOS: launch `program` directly and enable Claude Code's *built-in*
 * sandbox (Seatbelt-based, nothing to install) instead of wrapping the
 * process. A hand-rolled sandbox-exec profile keeps fighting Claude's own
 * needs — tty raw mode, keychain reads, atomic config writes — while the
 * native sandbox is maintained against them. It confines Bash commands and
 * their child processes to the worktree plus the session temp dir at the OS
 * level, and gates network access per domain. Read/Edit/Write file tools
 * follow the permission system rather than the sandbox; the worktree-pinning
 * system prompt (lib/claude.ts) covers those.
 *
 * The returned `localSettings` patch is merged into the worktree's
 * .claude/settings.local.json by the caller before launch. gitdirs and
 * shareReadWrite sources live outside the worktree, so they are granted via
 * sandbox.filesystem.allowWrite; shareReadOnly needs nothing (native default
 * read policy is broad).
 */
function buildNativeSandbox(input: BuildSandboxInput): SandboxArgv {
  const allowWrite: string[] = [];
  for (const dir of input.gitDirs ?? []) {
    if (fs.existsSync(dir)) allowWrite.push(canonical(dir));
  }
  for (const share of input.sandbox.shareReadWrite) {
    const abs = path.resolve(input.rootDir, share);
    if (fs.existsSync(abs)) allowWrite.push(canonical(abs));
  }

  const localSettings: Record<string, unknown> = input.sandbox.enabled
    ? {
        sandbox: {
          enabled: true,
          autoAllowBashIfSandboxed: true,
          filesystem: { allowWrite: unique(allowWrite) },
        },
      }
    : // Explicit false: settings.local.json persists across launches, so a
      // --no-sandbox run must overwrite what a sandboxed run wrote.
      { sandbox: { enabled: false } };

  return {
    command: input.program,
    args: input.programArgs,
    env: passthroughEnv(input.worktreePath),
    localSettings,
  };
}

/** Canonicalize a path (resolve symlinks like /tmp → /private/tmp) for subpath matching. */
function canonical(p: string): string {
  try {
    return fs.realpathSync(p);
  } catch {
    return path.resolve(p);
  }
}

function unique(items: string[]): string[] {
  return Array.from(new Set(items));
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

function buildSandboxEnv(
  home: string,
  resolved: ResolvedTools,
  worktreePath: string,
): Record<string, string> {
  const env: Record<string, string> = {
    HOME: home,
    USER: process.env.USER || 'user',
    TERM: process.env.TERM || 'xterm-256color',
    SHELL: '/bin/bash',
    LANG: process.env.LANG || 'en_US.UTF-8',
    PATH: buildSandboxPath(home, resolved),
    PWD: worktreePath,
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
