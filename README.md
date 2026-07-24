# Grove

A small CLI that creates a git worktree, runs your setup, and launches Claude Code inside a sandbox — `bwrap` on Linux, Claude Code's built-in sandbox on macOS. Local-only, no daemon, zero runtime dependencies.

```bash
grove new feat/auth-flow
```

…creates `<repo>/.grove/feat/auth-flow/`, branches off your default base, runs anything in `postCreateCommands`, and drops you into a sandboxed Claude session in that worktree.

## Install

Requires Node ≥ 20 and `pnpm`. On Linux, `bwrap` is needed for sandboxing (apt: `bubblewrap`); on macOS nothing extra is needed. Use `--no-sandbox` if you don't want sandboxing.

```bash
pnpm install
pnpm -C packages/cli build
ln -s "$PWD/packages/cli/bin/grove" ~/.local/bin/grove
```

Optional shell completion:

```bash
echo 'source <(grove completion bash)' >> ~/.bashrc
# or zsh:
grove completion zsh > "${fpath[1]}/_grove"
```

## Commands

```
grove init                       # write a starter .groverc
grove new <slug> [flags] [-- …]  # create worktree + launch Claude
grove resume <slug> [flags]      # re-enter and `claude --continue`
grove sync <slug> [flags]        # update the worktree branch(es) from their remote
grove ls                         # list Grove-managed worktrees
grove rm <slug> [--force]        # remove worktree + branch
grove completion <bash|zsh>      # print a completion script
```

`<slug>` must start with one of `feat/`, `fix/`, `chore/`. Anything after `--` is passed straight through to `claude`.

`grove new` flags: `--from <branch>` overrides the base branch, `--fetch` fetches the default remote first (so the base and `origin/<slug>` are current), `--no-sandbox` skips the sandbox, `--keep-on-failure` leaves a half-set-up worktree in place if `postCreateCommands` exit non-zero, `--dry-run` prints the plan and exits.

**Branch reuse.** If a branch matching `<slug>` already exists, `grove new` reuses it instead of erroring: a local branch is checked out as-is, otherwise a local branch tracking `origin/<slug>` is created. Only when neither exists is a fresh branch cut from the base (so `--from` is ignored on reuse/adopt). A branch that's already checked out in another worktree is refused with a pointer to `grove resume`.

## Keeping a worktree in sync

`grove sync <slug>` fetches and reconciles the worktree's branch with its remote. It fast-forwards when the remote is strictly ahead, and leaves diverged branches untouched — telling you to pass `--hard`, which `reset --hard`s the branch onto its upstream. That's the fix after someone force-pushes a shared branch. In a workspace it syncs every repo of the worktree independently and prints a per-repo summary.

```bash
grove sync feat/auth-flow            # fast-forward if possible
grove sync feat/auth-flow --hard     # reset to origin after a force-push
grove sync feat/auth-flow --dry-run  # report what each branch would do
```

Flags: `--hard` resets (discarding local divergence), `--remote <name>` overrides the remote (defaults to the branch's upstream remote, else `origin`), `--dry-run` reports without mutating.

## `.groverc`

Optional JSON file at the repo root. With no file present, defaults are: current branch as base, sandbox enabled, no post-create commands, `claude --permission-mode bypassPermissions`.

### Single repo

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

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `baseBranch` | `string` | current HEAD | Branch new worktrees fork from. Override per-call with `--from`. |
| `sandbox.enabled` | `boolean` | `true` | When false, `claude` runs directly in the worktree (same as `--no-sandbox`). |
| `sandbox.shareReadOnly` | `string[]` | `[]` | Repo-relative paths bind-mounted read-only into the sandbox **and** symlinked into the worktree. |
| `sandbox.shareReadWrite` | `string[]` | `[]` | Same, but bind-mounted read-write. Use for `node_modules` etc. |
| `postCreateCommands` | `string[]` | `[]` | Shell commands run sequentially in the new worktree. Non-zero exit aborts (or leaves it in place with `--keep-on-failure`). |
| `claude.command` | `string` | `"claude"` | Binary to launch. |
| `claude.extraArgs` | `string[]` | `["--permission-mode","bypassPermissions"]` | Extra flags prepended to Claude's argv. |

### Workspace (multi-repo)

Set `type: "workspace"` and list `repos`. Each `grove new <slug>` then creates one worktree per repo at `<workspace>/.grove/<type>/<name>/<repo.path>`. Run `grove new` / `grove rm` from the workspace root.

```jsonc
{
  "type": "workspace",
  "repos": [
    { "name": "web", "path": "apps/web", "baseBranch": "development" },
    { "name": "api", "path": "apps/api", "baseBranch": "development" }
  ],
  "sandbox": {
    "enabled": true,
    "shareReadOnly": ["package.json", "node_modules", "tsconfig.json", "docker-compose.yml"]
  },
  "postCreateCommands": ["npm install"],
  "claude": { "command": "claude", "extraArgs": [] }
}
```

`shareReadOnly` / `shareReadWrite` entries are also symlinked into the worktree skeleton, so workspace-level `package.json`, `node_modules`, etc. resolve from inside the new directory without being copied.

## Sandbox

`bwrap` is invoked with namespace isolation (`--unshare-{user,pid,uts,cgroup}`, `--die-with-parent`) but network stays so Claude can reach the API. Mounted in:

- **Read-only:** `/usr`, `/bin`, `/lib`, `/sbin`, `/etc`, `/lib64`, `/run`, `/mnt/wsl`, `/home/linuxbrew` (when present), the repo root, plus from `$HOME`: `.gitconfig`, `.config/{git,gh,glab-cli}`, `.ssh`, `.local/bin`, `.local/share/claude`, the resolved `node` prefix (when under `$HOME`), `$PNPM_HOME`.
- **Read-write:** the worktree itself, every relevant `.git` dir, anything in `sandbox.shareReadWrite`, plus `$HOME/.claude` and `$HOME/.claude.json` so credentials and session state persist.
- **Env:** cleared, then a curated set (`HOME`, `USER`, `TERM`, `SHELL`, `LANG`, `PATH`, plus `ANTHROPIC_API_KEY`, `SSH_AUTH_SOCK`, `COLORTERM` when present).

Override `bwrap` location with `GROVE_BWRAP_PATH`. Skip sandboxing for one call with `--no-sandbox` or globally via `sandbox.enabled: false`.

## License

MIT
