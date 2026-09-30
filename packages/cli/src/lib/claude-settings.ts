import * as fs from 'node:fs';
import * as path from 'node:path';

/**
 * Deep-merge `patch` into `<worktreeDir>/.claude/settings.local.json`,
 * preserving any keys the patch doesn't touch. Nested plain objects merge;
 * arrays and scalars are replaced wholesale (so grove-managed lists like
 * `sandbox.filesystem.allowWrite` reflect the current config, not an
 * accumulation of past runs).
 *
 * `.claude/settings.local.json` is the per-project, gitignored override Claude
 * Code reads from cwd at startup — the right place for per-worktree launch
 * state without polluting the committed settings.
 */
export function mergeLocalSettings(worktreeDir: string, patch: Record<string, unknown>): void {
  const claudeDir = path.join(worktreeDir, '.claude');
  const settingsPath = path.join(claudeDir, 'settings.local.json');

  let settings: Record<string, unknown> = {};
  if (fs.existsSync(settingsPath)) {
    try {
      const parsed = JSON.parse(fs.readFileSync(settingsPath, 'utf-8'));
      if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
        settings = parsed as Record<string, unknown>;
      }
    } catch {
      // Malformed — fall through and overwrite.
    }
  }

  deepMerge(settings, patch);

  fs.mkdirSync(claudeDir, { recursive: true });
  fs.writeFileSync(settingsPath, JSON.stringify(settings, null, 2) + '\n', 'utf-8');
}

/**
 * Copy `<rootDir>/.claude/settings.local.json` into a freshly created worktree
 * so local-only settings (permission allowlists, env, …) carry over. A one-time
 * snapshot: later edits on either side stay independent. Skipped when the root
 * has no such file or the worktree already has one. Returns true if copied.
 *
 * Must run before `ensureSharedLinks`, so a real file occupies the path and a
 * share entry for it is skipped — otherwise grove's own settings writes would
 * follow the symlink chain into the root's file.
 */
export function seedLocalSettings(rootDir: string, worktreeDir: string): boolean {
  const src = path.join(rootDir, '.claude', 'settings.local.json');
  const dest = path.join(worktreeDir, '.claude', 'settings.local.json');
  if (!fs.existsSync(src) || fs.existsSync(dest)) return false;
  fs.mkdirSync(path.dirname(dest), { recursive: true });
  fs.copyFileSync(src, dest);
  return true;
}

/** Ensure the settings file has a statusLine command that prints `text`. */
export function ensureStatusLine(worktreeDir: string, text: string): void {
  mergeLocalSettings(worktreeDir, {
    statusLine: {
      type: 'command',
      command: `printf '%s' ${shellQuote(text)}`,
    },
  });
}

function deepMerge(target: Record<string, unknown>, patch: Record<string, unknown>): void {
  for (const [key, value] of Object.entries(patch)) {
    const existing = target[key];
    if (isPlainObject(existing) && isPlainObject(value)) {
      deepMerge(existing, value);
    } else {
      target[key] = value;
    }
  }
}

function isPlainObject(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null && !Array.isArray(v);
}

function shellQuote(s: string): string {
  return `'${s.replace(/'/g, `'\\''`)}'`;
}
