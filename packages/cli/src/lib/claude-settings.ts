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
