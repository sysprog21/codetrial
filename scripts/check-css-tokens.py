from pathlib import Path
import re
import sys

COLOR_LITERAL_RE = re.compile(
    r'(?<!var\()(?<![\w-])(#(?:[0-9a-fA-F]{3,4}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})\b|rgba?\([^)]+\)|hsla?\([^)]+\))'
)

TOKEN_SCOPE_START_RE = re.compile(
    r'(:root|\[data-theme=|@media\s*\(prefers-color-scheme)'
)


def check_file(filepath: Path) -> list[str]:
  errors = []
  in_token_scope = 0

  with open(filepath, 'r', encoding='utf-8') as f:
    for line_num, line in enumerate(f, start=1):
      stripped = line.strip()

 
      if (
          not stripped
          or stripped.startswith('/*')
          or stripped.startswith('*')
          or stripped.endswith('*/')
      ):
        continue

      if TOKEN_SCOPE_START_RE.search(stripped):
        in_token_scope += stripped.count('{') - stripped.count('}')
        continue

      if in_token_scope > 0:
        in_token_scope += stripped.count('{') - stripped.count('}')
        continue

      if ':' in stripped and not stripped.startswith('--'):
        match = COLOR_LITERAL_RE.search(stripped)
        if match:
          val = match.group(0)
          errors.append(f'{filepath}:{line_num}: Hardcoded color "{val}".')

  return errors


def main() -> int:
  repo_root = Path(__file__).resolve().parent.parent
  css_files = sorted(repo_root.glob('web/**/*.css'))

  print(f'Scanning {len(css_files)} CSS files in {repo_root / "web"}...')

  all_errors = []
  for css_file in css_files:
    all_errors.extend(check_file(css_file))

  if all_errors:
    print(
        f'\nCSS Token Enforcement Failed ({len(all_errors)} errors found):',
        file=sys.stderr,
    )
    for err in all_errors:
      print(f'  {err}', file=sys.stderr)
    return 1

  print('All CSS files pass token enforcement.')
  return 0


if __name__ == '__main__':
  sys.exit(main())