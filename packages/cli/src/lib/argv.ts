/**
 * Split argv at the first standalone `--`. Everything before is parsed by the
 * subcommand; everything after is passed through to the child process unchanged.
 *
 * This sidesteps parseArgs' behavior of folding post-`--` tokens into the
 * positionals array, which makes it hard to distinguish the subcommand's own
 * positional args from the passthrough.
 */
export function splitPassthrough(argv: string[]): { args: string[]; passthrough: string[] } {
  const idx = argv.indexOf('--');
  if (idx < 0) return { args: argv, passthrough: [] };
  return { args: argv.slice(0, idx), passthrough: argv.slice(idx + 1) };
}
