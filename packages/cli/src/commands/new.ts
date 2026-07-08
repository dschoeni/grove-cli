import * as path from 'node:path';
import { parseArgs } from 'node:util';
import { loadProject, GroveError, ensureGroveIgnored } from '../lib/project.js';
import {
  parseSlug,
  addWorktree,
  rollbackWorktree,
  addWorkspaceWorktree,
  rollbackWorkspaceWorktree,
  workspaceWorktreeDir,
  repoAbsPath,
  type ParsedSlug,
  type WorktreeAddMode,
} from '../lib/worktree.js';
import { defaultRemote, fetchRemote } from '../lib/git.js';
import { ensureSharedLinks } from '../lib/shared.js';
import { runPostCreateCommands } from '../lib/post-create.js';
import { buildSandbox, resolveWorktreeGitDir } from '../lib/sandbox.js';
import { buildClaudeArgv } from '../lib/claude.js';
import { ensureStatusLine } from '../lib/claude-settings.js';
import { workspaceStatusLineText, singleStatusLineText } from '../lib/status-line.js';
import { execInteractive } from '../lib/exec.js';
import { splitPassthrough } from '../lib/argv.js';
import type { ProjectContext, SandboxConfig, WorkspaceRepo } from '../types.js';

const HELP = `\
grove new — create a worktree and launch a sandboxed Claude session

Usage:
  grove new <slug> [--from <branch>] [--fetch] [--no-sandbox] [--keep-on-failure] [--dry-run] [-- <claude-args>…]

Arguments:
  <slug>                 Branch name. Must start with feat/, fix/, or chore/.

If a branch matching <slug> already exists it is reused: a local branch is checked
out as-is, otherwise a local tracking branch is created from origin/<slug>. --from is
only consulted when a brand-new branch has to be created.

Flags:
  --from <branch>        Base branch to fork from. Defaults to .groverc baseBranch / current HEAD.
                         In workspace mode, overrides every repo's baseBranch.
                         Ignored when an existing branch is reused/adopted.
  --fetch                git fetch the default remote first, so base and origin/<slug> are current.
  --no-sandbox           Skip bwrap. Launches claude in the worktree directly.
  --keep-on-failure      On postCreateCommand failure, leave the worktree in place.
  --dry-run              Print the planned actions and exit before any side effects.
  --                     Stop flag parsing; remaining args are passed to claude.
`;

export async function runNew(argv: string[]): Promise<number> {
  const { args, passthrough } = splitPassthrough(argv);

  const { values, positionals } = parseArgs({
    args,
    options: {
      help: { type: 'boolean', short: 'h' },
      from: { type: 'string' },
      fetch: { type: 'boolean' },
      'no-sandbox': { type: 'boolean' },
      'keep-on-failure': { type: 'boolean' },
      'dry-run': { type: 'boolean' },
    },
    strict: true,
    allowPositionals: true,
  });

  if (values.help) {
    process.stdout.write(HELP);
    return 0;
  }

  if (positionals.length === 0) {
    throw new GroveError('Missing slug. Usage: grove new <slug>');
  }
  if (positionals.length > 1) {
    throw new GroveError(`Too many positionals: ${positionals.join(' ')}`);
  }

  const slug = parseSlug(positionals[0]!);
  const project = loadProject();

  const sandboxConfig: SandboxConfig = {
    ...project.config.sandbox,
    enabled: project.config.sandbox.enabled && !values['no-sandbox'],
  };

  if (project.kind === 'workspace') {
    requireCwdAtWorkspaceRoot(project.workspaceRoot);
    return runNewWorkspace({
      project,
      slug,
      fromOverride: values.from ?? null,
      fetch: Boolean(values.fetch),
      sandboxConfig,
      passthrough,
      keepOnFailure: Boolean(values['keep-on-failure']),
      dryRun: Boolean(values['dry-run']),
    });
  }
  return runNewSingle({
    project,
    slug,
    fromOverride: values.from ?? null,
    fetch: Boolean(values.fetch),
    sandboxConfig,
    passthrough,
    keepOnFailure: Boolean(values['keep-on-failure']),
    dryRun: Boolean(values['dry-run']),
  });
}

