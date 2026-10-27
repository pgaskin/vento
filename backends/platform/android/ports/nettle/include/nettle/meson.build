# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The public headers, copied here as upstream's install would; a header that
# configure wrote is gen/'s.

nettle_public_headers = []
foreach name : nettle_headers
  from = fs.exists('../../gen' / name) ? '../../gen' / name : '../..' / nettle_dir / name
  nettle_public_headers += fs.copyfile(from, name)
endforeach
