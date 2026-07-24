# CLAUDE.md

Guidance for Claude Code when working in this repository.

## What This Is

Grove is a single CLI binary, `grove`, that creates a git worktree, optionally runs setup commands, and launches Claude Code inside a sandbox (`bwrap` on Linux; Claude Code's built-in sandbox on macOS). It is local-only, has no daemon, no server, no UI, and zero runtime dependencies — only Node's built-ins. Worktrees live under `<repo>/.grove/<type>/<name>` and are auto-added to `.git/info/exclude` so they don't pollute `git status`.

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
│       ├── shared.ts             # shared-entry symlink chains + bwrap tmpfs overlay
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

## Shared entries

Each `shareReadOnly`/`shareReadWrite` entry is wired on disk as a two-hop symlink chain — `<worktree>/<entry>` → `.grove/shared/<entry>` → `<root>/<entry>` — created by `ensureSharedLinks` (`lib/shared.ts`) on both `new` (before post-create commands run, so `npm install` etc. see the shares) and `resume`. Outside the sandbox the chain resolves to the real content, so host-side `npm run dev` in a worktree just works. Inside bwrap, a tmpfs is mounted over `<worktree>/.grove/shared` and each source is bound at its chain path (`resolveSharedOverlay`), which keeps `realpath` of a shared entry inside the worktree *and* keeps bwrap's auto-created mountpoints in the tmpfs instead of leaking empty files onto the host. A real (non-empty) file or dir already at the worktree path wins — the entry is skipped so the branch's own copy is never clobbered; a zero-byte file or empty dir at a declared share path is treated as leftover mountpoint junk and replaced with the link (this also self-heals worktrees damaged by the earlier bind-at-dest scheme). The `.grove/shared` dir inside the worktree is covered by the existing `/.grove/` entry in `.git/info/exclude`.

## Sandbox

When `sandbox.enabled` is true, `grove new` / `grove resume` sandbox the session per platform: on Linux the whole Claude process is re-execed inside `bwrap`; on macOS Claude runs directly and grove enables Claude Code's *built-in* sandbox instead. `buildSandbox` in `lib/sandbox.ts` dispatches on `process.platform`; any other platform errors out (use `--no-sandbox`).

### Linux (`bwrap`)

- Read-only binds: `/usr`, `/bin`, `/lib`, `/sbin`, `/etc`, `/lib64`, `/run`, `/mnt/wsl`, `/home/linuxbrew` (when present), the repo root, and from `$HOME`: `.gitconfig`, `.config/git`, `.config/glab-cli`, `.config/gh`, `.ssh`, `.local/bin`, `.local/share/claude`, the resolved `node` install prefix (when under `$HOME`), and `$PNPM_HOME`.
- Read-write binds: the worktree directory itself and every relevant `.git` dir. From `$HOME`: `.claude` and `.claude.json` so credentials and session state persist.
- Shared entries: a tmpfs over `<worktree>/.grove/shared`, with each `shareReadOnly` source ro-bound and each `shareReadWrite` source rw-bound at its chain path inside it (see "Shared entries" above).
- Namespace isolation: `--unshare-user --unshare-pid --unshare-uts --unshare-cgroup --die-with-parent`. Network is **not** unshared so Claude can reach the API.
- `--clearenv`, then a curated env with `HOME`, `USER`, `TERM`, `SHELL=/bin/bash`, `LANG`, a constructed `PATH`, `ANTHROPIC_API_KEY` (when set), `SSH_AUTH_SOCK`, `COLORTERM`.
- Override `bwrap` location with `GROVE_BWRAP_PATH`.

### macOS (native Claude Code sandbox)

Wrapping the whole process in a hand-rolled `sandbox-exec` profile kept fighting Claude's own needs (tty raw mode, keychain reads, atomic config writes), so on macOS grove instead launches `claude` directly and enables Claude Code's built-in sandbox (Seatbelt underneath, nothing to install) by merging a patch into `<worktree>/.claude/settings.local.json` — the same file `ensureStatusLine` already writes. `buildNativeSandbox` (`lib/sandbox.ts`) produces the patch; `mergeLocalSettings` (`lib/claude-settings.ts`) deep-merges it (objects merge, arrays/scalars replace) before launch:

- `sandbox.enabled: true` and `autoAllowBashIfSandboxed: true` — Bash commands and all their child processes are OS-confined and run without permission prompts.
- `sandbox.filesystem.allowWrite`: the gitdirs and each `shareReadWrite` source (canonicalized), since those live outside the worktree. The worktree itself and the session temp dir are writable by default; `shareReadOnly` needs nothing because the native default read policy is broad.
- Network: native default — no domains pre-allowed; Claude Code prompts once per new domain.
- `--no-sandbox` writes `sandbox.enabled: false` — settings.local.json persists across launches, so an explicit false must overwrite what a sandboxed run wrote.
- Scope caveat: the native sandbox confines *Bash commands*; the Read/Edit/Write file tools go through the permission system instead (unrestricted under `bypassPermissions`). The worktree-pinning system prompt (below) is what keeps the file tools inside the worktree.

Pass `--no-sandbox` to skip sandboxing entirely on either platform.

### Worktree pinning

Because the worktree lives *inside* the main checkout, Claude inherits parent-directory `CLAUDE.md` files and sees symlink targets that resolve to the root, and tends to drift out of the worktree. `grove new`/`grove resume` therefore append `--append-system-prompt` (built in `buildClaudeArgv`, `lib/claude.ts`) telling Claude its worktree is the project root and the main checkout is off-limits. Skipped when the user passes their own `--append-system-prompt` via `claude.extraArgs` or `-- …`.

## Conventions

- Local imports use the `.js` suffix (TypeScript `module: "nodenext"`).
- Errors that should reach the user as a one-liner are thrown as `GroveError` (from `lib/project.ts`); anything else surfaces as a stack trace.
- Subcommands are dispatched via `node:util.parseArgs` with `strict: true`. The `splitPassthrough` helper isolates the `-- ...` claude pass-through from the subcommand's own positionals.
- Worktrees are nested as `.grove/<type>/<name>/` (e.g. `.grove/feat/auth-flow/`) so the empty type dirs get pruned on `rm`.
- The CLI never writes to a central state file; everything is derived from `git worktree list --porcelain` plus filesystem walks under `.grove/`.