interface RunNewInput<P extends ProjectContext> {
  project: P;
  slug: ParsedSlug;
  fromOverride: string | null;
  fetch: boolean;
  sandboxConfig: SandboxConfig;
  passthrough: string[];
  keepOnFailure: boolean;
  dryRun: boolean;
}

async function runNewSingle(
  input: RunNewInput<Extract<ProjectContext, { kind: 'single' }>>,
): Promise<number> {
  const { project, slug, fromOverride, sandboxConfig, passthrough, keepOnFailure, dryRun } = input;

  const baseBranch = fromOverride ?? project.config.baseBranch;

  if (dryRun) {
    printDryRunSingle({ slug: slug.full, baseBranch, project, sandboxConfig, passthrough });
    return 0;
  }

  ensureGroveIgnored(project.repoRoot);

  if (input.fetch) {
    const remote = defaultRemote(project.repoRoot);
    if (remote) {
      process.stderr.write(`\x1b[36m[grove]\x1b[0m fetching ${remote}...\n`);
      fetchRemote(project.repoRoot, remote);
    }
  }

  process.stderr.write(`\x1b[36m[grove]\x1b[0m creating worktree ${slug.full}\n`);
  const added = addWorktree({ repoRoot: project.repoRoot, slug, baseBranch });
  const worktreePath = added.worktreePath;
  reportAddMode(slug.full, added.mode, baseBranch, fromOverride);

  try {
    setupShared(project.repoRoot, worktreePath, project.config.sandbox);
    if (project.config.postCreateCommands.length > 0) {
      runPostCreateCommands({ worktreePath, commands: project.config.postCreateCommands });
    }
  } catch (err) {
    if (!keepOnFailure) {
      process.stderr.write(`\x1b[31m[grove]\x1b[0m setup failed, rolling back...\n`);
      rollbackWorktree(project.repoRoot, slug, added.createdBranch);
    } else {
      process.stderr.write(
        `\x1b[33m[grove]\x1b[0m setup failed but --keep-on-failure set; worktree left at ${worktreePath}\n`,
      );
    }
    throw err;
  }

  ensureStatusLine(worktreePath, singleStatusLineText(slug.full));

  const claude = buildClaudeArgv({ claude: project.config.claude, passthrough, resume: false });
  const sandboxed = buildSandbox({
    rootDir: project.repoRoot,
    worktreePath,
    gitDirs: collectSingleGitDirs(project.repoRoot, worktreePath),
    sandbox: sandboxConfig,
    program: claude.program,
    programArgs: claude.args,
  });
  return execInteractive({
    command: sandboxed.command,
    args: sandboxed.args,
    env: sandboxed.env,
    cwd: worktreePath,
  });
}

