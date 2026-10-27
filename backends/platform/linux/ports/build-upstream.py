#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Runs an upstream's own build, configure then make, out of its source tree,
# and installs it into a prefix in the Meson build tree: what the Linux ports
# of OpenSSL, nettle and GnuTLS run as a custom target. Each run starts from
# an empty build directory and prefix; ninja reruns it when its command line
# changes. The output goes to a log, printed only when a step fails.
#
#   build-upstream.py --build DIR --prefix DIR [--env NAME=VALUE]... [--unset NAME]...
#                     [--make TARGET]... [--copy FILE=DEST]... --stamp FILE -- CONFIGURE...
#
# --copy copies a file from the prefix to where Meson expects the target's
# output; --stamp writes an empty header, the output that orders the
# consumers' compiles after the install.

import argparse
import os
import shutil
import subprocess
import sys


def run(argv, cwd, env, log):
    with open(log, 'w') as out:
        status = subprocess.run(argv, cwd=cwd, env=env, stdout=out, stderr=subprocess.STDOUT).returncode
    if status != 0:
        with open(log, errors='replace') as f:
            sys.stdout.write(''.join(f.readlines()[-100:]))
        sys.exit(f'{" ".join(argv)} failed ({status}); the whole log is {log}')


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--build', required=True)
    ap.add_argument('--prefix', required=True)
    ap.add_argument('--env', action='append', default=[])
    ap.add_argument('--unset', action='append', default=[])
    ap.add_argument('--make', action='append', default=[])
    ap.add_argument('--copy', action='append', default=[])
    ap.add_argument('--stamp', required=True)
    ap.add_argument('configure', nargs='+')
    args = ap.parse_args()

    env = dict(os.environ)
    for name in args.unset:
        env.pop(name, None)
    for assignment in args.env:
        name, _, value = assignment.partition('=')
        env[name] = value

    for path in (args.build, args.prefix):
        shutil.rmtree(path, ignore_errors=True)
    os.makedirs(args.build)

    run(args.configure, args.build, env, os.path.join(args.build, 'configure.log'))
    make = shutil.which('gmake') or 'make'
    for target in args.make:
        run([make, f'-j{os.cpu_count()}', target], args.build, env,
            os.path.join(args.build, f'make-{target}.log'))

    for copy in args.copy:
        source, _, dest = copy.partition('=')
        shutil.copyfile(os.path.join(args.prefix, source), dest)
    with open(args.stamp, 'w') as f:
        f.write('/* Written once the upstream build installed its headers. */\n')


if __name__ == '__main__':
    main()
