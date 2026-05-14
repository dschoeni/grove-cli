import * as fs from 'node:fs';
import * as path from 'node:path';

export interface SymlinkSharedInput {
  repoRoot: string;
  worktreePath: string;
  shareReadOnly: string[];
  shareReadWrite: string[];
}

export interface SymlinkResult {
  created: string[];
  skipped: { entry: string; reason: string }[];
}

export function symlinkShared(input: SymlinkSharedInput): SymlinkResult {
  const created: string[] = [];
  const skipped: { entry: string; reason: string }[] = [];
  const all = [...input.shareReadOnly, ...input.shareReadWrite];

  for (const entry of all) {
    const source = path.resolve(input.repoRoot, entry);
    const target = path.join(input.worktreePath, entry);

    if (!fs.existsSync(source)) {
      skipped.push({ entry, reason: 'source missing' });
      continue;
    }
    if (fs.existsSync(target) || isSymlink(target)) {
      skipped.push({ entry, reason: 'target exists' });
      continue;
    }
    const parent = path.dirname(target);
    fs.mkdirSync(parent, { recursive: true });
    fs.symlinkSync(source, target);
    created.push(entry);
  }

  return { created, skipped };
}

function isSymlink(p: string): boolean {
  try {
    return fs.lstatSync(p).isSymbolicLink();
  } catch {
    return false;
  }
}