async function runNewWorkspace(
  input: RunNewInput<Extract<ProjectContext, { kind: 'workspace' }>>,
): Promise<number> {
  const { project, slug, fromOverride, sandboxConfig, passthrough, keepOnFailure, dryRun } = input;
  const { workspaceRoot, config } = project;

  if (dryRun) {
    printDryRunWorkspace({
      slug: slug.full,
      project,
      fromOverride,
      sandboxConfig,
      passthrough,
    });
    return 0;
  }

  ensureGroveIgnored(workspaceRoot);
  for (const repo of config.repos) {
    ensureGroveIgnored(repoAbsPath(workspaceRoot, repo));
  }

  if (input.fetch) {
    for (const repo of config.repos) {
      const abs = repoAbsPath(workspaceRoot, repo);
      const remote = defaultRemote(abs);
      if (remote) {
        process.stderr.write(`\x1b[36m[grove]\x1b[0m fetching ${remote} in ${repo.name}...\n`);
        fetchRemote(abs, remote);
      }
    }
  }

  process.stderr.write(
    `\x1b[36m[grove]\x1b[0m creating workspace worktree ${slug.full} across ${config.repos.length} repo(s)\n`,
  );

  const added = addWorkspaceWorktree({
    workspaceRoot,
    slug,
    repos: config.repos,
    baseBranchOverride: fromOverride,
  });
  const worktreePath = added.workspaceDir;
  for (const r of added.perRepo) {
    reportAddMode(`${slug.full} · ${r.repo.name}`, r.mode, fromOverride ?? r.repo.baseBranch, fromOverride);
  }

  try {
    setupShared(workspaceRoot, worktreePath, config.sandbox);
    if (config.postCreateCommands.length > 0) {
      runPostCreateCommands({ worktreePath, commands: config.postCreateCommands });
    }
  } catch (err) {
    if (!keepOnFailure) {
      process.stderr.write(`\x1b[31m[grove]\x1b[0m setup failed, rolling back...\n`);
      rollbackWorkspaceWorktree(workspaceRoot, slug, added.perRepo);
    } else {
      process.stderr.write(
        `\x1b[33m[grove]\x1b[0m setup failed but --keep-on-failure set; worktree left at ${worktreePath}\n`,
      );
    }
    throw err;
  }

  ensureStatusLine(worktreePath, workspaceStatusLineText(workspaceRoot, slug.full));

  const claude = buildClaudeArgv({ claude: config.claude, passthrough, resume: false });
  const sandboxed = buildSandbox({
    rootDir: workspaceRoot,
    worktreePath,
    gitDirs: collectWorkspaceGitDirs(workspaceRoot, config.repos, slug),
    sandbox: sandboxConfig,
    program: claude.program,
    programArgs: claude.args,
  });
  return execInteractive({
    command: sandboxed.command,
    args: sandboxed.args,
    env: sandboxed.env,
    cwd: worktreePath,
  });
}

function requireCwdAtWorkspaceRoot(workspaceRoot: string): void {
  const cwd = path.resolve(process.cwd());
  if (cwd !== path.resolve(workspaceRoot)) {
    throw new GroveError(
      `Run this command from the workspace root: ${workspaceRoot} (current: ${cwd})`,
    );
  }
}

function reportAddMode(
  label: string,
  mode: WorktreeAddMode,
  base: string | null,
  fromOverride: string | null,
): void {
  const c = (s: string) => `\x1b[36m[grove]\x1b[0m ${s}`;
  if (mode === 'reuse') {
    process.stderr.write(c(`reusing existing branch ${label}\n`));
  } else if (mode === 'adopt') {
    process.stderr.write(c(`adopting remote branch for ${label} (tracking origin)\n`));
  } else {
    process.stderr.write(c(`branched ${label}${base ? ` from ${base}` : ''}\n`));
  }
  if (mode !== 'create' && fromOverride) {
    process.stderr.write(
      `\x1b[33m[grove]\x1b[0m --from ${fromOverride} ignored; ${label} already exists\n`,
    );
  }
}

function setupShared(rootDir: string, worktreePath: string, sandbox: SandboxConfig): void {
  const result = ensureSharedLinks({
    rootDir,
    worktreePath,
    shareReadOnly: sandbox.shareReadOnly,
    shareReadWrite: sandbox.shareReadWrite,
  });
  for (const entry of result.linked) {
    process.stderr.write(`\x1b[36m[grove]\x1b[0m linked ${entry}\n`);
  }
  for (const entry of result.repaired) {
    process.stderr.write(`\x1b[36m[grove]\x1b[0m repaired ${entry} (removed stale empty mountpoint)\n`);
  }
  for (const s of result.skipped) {
    process.stderr.write(`\x1b[33m[grove]\x1b[0m skipped ${s.entry} (${s.reason})\n`);
  }
}

