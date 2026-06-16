const BASH = `\
_grove() {
  local cur cmd
  cur="\${COMP_WORDS[COMP_CWORD]}"
  cmd="\${COMP_WORDS[1]:-}"

  if [[ \$COMP_CWORD -eq 1 ]]; then
    COMPREPLY=( \$(compgen -W "init new resume ls rm pull completion" -- "\$cur") )
    return
  fi

  case "\$cmd" in
    resume|rm)
      if [[ \$COMP_CWORD -eq 2 ]]; then
        local slugs
        slugs="\$(grove __complete-slugs 2>/dev/null)"
        COMPREPLY=( \$(compgen -W "\$slugs" -- "\$cur") )
      fi
      ;;
    completion)
      if [[ \$COMP_CWORD -eq 2 ]]; then
        COMPREPLY=( \$(compgen -W "bash zsh" -- "\$cur") )
      fi
      ;;
  esac
}
complete -F _grove grove
`;

const ZSH = `\
#compdef grove

_grove() {
  local -a cmds
  cmds=(
    'init:Write a starter .groverc at the repo root'
    'new:Create a worktree and launch a sandboxed Claude session'
    'resume:Re-enter an existing worktree'
    'ls:List Grove-managed worktrees'
    'rm:Remove a worktree and its branch'
    'pull:Fast-forward the base branch to its latest remote state'
    'completion:Output a shell completion script'
  )

  if (( CURRENT == 2 )); then
    _describe 'command' cmds
    return
  fi

  case "\$words[2]" in
    resume|rm)
      if (( CURRENT == 3 )); then
        local -a slugs
        slugs=("\${(@f)\$(grove __complete-slugs 2>/dev/null)}")
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
`;

const HELP = `\
grove completion — output a shell completion script

Usage:
  grove completion <bash|zsh>

Examples:
  # bash, per-user
  echo 'source <(grove completion bash)' >> ~/.bashrc

  # zsh, per-user (with a directory on your $fpath)
  grove completion zsh > "\${fpath[1]}/_grove"

  # bash, system-wide
  grove completion bash | sudo tee /etc/bash_completion.d/grove > /dev/null
`;

export function runCompletion(argv: string[]): number {
  const arg = argv[0];
  if (!arg || arg === '-h' || arg === '--help') {
    process.stdout.write(HELP);
    return arg ? 0 : 64;
  }
  if (arg === 'bash') {
    process.stdout.write(BASH);
    return 0;
  }
  if (arg === 'zsh') {
    process.stdout.write(ZSH);
    return 0;
  }
  process.stderr.write(`Unknown shell: ${arg}\n\n${HELP}`);
  return 64;
}
