"""Release counter: components 0..98; the 99th increment carries."""
import re
import subprocess
from pathlib import Path


def parse(value):
    match = re.fullmatch(r"v?(\d+)\.(\d+)\.(\d+)", value)
    if not match:
        raise ValueError(f"Not a release version: {value!r}")
    return tuple(map(int, match.groups()))


def next_version(versions):
    major, minor, patch = max(map(parse, versions))
    minor, patch = minor + (patch + 1) // 99, (patch + 1) % 99
    major, minor = major + minor // 99, minor % 99
    return f"{major}.{minor}.{patch}"


def main():
    manifest = Path('Cargo.toml')
    text = manifest.read_text()
    current = re.search(r'^version = "([^"]+)"', text, re.M).group(1)
    tags = subprocess.check_output(['git', 'tag', '--list', 'v*'], text=True).splitlines()
    versions = [current] + [tag for tag in tags if re.fullmatch(r'v\d+\.\d+\.\d+', tag)]
    version = next_version(versions)
    manifest.write_text(text.replace(f'version = "{current}"', f'version = "{version}"', 1))
    lock = Path('Cargo.lock')
    lock.write_text(lock.read_text().replace(f'name = "uncl"\nversion = "{current}"', f'name = "uncl"\nversion = "{version}"', 1))
    readme = Path('readme.md')
    readme.write_text(re.sub(r'^v\d+\.\d+\.\d+$', f'v{version}', readme.read_text(), flags=re.M))
    print(version)


if __name__ == '__main__':
    main()
