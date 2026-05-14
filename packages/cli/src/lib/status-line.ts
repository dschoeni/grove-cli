import * as path from 'node:path';

const CYAN = '\x1b[36m';
const DIM = '\x1b[2m';
const BOLD = '\x1b[1m';
const RESET = '\x1b[0m';

export function singleStatusLineText(slug: string): string {
  return `${CYAN}grove${RESET}: ${BOLD}${slug}${RESET}`;
}

export function workspaceStatusLineText(workspaceRoot: string, slug: string): string {
  const wsName = path.basename(workspaceRoot);
  return `${CYAN}grove${RESET}${DIM}[${wsName}]${RESET}: ${BOLD}${slug}${RESET}`;
}
