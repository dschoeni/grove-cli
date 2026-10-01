use crate::term::{stderr, stdout};

const BASH: &str = r#"_grove() {
  local cur cmd
  cur="${COMP_WORDS[COMP_CWORD]}"
  cmd="${COMP_WORDS[1]:-}"

  if [[ $COMP_CWORD -eq 1 ]]; then
    COMPREPLY=( $(compgen -W "init new resume sync ls rm pull completion" -- "$cur") )
    return
  fi

  case "$cmd" in
    resume|sync|rm)
      if [[ $COMP_CWORD -eq 2 ]]; then
        local slugs
        slugs="$(grove __complete-slugs 2>/dev/null)"
        COMPREPLY=( $(compgen -W "$slugs" -- "$cur") )
      fi
      ;;
    completion)
      if [[ $COMP_CWORD -eq 2 ]]; then
        COMPREPLY=( $(compgen -W "bash zsh" -- "$cur") )
      fi
      ;;
  esac
}
complete -F _grove grove
"#;

const ZSH: &str = r#"#compdef grove

_grove() {
  local -a cmds
  cmds=(
    'init:Write a starter .groverc at the repo root'
    'new:Create a worktree and launch a sandboxed Claude session'
    'resume:Re-enter an existing worktree'
    'sync:Update worktree branches from their remote'
    'ls:List Grove-managed worktrees'
    'rm:Remove a worktree and its branch'
    'pull:Fast-forward the base branch to its latest remote state'
    'completion:Output a shell completion script'
  )

  if (( CURRENT == 2 )); then
    _describe 'command' cmds
    return
  fi

  case "$words[2]" in
    resume|sync|rm)
      if (( CURRENT == 3 )); then
        local -a slugs
        slugs=("${(@f)$(grove __complete-slugs 2>/dev/null)}")
        _describe 'slug' slugs
      fi
      ;;
    completion)
      if (( CURRENT == 3 )); then
        _values 'shell' bash zsh
      fi
      ;;
  esac
}

compdef _grove grove
"#;

const USAGE: &str = r#"grove completion — output a shell completion script

Usage:
  grove completion <bash|zsh>

Examples:
  # bash, per-user
  echo 'source <(grove completion bash)' >> ~/.bashrc

  # zsh, per-user (with a directory on your $fpath)
  grove completion zsh > "${fpath[1]}/_grove"

  # bash, system-wide
  grove completion bash | sudo tee /etc/bash_completion.d/grove > /dev/null
"#;

pub fn run(argv: &[String]) -> i32 {
    match argv.first().map(String::as_str) {
        None => {
            stdout!("{USAGE}");
            64
        }
        Some("-h" | "--help") => {
            stdout!("{USAGE}");
            0
        }
        Some("bash") => {
            stdout!("{BASH}");
            0
        }
        Some("zsh") => {
            stdout!("{ZSH}");
            0
        }
        Some(other) => {
            stderr!("Unknown shell: {other}\n\n{USAGE}");
            64
        }
    }
}
