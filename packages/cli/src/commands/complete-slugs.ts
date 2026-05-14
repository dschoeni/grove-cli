import * as path from 'node:path';
import { loadProject } from '../lib/project.js';
import { listGroveWorktrees, listWorkspaceWorktrees } from '../lib/worktree.js';

/**
 * Hidden helper invoked by shell completion. Prints one slug per line of every
 * Grove worktree known in the current project. Always exits 0 and stays silent
 * on errors so a transient failure doesn't break tab-completion.
 */
export function runCompleteSlugs(): void {
  try {
    const project = loadProject();
    if (project.kind === 'workspace') {
      const rows = listWorkspaceWorktrees(project.workspaceRoot, project.config.repos);
      for (const r of rows) process.stdout.write(r.slug + '\n');
    } else {
      const groveDir = path.join(project.repoRoot, '.grove');
      for (const w of listGroveWorktrees(project.repoRoot)) {
        process.stdout.write(path.relative(groveDir, w.path) + '\n');
      }
    }
  } catch {
    // silent
  }
}
