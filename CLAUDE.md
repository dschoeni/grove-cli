# CLAUDE.md

Guidance for Claude Code when working in this repository.

## What This Is

Grove is a single CLI binary, `grove`, that creates a git worktree, optionally runs setup commands, and launches Claude Code inside a sandbox (`bwrap` on Linux, `sandbox-exec`/Seatbelt on macOS). It is local-only, has no daemon, no server, no UI, and zero runtime dependencies — only Node's built-ins. Worktrees live under `<repo>/.grove/<type>/<name>` and are auto-added to `.git/info/exclude` so they don't pollute `git status`.

## Layout

Monorepo with one workspace package: `packages/cli`. `pnpm-workspace.yaml` keeps the `packages/*` glob in case more get added.

```
packages/cli/
├── bin/grove                     # node shim → dist/cli.js
├── src/
│   ├── cli.ts                    # argv dispatch
│   ├── types.ts                  # config + slug types
│   ├── commands/
│   │   ├── init.ts               # write a starter .groverc
│   │   ├── new.ts                # create worktree + launch Claude
│   │   ├── resume.ts             # re-enter and `claude --continue`
│   │   ├── sync.ts               # fetch + fast-forward / --hard reset the worktree branch(es)
│   │   ├── ls.ts                 # list Grove-managed worktrees
│   │   ├── rm.ts                 # remove worktree + branch
│   │   ├── completion.ts         # bash/zsh completion script
│   │   └── complete-slugs.ts     # hidden helper for tab-completion
│   └── lib/
│       ├── argv.ts               # split argv at standalone `--`
│       ├── claude.ts             # build claude argv from .groverc
│       ├── claude-settings.ts    # write .claude/settings.local.json (statusLine)
│       ├── exec.ts               # spawn child with inherit + signal forwarding
│       ├── git.ts                # thin execFile wrapper + worktree porcelain
│       ├── post-create.ts        # run .groverc postCreateCommands
│       ├── project.ts            # locate repo, parse .groverc, ensure .grove ignored
│       ├── sandbox.ts            # build bwrap argv (mounts, env, tools)
│       ├── shared.ts             # symlink shareReadOnly/shareReadWrite into worktree
│       ├── status-line.ts        # ANSI status-line text (single + workspace)
│       └── worktree.ts           # add/remove/list worktrees (single + workspace)
├── package.json
└── tsconfig.json
```

## Build & run

```bash
pnpm install
pnpm -C packages/cli build       # tsc → dist/
node packages/cli/bin/grove --help

# or symlink onto your PATH
ln -s "$PWD/packages/cli/bin/grove" ~/.local/bin/grove
```

`pnpm -C packages/cli lint` is `tsc --noEmit`. There is no test suite at the moment.

## Commands

| Command | What it does |
|---------|--------------|
| `grove init` | Write a starter `.groverc` at the repo root and add `/.grove/` to `.git/info/exclude`. |
| `grove new <slug>` | Create `.grove/<type>/<name>`, run post-create commands, launch Claude in a bwrap sandbox. `<slug>` must start with `feat/`, `fix/`, or `chore/`. Reuses an existing local branch, or adopts `origin/<slug>` as a tracking branch, before cutting a fresh one from the base. |
| `grove resume <slug>` | Re-enter an existing worktree and run `claude --continue`. |
| `grove sync <slug> [--hard]` | Fetch and reconcile the worktree branch(es) with their remote: fast-forward when possible, `--hard` to reset after a force-push. Workspace mode syncs every repo. |
| `grove ls` | List Grove-managed worktrees in the current project. |
| `grove rm <slug> [--force]` | Remove the worktree and delete the branch. |
| `grove pull [branch] [--remote <name>]` | Fast-forward the base branch (`.groverc` `baseBranch`, or per-repo in workspace mode) to its latest remote state. Fast-forward only; updates in place when checked out, else advances the ref directly. |
| `grove completion <bash\|zsh>` | Print a completion script. |

Flags: `grove new` accepts `--from <branch>`, `--fetch`, `--no-sandbox`, `--keep-on-failure`, `--dry-run`, and `-- <args>` to forward to `claude`. `grove sync` accepts `--hard`, `--remote <name>`, `--dry-run`. Run `grove <cmd> --help` for the per-command flags.

Branch resolution lives in `planBranch` (`lib/worktree.ts`) and is shared by single-repo and workspace `new`. It tracks whether grove *created* the branch so rollback only deletes branches grove made — never a pre-existing branch it merely reused.

## `.groverc`

Optional JSON file at the repo root. When absent, the CLI uses defaults (current branch as base, sandbox enabled, no post-create commands, `claude --permission-mode bypassPermissions`).

### Single-repo schema

```jsonc
{
  "baseBranch": "main",
  "sandbox": {
    "enabled": true,
    "shareReadOnly": [".env"],
    "shareReadWrite": ["node_modules"]
  },
  "postCreateCommands": ["pnpm install"],
  "claude": {
    "command": "claude",
    "extraArgs": ["--permission-mode", "bypassPermissions"]
  }
}
```

