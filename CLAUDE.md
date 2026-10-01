# CLAUDE.md

Guidance for Claude Code when working in this repository.

## What This Is

Grove is a single CLI binary, `grove`, that creates a git worktree, optionally runs setup commands, and launches Claude Code inside a sandbox (`bwrap` on Linux; Claude Code's built-in sandbox on macOS). It is local-only, has no daemon, no server, no UI, and no runtime dependencies: it is a single Rust binary (statically linked on Linux) whose only crate dependency is `serde_json`. Worktrees live under `<repo>/.grove/<type>/<name>` and are auto-added to `.git/info/exclude` so they don't pollute `git status`.

## Layout

A single Cargo crate at the repo root; the binary is `grove`.

```
Cargo.toml
rustfmt.toml                      # max_width = 120
src/
├── main.rs                       # argv dispatch, error printing, exit codes
├── types.rs                      # config, project + slug types
├── error.rs                      # Error::{Grove, Other} + bail! macro
├── args.rs                       # strict parseArgs-style flag parsing, split at `--`
├── term.rs                       # stdout!/stderr! (EPIPE-safe), colors, [grove] info/warn/fail
├── paths.rs                      # Node-style join/resolve/relative, which, symlink, fs helpers
├── commands/
│   ├── mod.rs                    # shared helpers incl. launch_claude (argv → sandbox → exec)
│   ├── init.rs                   # write a starter .groverc
│   ├── new.rs                    # create worktree + launch Claude
│   ├── resume.rs                 # re-enter and `claude --continue`
│   ├── sync.rs                   # fetch + fast-forward / --hard reset the worktree branch(es)
│   ├── ls.rs                     # list Grove-managed worktrees
│   ├── rm.rs                     # remove worktree + branch
│   ├── pull.rs                   # fast-forward the base branch
│   ├── completion.rs             # bash/zsh completion script
│   └── complete_slugs.rs         # hidden helper for tab-completion
├── claude.rs                     # build claude argv from .groverc
├── claude_settings.rs            # write .claude/settings.local.json (statusLine, sandbox patch)
├── exec.rs                       # execve into the child (spawn+wait on non-Unix)
├── git.rs                        # thin git process wrapper + worktree porcelain
├── post_create.rs                # run .groverc postCreateCommands
├── project.rs                    # locate repo, parse .groverc, ensure .grove ignored
├── sandbox.rs                    # build bwrap argv / native macOS sandbox patch
├── shared.rs                     # shared-entry symlink chains + bwrap tmpfs overlay
├── status_line.rs                # ANSI status-line text (single + workspace)
└── worktree.rs                   # add/remove/list worktrees (single + workspace)
.github/workflows/
├── ci.yml                        # fmt, clippy -D warnings, tests (Linux/macOS/Windows)
└── release.yml                   # v* tag → static binaries for 5 targets on a GitHub release
```

## Build & run

```bash
cargo build --release
./target/release/grove --help

# install onto your PATH
cargo install --path .

# fully static Linux binary
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
```

`cargo fmt`, `cargo clippy --all-targets -- -D warnings` and `cargo test` must stay clean (CI enforces all three). Unit tests live next to the code in `#[cfg(test)]` modules and cover the pure logic (paths, arg parsing, slug validation, porcelain parsing, settings merge, Claude argv). Process-level behavior (git, bwrap, exec) has no automated tests; exercise it against throwaway repos with a stub `claude.command` and `GROVE_BWRAP_PATH` pointing at a script that prints its argv.

Pushing a `v*` tag runs `release.yml`, which builds Linux x86_64/aarch64 (musl, static), macOS x86_64/arm64 and Windows x86_64 and publishes them with `SHA256SUMS`.

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

Branch resolution lives in `plan_branch` (`src/worktree.rs`) and is shared by single-repo and workspace `new`. It tracks whether grove *created* the branch so rollback only deletes branches grove made — never a pre-existing branch it merely reused.

## `.groverc`

Optional JSON file at the repo root. When absent, the CLI uses defaults (current branch as base, sandbox enabled, no post-create commands, `claude --permission-mode auto`). Auto mode uses Claude Code's classifier to approve actions instead of prompting and falls back to manual mode when unavailable; set `claude.extraArgs` to `["--permission-mode", "bypassPermissions"]` for the old skip-everything behavior.

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
    "extraArgs": ["--permission-mode", "auto"]
  }
}
```

`claude.inheritLocalSettings` (default `true`): on `grove new`, copy `<root>/.claude/settings.local.json` into the fresh worktree (`seed_local_settings`, `src/claude_settings.rs`) before shared links and grove's own settings patch are applied. It's a one-time snapshot: nothing is copied when the worktree already has the file, and later edits on either side stay independent. Because it runs before `ensure_shared_links`, a share entry for that path gets skipped, so grove's writes never follow a symlink into the root's file.

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

Each `shareReadOnly`/`shareReadWrite` entry is wired on disk as a two-hop symlink chain — `<worktree>/<entry>` → `.grove/shared/<entry>` → `<root>/<entry>` — created by `ensure_shared_links` (`src/shared.rs`) on both `new` (before post-create commands run, so `npm install` etc. see the shares) and `resume`. Outside the sandbox the chain resolves to the real content, so host-side `npm run dev` in a worktree just works. Inside bwrap, a tmpfs is mounted over `<worktree>/.grove/shared` and each source is bound at its chain path (`resolve_shared_overlay`), which keeps `realpath` of a shared entry inside the worktree *and* keeps bwrap's auto-created mountpoints in the tmpfs instead of leaking empty files onto the host. A real (non-empty) file or dir already at the worktree path wins — the entry is skipped so the branch's own copy is never clobbered; a zero-byte file or empty dir at a declared share path is treated as leftover mountpoint junk and replaced with the link (this also self-heals worktrees damaged by the earlier bind-at-dest scheme). The `.grove/shared` dir inside the worktree is covered by the existing `/.grove/` entry in `.git/info/exclude`.

## Sandbox

When `sandbox.enabled` is true, `grove new` / `grove resume` sandbox the session per platform: on Linux the whole Claude process is re-execed inside `bwrap`; on macOS Claude runs directly and grove enables Claude Code's *built-in* sandbox instead. `build_sandbox` in `src/sandbox.rs` dispatches on the compile target (`cfg!(target_os)`); any other platform errors out (use `--no-sandbox`).

### Linux (`bwrap`)

- Read-only binds: `/usr`, `/bin`, `/lib`, `/sbin`, `/etc`, `/lib64`, `/run`, `/mnt/wsl`, `/home/linuxbrew` (when present), the repo root, and from `$HOME`: `.gitconfig`, `.config/git`, `.config/glab-cli`, `.config/gh`, `.ssh`, `.local/bin`, `.local/share/claude`, the resolved `node` install prefix (when under `$HOME`), and `$PNPM_HOME`.
- Read-write binds: the worktree directory itself and every relevant `.git` dir. From `$HOME`: `.claude` and `.claude.json` so credentials and session state persist.
- Shared entries: a tmpfs over `<worktree>/.grove/shared`, with each `shareReadOnly` source ro-bound and each `shareReadWrite` source rw-bound at its chain path inside it (see "Shared entries" above).
- Namespace isolation: `--unshare-user --unshare-pid --unshare-uts --unshare-cgroup --die-with-parent`. Network is **not** unshared so Claude can reach the API.
- `--clearenv`, then a curated env with `HOME`, `USER`, `TERM`, `SHELL=/bin/bash`, `LANG`, a constructed `PATH`, `ANTHROPIC_API_KEY` (when set), `SSH_AUTH_SOCK`, `COLORTERM`.
- Override `bwrap` location with `GROVE_BWRAP_PATH`.

### macOS (native Claude Code sandbox)

Wrapping the whole process in a hand-rolled `sandbox-exec` profile kept fighting Claude's own needs (tty raw mode, keychain reads, atomic config writes), so on macOS grove instead launches `claude` directly and enables Claude Code's built-in sandbox (Seatbelt underneath, nothing to install) by merging a patch into `<worktree>/.claude/settings.local.json` — the same file `ensure_status_line` already writes. `build_native_sandbox` (`src/sandbox.rs`) produces the patch; `merge_local_settings` (`src/claude_settings.rs`) deep-merges it (objects merge, arrays/scalars replace) before launch:

- `sandbox.enabled: true` and `autoAllowBashIfSandboxed: true` — Bash commands and all their child processes are OS-confined and run without permission prompts.
- `sandbox.filesystem.allowWrite`: the gitdirs and each `shareReadWrite` source (canonicalized), since those live outside the worktree. The worktree itself and the session temp dir are writable by default; `shareReadOnly` needs nothing because the native default read policy is broad.
- Network: native default — no domains pre-allowed; Claude Code prompts once per new domain.
- `--no-sandbox` writes `sandbox.enabled: false` — settings.local.json persists across launches, so an explicit false must overwrite what a sandboxed run wrote.
- Scope caveat: the native sandbox confines *Bash commands*; the Read/Edit/Write file tools go through the permission system instead (auto-approved by the classifier under the default `auto` mode, unrestricted under `bypassPermissions`). The worktree-pinning system prompt (below) is what keeps the file tools inside the worktree.

Pass `--no-sandbox` to skip sandboxing entirely on either platform.

### Worktree pinning

Because the worktree lives *inside* the main checkout, Claude inherits parent-directory `CLAUDE.md` files and sees symlink targets that resolve to the root, and tends to drift out of the worktree. `grove new`/`grove resume` therefore append `--append-system-prompt` (built in `build_claude_argv`, `src/claude.rs`) telling Claude its worktree is the project root and the main checkout is off-limits. Skipped when the user passes their own `--append-system-prompt` via `claude.extraArgs` or `-- …`.

## Conventions

- Errors that should reach the user as a one-liner are `Error::Grove` (usually via `bail!`, `src/error.rs`) and print as `error: …`; `io::Error` converts into `Error::Other` and prints as `unexpected error: …`. Best-effort operations return `Option`/`bool` (`git_try`, `fetch_remote`) instead of erroring.
- Subcommands parse flags with `args::parse` (strict: unknown flags are an error). `args::split_passthrough` isolates the `-- ...` claude pass-through from the subcommand's own positionals.
- Use `paths::join`/`resolve`/`relative` rather than `PathBuf::join` for config-derived paths: they normalize `..` and keep an absolute tail inside the base, like Node's `path` did.
- Write output with the `stdout!`/`stderr!` macros (`src/term.rs`), never `print!`, so a closed pipe can't panic; `[grove]` progress lines go through `term::info`/`warn`/`fail` on stderr.
- `new`/`resume` end by `exec`ing into bwrap/claude: nothing may run after `launch_claude`, and the child's exit status is grove's.
- Worktrees are nested as `.grove/<type>/<name>/` (e.g. `.grove/feat/auth-flow/`) so the empty type dirs get pruned on `rm`.
- The CLI never writes to a central state file; everything is derived from `git worktree list --porcelain` plus filesystem walks under `.grove/`.
