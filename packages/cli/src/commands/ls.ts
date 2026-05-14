import * as path from 'node:path';
import { parseArgs } from 'node:util';
import { loadProject } from '../lib/project.js';
import { listGroveWorktrees, listWorkspaceWorktrees } from '../lib/worktree.js';

const HELP = `\
grove ls — list Grove-managed worktrees in the current repo
`;

export function runLs(argv: string[]): void {
  const { values } = parseArgs({
    args: argv,
    options: { help: { type: 'boolean', short: 'h' } },
    strict: true,
    allowPositionals: false,
  });
  if (values.help) {
    process.stdout.write(HELP);
    return;
  }

  const project = loadProject();

  if (project.kind === 'workspace') {
    runLsWorkspace(project.workspaceRoot, project.config.repos);
    return;
  }
  runLsSingle(project.repoRoot);
}

function runLsSingle(repoRoot: string): void {
  const worktrees = listGroveWorktrees(repoRoot);
  if (worktrees.length === 0) {
    process.stdout.write('No Grove worktrees in this repo.\n');
    return;
  }

  const rows = worktrees.map((w) => ({
    slug: path.relative(path.join(repoRoot, '.grove'), w.path),
    branch: w.branch ?? '(detached)',
    path: w.path,
  }));

  printTable(['SLUG', 'BRANCH', 'PATH'], rows.map((r) => [r.slug, r.branch, r.path]));
}

function runLsWorkspace(
  workspaceRoot: string,
  repos: import('../types.js').WorkspaceRepo[],
): void {
  const rows = listWorkspaceWorktrees(workspaceRoot, repos);
  if (rows.length === 0) {
    process.stdout.write('No Grove worktrees in this workspace.\n');
    return;
  }

  const tableRows: string[][] = [];
  for (const row of rows) {
    const branches = new Set(
      row.perRepo.filter((r) => r.branch).map((r) => r.branch!),
    );
    const branchLabel =
      branches.size === 0
        ? '(missing)'
        : branches.size === 1
          ? Array.from(branches)[0]!
          : `mixed: ${Array.from(branches).join(', ')}`;
    const present = row.perRepo.filter((r) => r.registered).map((r) => r.repo.name);
    const reposLabel =
      present.length === row.perRepo.length
        ? `${present.length}/${row.perRepo.length}`
        : `${present.length}/${row.perRepo.length} (${present.join(',')})`;
    tableRows.push([row.slug, branchLabel, reposLabel, row.workspaceDir]);
  }
  printTable(['SLUG', 'BRANCH', 'REPOS', 'PATH'], tableRows);
}

function printTable(headers: string[], rows: string[][]): void {
  const widths = headers.map((h, i) =>
    Math.max(h.length, ...rows.map((r) => (r[i] ?? '').length)),
  );
  const pad = (s: string, w: number) => s + ' '.repeat(Math.max(0, w - s.length));
  const fmt = (cells: string[]) =>
    cells.map((c, i) => (i === cells.length - 1 ? c : pad(c, widths[i] ?? 0))).join('  ');
  process.stdout.write(fmt(headers) + '\n');
  for (const r of rows) process.stdout.write(fmt(r) + '\n');
}
