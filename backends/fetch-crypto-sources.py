#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Makes third_party/<name> the release crypto-sources.txt pins for it, as
# Gradle's fetchCryptoSources does: downloaded from the first mirror that
# answers, checked against the pinned SHA-256 before anything is unpacked,
# unpacked without the tarball's top directory, with a .pin file holding the
# hash. A tree whose .pin holds the pinned hash is kept; any other is
# replaced. The tree is unpacked under third_party/.fetch/ and renamed into
# place, so an interrupted run leaves the previous tree or none. The Linux
# Meson build runs this when it is configured.
#
#   python3 backends/fetch-crypto-sources.py [NAME...]    (default: every name in the file)

import fcntl
import hashlib
import os
import shutil
import sys
import tarfile
import urllib.request

BACKENDS = os.path.dirname(os.path.abspath(__file__))
THIRD_PARTY = os.path.join(os.path.dirname(BACKENDS), 'third_party')


def read_pins():
    pins = {}
    with open(os.path.join(BACKENDS, 'crypto-sources.txt'), encoding='utf-8') as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith('#'):
                continue
            key, sep, value = line.partition('=')
            name, dot, field = key.strip().partition('.')
            if not sep or not dot or field not in ('version', 'sha256', 'url') or not value.strip():
                sys.exit(f'crypto-sources.txt: not a `<name>.<key> = <value>` line: {line}')
            pin = pins.setdefault(name, {'urls': []})
            if field == 'url':
                pin['urls'].append(value.strip())
            else:
                pin[field] = value.strip()
    return pins


def current_pin(tree):
    try:
        with open(os.path.join(tree, '.pin'), encoding='utf-8') as f:
            return f.read().strip()
    except FileNotFoundError:
        return ''


def fetch(name, pin):
    tree = os.path.join(THIRD_PARTY, name)
    if current_pin(tree) == pin['sha256']:
        return
    work_root = os.path.join(THIRD_PARTY, '.fetch')
    os.makedirs(work_root, exist_ok=True)
    # Several build trees may be configured at once.
    with open(os.path.join(work_root, name + '.lock'), 'w') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        if current_pin(tree) == pin['sha256']:
            return
        print(f'{name}: fetching {name} {pin["version"]} into third_party/{name}', flush=True)
        work = os.path.join(work_root, name)
        shutil.rmtree(work, ignore_errors=True)
        os.makedirs(work)
        failures = []
        for url in pin['urls']:
            tarball = os.path.join(work, url.rsplit('/', 1)[-1])
            try:
                with urllib.request.urlopen(url, timeout=60) as response, open(tarball, 'wb') as out:
                    shutil.copyfileobj(response, out)
            except OSError as e:
                failures.append(f'{url}: {e}')
                continue
            digest = hashlib.sha256()
            with open(tarball, 'rb') as f:
                for chunk in iter(lambda: f.read(1 << 20), b''):
                    digest.update(chunk)
            got = digest.hexdigest()
            if got != pin['sha256']:
                failures.append(f'{url}: sha256 {got}, wanted {pin["sha256"]}')
                continue
            unpacked = os.path.join(work, 'unpacked')
            with tarfile.open(tarball) as tar:
                if hasattr(tarfile, 'data_filter'):
                    tar.extractall(unpacked, filter='data')
                else:
                    tar.extractall(unpacked)
            top = os.listdir(unpacked)
            if len(top) != 1 or not os.path.isdir(os.path.join(unpacked, top[0])):
                sys.exit(f'{url} does not hold a single top directory; check crypto-sources.txt')
            top = os.path.join(unpacked, top[0])
            with open(os.path.join(top, '.pin'), 'w', encoding='utf-8') as f:
                f.write(pin['sha256'] + '\n')
            if os.path.exists(tree):
                os.rename(tree, os.path.join(work, 'previous'))
            os.rename(top, tree)
            shutil.rmtree(work)
            return
        shutil.rmtree(work, ignore_errors=True)
        sys.exit(f'third_party/{name}: no mirror gave {name} {pin["version"]}:\n  '
                 + '\n  '.join(failures)
                 + f'\nUnpack it by hand instead: download it, check that its SHA-256 is '
                 f'{pin["sha256"]}, then `mkdir third_party/{name} && tar -xf TARBALL '
                 f'--strip-components=1 -C third_party/{name} && '
                 f'echo {pin["sha256"]} > third_party/{name}/.pin`.')


def main():
    pins = read_pins()
    for name in sys.argv[1:] or pins:
        if name not in pins:
            sys.exit(f'crypto-sources.txt has no {name}')
        fetch(name, pins[name])


if __name__ == '__main__':
    main()
