import { parseArgs } from 'node:util';
import * as path from 'node:path';
import { loadProject, GroveError } from '../lib/project.js';
import {
  parseSlug,
  findGroveWorktree,
  findWorkspaceWorktree,
  workspaceWorktreeDir,
  repoAbsPath,
} from '../lib/worktree.js';
import { buildSandbox, resolveWorktreeGitDir } from '../lib/sandbox.js';
import { buildClaudeArgv } from '../lib/claude.js';
import { ensureStatusLine } from '../lib/claude-settings.js';
import { workspaceStatusLineText, singleStatusLineText } from '../lib/status-line.js';
import { execInteractive } from '../lib/exec.js';
import { splitPassthrough } from '../lib/argv.js';

const HELP = `\
grove resume — re-enter an existing worktree and continue the last Claude session

Usage:
  grove resume <slug> [--no-sandbox] [-- <claude-args>…]

Flags:
  --no-sandbox           Skip bwrap.
  --                     Stop flag parsing; remaining args are passed to claude.
`;

export async function runResume(argv: string[]): Promise<number> {
  const { args, passthrough } = splitPassthrough(argv);

  const { values, positionals } = parseArgs({
    args,
    options: {
      help: { type: 'boolean', short: 'h' },
      'no-sandbox': { type: 'boolean' },
    },
    strict: true,
    allowPositionals: true,
  });

  if (values.help) {
    process.stdout.write(HELP);
    return 0;
  }
  if (positionals.length !== 1) {
    throw new GroveError('Usage: grove resume <slug>');
  }

  const slug = parseSlug(positionals[0]!);
  const project = loadProject();

  const sandboxConfig = {
    ...project.config.sandbox,
    enabled: project.config.sandbox.enabled && !values['no-sandbox'],
  };

  const claude = buildClaudeArgv({
    claude: project.config.claude,
    passthrough,
    resume: true,
  });

  let rootDir: string;
  let worktreePath: string;
  let gitDirs: string[];

  if (project.kind === 'workspace') {
    const row = findWorkspaceWorktree(project.workspaceRoot, project.config.repos, slug);
    if (!row) {
      throw new GroveError(`No Grove worktree found for slug "${slug.full}"`);
    }
    rootDir = project.workspaceRoot;
    worktreePath = workspaceWorktreeDir(project.workspaceRoot, slug);
    gitDirs = collectWorkspaceGitDirs(project.workspaceRoot, project.config.repos, row);
    ensureStatusLine(worktreePath, workspaceStatusLineText(project.workspaceRoot, slug.full));
  } else {
    const wt = findGroveWorktree(project.repoRoot, slug);
    if (!wt) {
      throw new GroveError(`No Grove worktree found for slug "${slug.full}"`);
    }
    rootDir = project.repoRoot;
    worktreePath = wt.path;
    gitDirs = collectSingleGitDirs(project.repoRoot, wt.path);
    ensureStatusLine(worktreePath, singleStatusLineText(slug.full));
  }

  const sandboxed = buildSandbox({
    rootDir,
    worktreePath,
    gitDirs,
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

function collectSingleGitDirs(repoRoot: string, worktreePath: string): string[] {
  const dirs: string[] = [path.join(repoRoot, '.git')];
  const wtGitDir = resolveWorktreeGitDir(worktreePath);
  if (wtGitDir) dirs.push(wtGitDir);
  return dirs;
}

function collectWorkspaceGitDirs(
  workspaceRoot: string,
  repos: import('../types.js').WorkspaceRepo[],
  row: import('../lib/worktree.js').WorkspaceWorktreeRow,
): string[] {
  const dirs: string[] = [];
  for (const repo of repos) {
    dirs.push(path.join(repoAbsPath(workspaceRoot, repo), '.git'));
  }
  for (const r of row.perRepo) {
    if (!r.registered) continue;
    const wtGitDir = resolveWorktreeGitDir(r.path);
    if (wtGitDir) dirs.push(wtGitDir);
  }
  return dirs;
}
