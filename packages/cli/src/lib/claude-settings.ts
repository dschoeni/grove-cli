import * as fs from 'node:fs';
import * as path from 'node:path';

/**
 * Ensure `<worktreeDir>/.claude/settings.local.json` has a statusLine command
 * that prints `text`. Preserves any other keys already in the file.
 *
 * `.claude/settings.local.json` is the per-project, gitignored override Claude
 * Code reads from cwd at startup. Writing it here means the launched agent
 * shows the slug in its status bar without polluting the committed settings.
 */
export function ensureStatusLine(worktreeDir: string, text: string): void {
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

  settings.statusLine = {
    type: 'command',
    command: `printf '%s' ${shellQuote(text)}`,
  };

  fs.mkdirSync(claudeDir, { recursive: true });
  fs.writeFileSync(settingsPath, JSON.stringify(settings, null, 2) + '\n', 'utf-8');
}

function shellQuote(s: string): string {
  return `'${s.replace(/'/g, `'\\''`)}'`;
}
