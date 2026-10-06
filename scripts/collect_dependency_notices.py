#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Collect exact license texts from a locked Cargo graph and pinned supplements."""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--metadata', required=True, type=Path)
    parser.add_argument('--root-crate', action='append', help='Repeat for binary-owning workspace crates')
    parser.add_argument('--title', default='FARIS native dependency notices')
    parser.add_argument('--platform-label', default='Linux',
                        help='Platform named in the header, matching the --filter-platform graph')
    parser.add_argument('--target-triple', default='x86_64-unknown-linux-gnu',
                        help='Target triple named in the header recreation note')
    parser.add_argument('--supplements', type=Path,
                        default=Path(__file__).resolve().parents[1] / 'licenses/upstream-rust')
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    metadata = json.loads(args.metadata.read_text())
    packages = {p['id']: p for p in metadata['packages']}
    nodes = {n['id']: n for n in metadata['resolve']['nodes']}
    root_names = set(args.root_crate or ('faris-cli', 'faris-app'))
    roots = [p['id'] for p in metadata['packages'] if p['name'] in root_names]
    if len(roots) != len(root_names):
        raise ValueError('metadata must include each requested client crate exactly once')
    pending, used = roots[:], set()
    while pending:
        package_id = pending.pop()
        if package_id in used:
            continue
        used.add(package_id)
        pending.extend(dependency['pkg'] for dependency in nodes[package_id]['deps'])
    supplements = json.loads((args.supplements / 'manifest.json').read_text())
    texts: dict[str, bytes] = {}
    rows = []
    for package in sorted((packages[i] for i in used if packages[i]['source']),
                          key=lambda p: (p['name'], p['version'])):
        root = Path(package['manifest_path']).parent
        paths = {path for path in root.rglob('*') if path.is_file()
                 and path.name.upper().startswith(('LICENSE', 'COPYING', 'NOTICE'))
                 and path.suffix.lower() not in ('.rs', '.c', '.h')}
        if package['license_file']:
            paths.add(root / package['license_file'])
        if package['name'] == 'epaint_default_fonts':
            paths.update(root / 'fonts' / name for name in
                         ('OFL.txt', 'UFL.txt', 'Hack-Regular.txt',
                          'emoji-icon-font-mit-license.txt'))
        vcs_path = root / '.cargo_vcs_info.json'
        vcs = json.loads(vcs_path.read_text()) if vcs_path.exists() else {}
        commit = vcs.get('git', {}).get('sha1')
        local_files = []
        for path in sorted(paths):
            if not path.is_file() or not path.resolve().is_relative_to(root.resolve()):
                raise ValueError(f'license file missing or outside crate: {path}')
            data = path.read_bytes()
            if len(data) > 256 * 1024:
                raise ValueError(f'license text exceeds bound: {path}')
            data.decode('utf-8')
            sha = digest(data)
            texts[sha] = data
            local_files.append((path.relative_to(root).as_posix(), sha))
        # Some crates publish no workspace-root license file. Pin those upstream
        # supplements to the source commit recorded inside the actual crate.
        if not local_files or package['name'].startswith(('egui', 'epaint', 'eframe', 'ecolor', 'emath', 'krilla')):
            matches = [record for record in supplements if record['commit'] == commit]
            for record in matches:
                path = args.supplements / record['file']
                data = path.read_bytes()
                if digest(data) != record['sha256']:
                    raise ValueError(f'upstream license changed: {path}')
                data.decode('utf-8')
                texts[record['sha256']] = data
                local_files.append((record['url'], record['sha256']))
        if not local_files:
            raise ValueError(f'no license text for {package["name"]} {package["version"]}')
        rows.append((package, commit, local_files))
    body = [
        '# ' + args.title, '',
        f'Exact license texts collected from the locked {args.platform_label} Cargo dependency graph.',
        'This conservative inventory includes build dependencies and resolved optional',
        'packages; it is not a list inferred from binary symbols. Original third-party',
        'copyright and license terms remain those reproduced below. Project licensing',
        'is recorded separately in its root license and source provenance.', '',
        'SHA-256 labels identify original source-file bytes before Markdown framing.', '',
        'Recreate the metadata with `cargo metadata --locked --offline --format-version 1',
        f'--filter-platform {args.target_triple}`, then run',
        '`python3 scripts/collect_dependency_notices.py --metadata METADATA.json',
        '--output OUTPUT.md`. Use `--root-crate CRATE` for each client in another',
        'workspace. Crate sources must be available in the Cargo cache.', '',
        f'{len(rows)} dependency versions; {len(texts)} distinct exact license texts.', '',
        '| Package | Declared license | Source | License texts |',
        '| --- | --- | --- | --- |',
    ]
    for package, commit, files in rows:
        crate = f'{package["name"]} {package["version"]}'
        source = f'https://crates.io/crates/{package["name"]}/{package["version"]}'
        refs = '<br>'.join(f'{name}: [SHA-256 {sha[:12]}](#text-{sha})' for name, sha in files)
        body.append(f'| {crate} | {package["license"] or "See declared license file"} | [{crate}]({source}) | {refs} |')
    for sha, data in sorted(texts.items()):
        body.extend(('', f'<a id="text-{sha}"></a>', '', f'## License text `{sha}`', '',
                     '````text', data.decode('utf-8').rstrip('\n'), '````'))
    result = ('\n'.join(body) + '\n').encode('utf-8')
    if len(result) > 4 * 1024 * 1024:
        raise ValueError('consolidated notice exceeds four MiB')
    args.output.write_bytes(result)
    print(json.dumps({'packages': len(rows), 'distinct_license_texts': len(texts),
                      'bytes': len(result), 'sha256': digest(result)}))


if __name__ == '__main__':
    main()