function collectSingleGitDirs(repoRoot: string, worktreePath: string): string[] {
  const dirs = [path.join(repoRoot, '.git')];
  const wtGitDir = resolveWorktreeGitDir(worktreePath);
  if (wtGitDir) dirs.push(wtGitDir);
  return dirs;
}

function collectWorkspaceGitDirs(
  workspaceRoot: string,
  repos: WorkspaceRepo[],
  slug: ParsedSlug,
): string[] {
  const dirs: string[] = [];
  for (const repo of repos) {
    dirs.push(path.join(repoAbsPath(workspaceRoot, repo), '.git'));
    const subPath = path.join(workspaceWorktreeDir(workspaceRoot, slug), repo.path);
    const wtGitDir = resolveWorktreeGitDir(subPath);
    if (wtGitDir) dirs.push(wtGitDir);
  }
  return dirs;
}

interface DryRunSingleInput {
  slug: string;
  baseBranch: string | null;
  project: Extract<ProjectContext, { kind: 'single' }>;
  sandboxConfig: SandboxConfig;
  passthrough: string[];
}

function printDryRunSingle(input: DryRunSingleInput): void {
  const lines = [
    `Mode:         single-repo`,
    `Repo root:    ${input.project.repoRoot}`,
    `Config:       ${input.project.configSource}`,
    `Branch:       ${input.slug}`,
    `From:         ${input.baseBranch ?? '(existing branch if present, else current HEAD)'}`,
    `Worktree:     ${input.project.repoRoot}/.grove/${input.slug}`,
    `Sandbox:      ${input.sandboxConfig.enabled ? 'enabled' : 'disabled'}`,
    `Shared (ro):  ${input.sandboxConfig.shareReadOnly.join(', ') || '(none)'}`,
    `Shared (rw):  ${input.sandboxConfig.shareReadWrite.join(', ') || '(none)'}`,
    `Post-create:  ${input.project.config.postCreateCommands.length} command(s)`,
    `Claude:       ${input.project.config.claude.command} ${input.project.config.claude.extraArgs.join(' ')}${
      input.passthrough.length ? ' -- ' + input.passthrough.join(' ') : ''
    }`,
  ];
  process.stdout.write(lines.join('\n') + '\n');
}

interface DryRunWorkspaceInput {
  slug: string;
  project: Extract<ProjectContext, { kind: 'workspace' }>;
  fromOverride: string | null;
  sandboxConfig: SandboxConfig;
  passthrough: string[];
}

function printDryRunWorkspace(input: DryRunWorkspaceInput): void {
  const { workspaceRoot, config } = input.project;
  const lines: string[] = [
    `Mode:         workspace`,
    `Workspace:    ${workspaceRoot}`,
    `Branch:       ${input.slug}`,
    `Worktree:     ${workspaceRoot}/.grove/${input.slug}`,
    `Repos:`,
  ];
  for (const repo of config.repos) {
    const base = input.fromOverride ?? repo.baseBranch ?? '(unset)';
    lines.push(`  • ${repo.name} (${repo.path}) from ${base}`);
  }
  lines.push(`Sandbox:      ${input.sandboxConfig.enabled ? 'enabled' : 'disabled'}`);
  lines.push(`Shared (ro):  ${input.sandboxConfig.shareReadOnly.join(', ') || '(none)'}`);
  lines.push(`Shared (rw):  ${input.sandboxConfig.shareReadWrite.join(', ') || '(none)'}`);
  lines.push(`Post-create:  ${config.postCreateCommands.length} command(s)`);
  lines.push(
    `Claude:       ${config.claude.command} ${config.claude.extraArgs.join(' ')}${
      input.passthrough.length ? ' -- ' + input.passthrough.join(' ') : ''
    }`,
  );
  process.stdout.write(lines.join('\n') + '\n');
}
