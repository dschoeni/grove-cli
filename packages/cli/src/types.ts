export type BranchType = 'feat' | 'fix' | 'chore';

export const BRANCH_TYPES: readonly BranchType[] = ['feat', 'fix', 'chore'];

export interface SandboxConfig {
  enabled: boolean;
  shareReadOnly: string[];
  shareReadWrite: string[];
}

export interface ClaudeConfig {
  command: string;
  extraArgs: string[];
  /** Seed a new worktree's `.claude/settings.local.json` from the root's copy. */
  inheritLocalSettings: boolean;
}

export interface GroveConfig {
  baseBranch: string | null;
  sandbox: SandboxConfig;
  postCreateCommands: string[];
  claude: ClaudeConfig;
}

export interface WorkspaceRepo {
  /** Human label, used in CLI output. */
  name: string;
  /** Path relative to the workspace root, e.g. "apps/web". */
  path: string;
  /** Default base branch for new worktrees of this repo. */
  baseBranch: string | null;
}

export interface WorkspaceConfig {
  repos: WorkspaceRepo[];
  sandbox: SandboxConfig;
  postCreateCommands: string[];
  claude: ClaudeConfig;
}

export type ProjectContext =
  | {
      kind: 'single';
      repoRoot: string;
      config: GroveConfig;
      configSource: 'groverc' | 'defaults';
    }
  | {
      kind: 'workspace';
      workspaceRoot: string;
      config: WorkspaceConfig;
      configSource: 'groverc';
    };

export interface WorktreeRow {
  branch: string;
  path: string;
  head: string;
}