### Workspace schema

For multi-repo workspaces, set `type: "workspace"` and list `repos`. Every `grove new <slug>` then creates one worktree per repo under `<workspace>/.grove/<type>/<name>/<repo.path>`.

```jsonc
{
  "type": "workspace",
  "repos": [
    { "name": "web", "path": "apps/web", "baseBranch": "development" },
    { "name": "api", "path": "apps/api", "baseBranch": "development" }
  ],
  "sandbox": { "enabled": true, "shareReadOnly": ["package.json", "node_modules"] },
  "postCreateCommands": ["npm install"],
  "claude": { "command": "claude", "extraArgs": [] }
}
```

`grove new` and `grove rm` in workspace mode must be run from the workspace root.

## Sandbox

When `sandbox.enabled` is true, `grove new` / `grove resume` re-exec Claude inside a sandbox chosen by platform: `bwrap` on Linux, `sandbox-exec` (Apple Seatbelt) on macOS. `buildSandbox` in `lib/sandbox.ts` dispatches on `process.platform`; any other platform errors out (use `--no-sandbox`). Both paths confine the agent's reads and writes to its worktree.

### Linux (`bwrap`)

- Read-only binds: `/usr`, `/bin`, `/lib`, `/sbin`, `/etc`, `/lib64`, `/run`, `/mnt/wsl`, `/home/linuxbrew` (when present), the repo root, and from `$HOME`: `.gitconfig`, `.config/git`, `.config/glab-cli`, `.config/gh`, `.ssh`, `.local/bin`, `.local/share/claude`, the resolved `node` install prefix (when under `$HOME`), and `$PNPM_HOME`.
- Read-write binds: the worktree directory itself, every relevant `.git` dir, plus anything in `sandbox.shareReadWrite`. From `$HOME`: `.claude` and `.claude.json` so credentials and session state persist.
- Namespace isolation: `--unshare-user --unshare-pid --unshare-uts --unshare-cgroup --die-with-parent`. Network is **not** unshared so Claude can reach the API.
- `--clearenv`, then a curated env with `HOME`, `USER`, `TERM`, `SHELL=/bin/bash`, `LANG`, a constructed `PATH`, `ANTHROPIC_API_KEY` (when set), `SSH_AUTH_SOCK`, `COLORTERM`.
- Override `bwrap` location with `GROVE_BWRAP_PATH`.

### macOS (`sandbox-exec`)

Seatbelt can't remount the filesystem the way `bwrap` does, so Grove generates a deny-by-default SBPL profile (passed inline via `-p`) and re-allows just what a dev session needs:

- `(deny default)`, then `process-exec*`, `process-fork`, `signal (target self)`, `sysctl-read`, `mach-lookup`, `ipc-posix-shm`, `iokit-open`, `system-socket`, and `network*` (so Claude reaches the API).
- **Reads** (`file-read*`) are confined to system paths needed to load binaries (`/usr`, `/System`, `/Library`, `/bin`, `/sbin`, `/opt`, `/dev`, `/private/etc`, `/private/var/{db,run,folders}`), home-local tool installs (`.local/bin`, `.local/share/claude`, the `node` prefix, `$PNPM_HOME`), auth (`.gitconfig`, `.config/git`, `.config/gh`, `.config/glab-cli`, `.ssh`, the `SSH_AUTH_SOCK` dir), `sandbox.shareReadOnly`, and everything in the write set below. The agent **cannot read file contents outside its worktree.**
- **Writes** (`file-write*`) are limited to the worktree, the gitdirs, `sandbox.shareReadWrite`, `~/.claude`, `~/.claude.json`, the temp dirs (`/private/tmp`, `/private/var/folders`, `$TMPDIR`), and `/dev`.
- `file-read-metadata` is allowed globally because the kernel must stat ancestor path components to resolve any path — so existence/size of arbitrary paths leaks, but file *contents* outside the allowed subpaths do not. All subpaths are canonicalized (`realpath`) so symlinked roots like `/tmp` → `/private/tmp` match. The spawned process inherits the same curated env as the Linux path (plus `TMPDIR`).
- Override `sandbox-exec` location with `GROVE_SANDBOX_EXEC_PATH`.

Pass `--no-sandbox` to skip sandboxing entirely on either platform.

## Conventions

- Local imports use the `.js` suffix (TypeScript `module: "nodenext"`).
- Errors that should reach the user as a one-liner are thrown as `GroveError` (from `lib/project.ts`); anything else surfaces as a stack trace.
- Subcommands are dispatched via `node:util.parseArgs` with `strict: true`. The `splitPassthrough` helper isolates the `-- ...` claude pass-through from the subcommand's own positionals.
- Worktrees are nested as `.grove/<type>/<name>/` (e.g. `.grove/feat/auth-flow/`) so the empty type dirs get pruned on `rm`.
- The CLI never writes to a central state file; everything is derived from `git worktree list --porcelain` plus filesystem walks under `.grove/`.
