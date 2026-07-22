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
}

/**
 * Build the argv to launch `program` inside a sandbox.
 *
 * When sandbox is disabled, returns `{ command: program, args: programArgs }`
 * with cwd handled by the caller. Otherwise dispatches by platform: bwrap on
 * Linux (a mount namespace exposing only what is bound), sandbox-exec on macOS
 * (a Seatbelt write-confinement profile over the real filesystem).
 */
export function buildSandbox(input: BuildSandboxInput): SandboxArgv {
  if (!input.sandbox.enabled) {
    return {
      command: input.program,
      args: input.programArgs,
      env: passthroughEnv(input.worktreePath),
    };
  }

  if (process.platform === 'darwin') return buildSeatbelt(input);
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
  for (const [k, v] of Object.entries(buildSandboxEnv(home, resolved, 'linux', input.worktreePath))) {
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
 * macOS: launch `program` under sandbox-exec with a Seatbelt profile that
 * denies everything by default, then re-allows the operations a dev session
 * needs (exec, network, mach lookups) plus *content* reads and writes within a
 * confined set of paths. Reads are restricted to the worktree, the gitdirs,
 * declared shares, Claude/tool state, and the system paths required to run
 * binaries — so the agent cannot read files outside its workspace. Writes are
 * the read-write subset of that (worktree, gitdirs, rw shares, Claude state,
 * temp).
 *
 * `file-read-metadata` is allowed globally: the kernel needs to stat ancestor
 * path components to resolve any path at all, so existence/size of arbitrary
 * paths leaks, but file *contents* outside the allowed subpaths do not.
 */
function buildSeatbelt(input: BuildSandboxInput): SandboxArgv {
  const sandboxExec = resolveSandboxExec();
  const resolved = resolveTools();
  const home = os.homedir();

  // System paths required to load and run binaries (dyld cache, frameworks,
  // shells, brew, resolver config, device nodes).
  const readSubpaths: string[] = [];
  for (const dir of [
    '/usr',
    '/System',
    '/Library',
    '/bin',
    '/sbin',
    '/opt',
    '/dev',
    '/private/etc',
    '/private/var/db',
    '/private/var/run',
    '/private/var/folders',
  ]) {
    if (fs.existsSync(dir)) readSubpaths.push(dir);
  }

  // Tool installs that may live under $HOME (nvm node, ~/.local/share/claude).
  for (const p of toolReadPaths(home, resolved)) {
    if (fs.existsSync(p)) readSubpaths.push(canonical(p));
  }

  // Identity / auth, read-only.
  for (const rel of ['.gitconfig', '.config/git', '.config/gh', '.config/glab-cli', '.ssh']) {
    const p = path.join(home, rel);
    if (fs.existsSync(p)) readSubpaths.push(canonical(p));
  }
  const sshAuthSock = process.env.SSH_AUTH_SOCK;
  if (sshAuthSock && fs.existsSync(sshAuthSock)) readSubpaths.push(canonical(path.dirname(sshAuthSock)));

  // Claude on macOS stores OAuth credentials in the login keychain; the
  // Security framework reads the keychain db directly (mach-lookup to
  // securityd is already allowed above via the global mach-lookup rule).
  const keychains = path.join(home, 'Library/Keychains');
  if (fs.existsSync(keychains)) readSubpaths.push(canonical(keychains));

  // Writable regions — also readable.
  const writeSubpaths: string[] = [canonical(input.worktreePath)];
  for (const dir of input.gitDirs ?? []) {
    if (fs.existsSync(dir)) writeSubpaths.push(canonical(dir));
  }
  for (const share of input.sandbox.shareReadWrite) {
    const abs = path.resolve(input.rootDir, share);
    if (fs.existsSync(abs)) writeSubpaths.push(canonical(abs));
  }
  // Unconditional so a fresh install can create ~/.claude inside the sandbox.
  const claudeDir = process.env.CLAUDE_CONFIG_DIR
    ? path.resolve(process.env.CLAUDE_CONFIG_DIR)
    : path.join(home, '.claude');
  writeSubpaths.push(canonical(claudeDir));
  for (const t of ['/private/tmp', '/private/var/folders', process.env.TMPDIR]) {
    if (t && fs.existsSync(t)) writeSubpaths.push(canonical(t));
  }
  writeSubpaths.push('/dev');

  // Read-only shares declared in .groverc (resolve symlink targets back in the repo).
  for (const share of input.sandbox.shareReadOnly) {
    const abs = path.resolve(input.rootDir, share);
    if (fs.existsSync(abs)) readSubpaths.push(canonical(abs));
  }

  // ~/.claude.json is rewritten atomically: Claude writes a temp sibling
  // (.claude.json.<hash>), renames it over the original, and keeps a
  // .claude.json.backup. A prefix match covers the whole family, read+write —
  // a bare write literal on .claude.json alone breaks startup (config
  // unreadable) and every save (temp sibling unwritable).
  const rwPrefixes = [canonical(path.join(home, '.claude.json'))];

  const profile = buildSeatbeltProfile({
    read: unique([...readSubpaths, ...writeSubpaths]),
    write: unique(writeSubpaths),
    rwPrefixes: unique(rwPrefixes),
  });
  const program = resolveProgram(input.program);

  return {
    command: sandboxExec,
    args: ['-p', profile, program, ...input.programArgs],
    env: buildSandboxEnv(home, resolved, 'darwin', input.worktreePath),
  };
}

/** Home-local tool install dirs that must be readable to run node/claude/pnpm. */
function toolReadPaths(home: string, resolved: ResolvedTools): string[] {
  const paths = [path.join(home, '.local/bin'), path.join(home, '.local/share/claude')];
  if (resolved.node) paths.push(path.resolve(path.dirname(resolved.node), '..'));
  paths.push(process.env.PNPM_HOME || path.join(home, '.local/share/pnpm'));
  return paths;
}

interface SeatbeltPaths {
  read: string[];
  write: string[];
  /** Absolute path prefixes allowed read+write via regex, e.g. ~/.claude.json*. */
  rwPrefixes: string[];
}

function buildSeatbeltProfile(paths: SeatbeltPaths): string {
  const lines = [
    '(version 1)',
    '(deny default)',
    '(allow process-exec*)',
    '(allow process-fork)',
    // Node/libuv stat their own (and spawned children's) processes via
    // proc_pidinfo; Claude also signals its child shells (timeouts, Ctrl-C),
    // so `target self` is not enough.
    '(allow process-info*)',
    '(allow signal (target same-sandbox))',
    '(allow sysctl-read)',
    '(allow mach-lookup)',
    '(allow ipc-posix-shm)',
    '(allow iokit-open)',
    '(allow system-socket)',
    '(allow network*)',
    // Interactive terminal: raw-mode ioctls on the inherited tty (Ink dies
    // instantly without them) and pty allocation for shell tools. ioctl needs
    // an open fd, so the file-read/file-write confinement still gates it.
    '(allow file-ioctl)',
    '(allow pseudo-tty)',
    // Metadata (stat) must be global so the kernel can resolve path components.
    '(allow file-read-metadata)',
    '(allow file-read*',
  ];
  for (const p of paths.read) lines.push(`  (subpath ${sbplString(p)})`);
  for (const p of paths.rwPrefixes) lines.push(`  (regex ${sbplPrefixRegex(p)})`);
  lines.push(')');
  lines.push('(allow file-write*');
  for (const p of paths.write) lines.push(`  (subpath ${sbplString(p)})`);
  for (const p of paths.rwPrefixes) lines.push(`  (regex ${sbplPrefixRegex(p)})`);
  lines.push(')');
  return lines.join('\n') + '\n';
}

/** Quote a path as an SBPL string literal, escaping backslashes and quotes. */
function sbplString(p: string): string {
  return `"${p.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"`;
}

/** Anchored SBPL regex literal matching `prefix` and anything appended to it. */
function sbplPrefixRegex(prefix: string): string {
  const escaped = prefix.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  return `#"^${escaped.replace(/"/g, '\\"')}"`;
}

function resolveSandboxExec(): string {
  const candidate = process.env.GROVE_SANDBOX_EXEC_PATH || '/usr/bin/sandbox-exec';
  try {
    fs.accessSync(candidate, fs.constants.X_OK);
    return candidate;
  } catch {
    throw new GroveError(
      `sandbox-exec not found (looked for "${candidate}"). Pass --no-sandbox to run without a sandbox.`,
    );
  }
}

/** Resolve a bare program name to an absolute path so sandbox-exec can exec it. */
function resolveProgram(program: string): string {
  if (program.includes('/')) return program;
  try {
    const p = execSync(`which ${program}`, { encoding: 'utf-8' }).trim();
    return p || program;
  } catch {
    return program;
  }
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
  platform: NodeJS.Platform = process.platform,
  worktreePath?: string,
): Record<string, string> {
  const env: Record<string, string> = {
    HOME: home,
    USER: process.env.USER || 'user',
    TERM: process.env.TERM || 'xterm-256color',
    SHELL: '/bin/bash',
    LANG: process.env.LANG || 'en_US.UTF-8',
    PATH: buildSandboxPath(home, resolved, platform),
  };
  if (worktreePath) env.PWD = worktreePath;
  if (process.env.ANTHROPIC_API_KEY) env.ANTHROPIC_API_KEY = process.env.ANTHROPIC_API_KEY;
  if (process.env.SSH_AUTH_SOCK) env.SSH_AUTH_SOCK = process.env.SSH_AUTH_SOCK;
  if (process.env.COLORTERM) env.COLORTERM = process.env.COLORTERM;
  if (platform === 'darwin') {
    // sandbox-exec inherits this env directly; macOS tools rely on TMPDIR.
    if (process.env.TMPDIR) env.TMPDIR = process.env.TMPDIR;
    // The seatbelt profile grants rw on this dir instead of ~/.claude.
    if (process.env.CLAUDE_CONFIG_DIR) env.CLAUDE_CONFIG_DIR = process.env.CLAUDE_CONFIG_DIR;
  }
  return env;
}

function buildSandboxPath(
  home: string,
  resolved: ResolvedTools,
  platform: NodeJS.Platform,
): string {
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
  if (platform === 'darwin') {
    for (const d of ['/opt/homebrew/bin', '/opt/homebrew/sbin']) {
      if (fs.existsSync(d)) dirs.add(d);
    }
  } else if (fs.existsSync('/home/linuxbrew/.linuxbrew/bin')) {
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
