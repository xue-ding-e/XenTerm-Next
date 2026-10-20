"""Read only the lockfile-pinned GPUI X11 source for window mapping evidence."""
import os
from pathlib import Path
root = Path(os.environ['CARGO_HOME']) / 'registry' / 'src'
packages = list(root.glob('*/gpui-pre-linux-0.3.8'))
if not packages:
    raise SystemExit('Pinned gpui-pre-linux 0.3.8 source not present')
for package in packages:
    for source in sorted(package.glob('src/**/*.rs')):
        if 'x11' not in str(source):
            continue
        lines = source.read_text().splitlines()
        selected = set()
        for i, line in enumerate(lines):
            if any(term in line for term in ['_NET_ACTIVE_WINDOW', 'map_window(', 'MapRequest', 'WM_STATE', 'set_visible']):
                selected.update(range(max(0, i - 6), min(len(lines), i + 10)))
        if selected:
            print(f'\nSOURCE gpui-pre-linux-0.3.8/{source.relative_to(package)}')
            for i in sorted(selected):
                print(f'{i + 1}: {lines[i]}')
