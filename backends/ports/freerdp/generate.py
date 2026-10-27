#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The two files FreeRDP's CMake writes by running code rather than by
# substituting variables, written the same way, byte for byte:
#
#   generate.py settings-keys <settings_types_private.h> <settings_keys.h.in> <out>
#       the FreeRDP_Settings_Keys_* enums, read out of the settings struct
#       (upstream: include/CMakeLists.txt, a regex loop over file(STRINGS)).
#   generate.py tables <tables.c.in> <out> <channel>...
#       channels/client/tables.c, the static registry of every built-in
#       channel (upstream: channels/client/CMakeLists.txt over what
#       channels/CMakeLists.txt's add_channel_client macros registered).
#       <channel> is name:Entry[+Entry]:[subsystem,subsystem], subsystems in
#       the order the channel's client/CMakeLists adds them.
import re
import sys

TYPES = ['BOOL', 'INT16', 'UINT16', 'INT32', 'UINT32', 'INT64', 'UINT64', 'STRING', 'POINTER']


def settings_keys(header, template, out):
    keys = {t: [] for t in TYPES}
    # file(STRINGS ... REGEX "ALIGN64[ \ta-zA-Z0-9]*"), then each line as the
    # foreach in include/CMakeLists.txt reads it.
    for line in open(header, encoding='utf-8').read().split('\n'):
        if not re.search(r'ALIGN64[ \ta-zA-Z0-9]*', line):
            continue
        key = line.strip()
        index = re.sub(r'[ \t/*]', '', re.sub(r'.+/\*', '', key, count=1))
        if not re.fullmatch(r'[0-9]+', index):
            continue
        value = re.sub(r'^SETTINGS_DEPRECATED\(ALIGN64[ \t]+[a-zA-Z0-9_*]+[ \t]+', '', key, count=1).strip()
        value = value[:value.find(')')]
        kind = 'POINTER'
        for t in TYPES[:7]:
            if re.match(r'^SETTINGS_DEPRECATED\(ALIGN64[ \t]+' + t + r'[ \t]+', key):
                kind = t
                break
        else:
            if re.match(r'^SETTINGS_DEPRECATED\(ALIGN64[ \t]+(char|CHAR)[ \t]*\*[ \t]+', key):
                kind = 'STRING'
        keys[kind].append(f'FreeRDP_{value} = {index}')
    text = open(template, encoding='utf-8').read()
    for t in TYPES:
        text = text.replace(f'@SETTINGS_KEYS_{t}@', ',\n\t'.join(keys[t] + [f'FreeRDP_{t}_UNUSED = -1']))
    open(out, 'w', encoding='utf-8').write(text)


# entry point -> (import signature, union member, entry table type)
ENTRY = {
    'VirtualChannelEntry': ('BOOL', '(PCHANNEL_ENTRY_POINTS)', '.csevc', 'STATIC_ENTRY_VC'),
    'VirtualChannelEntryEx': ('BOOL', '(PCHANNEL_ENTRY_POINTS_EX,PVOID)', '.csevcex', 'STATIC_ENTRY_VCEX'),
    'DVCPluginEntry': ('UINT', '(IDRDYNVC_ENTRY_POINTS* pEntryPoints)', '.csedvc', 'STATIC_ENTRY_DVC'),
    'DeviceServiceEntry': ('UINT', '(PDEVICE_SERVICE_ENTRY_POINTS pEntryPoints)', '.csedse', 'STATIC_ENTRY_DSE'),
}


def tables(template, out, specs):
    channels = {}
    for spec in specs:
        name, entries, subs = spec.split(':')
        channels[name] = (entries.split('+'), [s for s in subs.split(',') if s])
    # channels/CMakeLists.txt globs the channel directories (sorted), moves
    # drdynvc to the end and reverses the list, so that drdynvc registers first.
    order = sorted(n for n in channels if n != 'drdynvc')
    if 'drdynvc' in channels:
        order.append('drdynvc')
    order.reverse()
    static_entries = []
    for n in order:
        for e in channels[n][0]:
            if e not in static_entries:
                static_entries.append(e)

    imports = {e: '' for e in static_entries}
    table = {e: '' for e in static_entries}
    for e in static_entries:
        ret, args, _, _ = ENTRY[e]
        for n in order:
            if e in channels[n][0]:
                imports[e] += f'\nextern {ret} VCAPITYPE {n}_{e}{args};'
                table[e] += f'\n\t{{ "{n}", {n}_{e} }},'

    entry_imports = ''
    entry_tables = ''
    tables_list = ('\nextern const STATIC_ENTRY_TABLE CLIENT_STATIC_ENTRY_TABLES[];\n'
                   'const STATIC_ENTRY_TABLE CLIENT_STATIC_ENTRY_TABLES[] =\n{')
    for e in static_entries:
        _, _, init, ctype = ENTRY[e]
        entry_imports += '\n' + imports[e]
        entry_tables += f'\nextern const {ctype} CLIENT_{e}_TABLE[];\n'
        entry_tables += f'const {ctype} CLIENT_{e}_TABLE[] =\n{{'
        entry_tables += '\n' + table[e]
        entry_tables += '\n\t{ nullptr, nullptr }\n};'
        tables_list += f'\n\t{{ "{e}", {{ {init} = CLIENT_{e}_TABLE }} }},'
    tables_list += '\n\t{ nullptr, { .cse = nullptr } }\n};'

    sub_imports = ''
    sub_tables = ''
    addin = ('extern const STATIC_ADDIN_TABLE CLIENT_STATIC_ADDIN_TABLE[];\n'
             'const STATIC_ADDIN_TABLE CLIENT_STATIC_ADDIN_TABLE[] =\n{')
    for n in order:
        entries, subs = channels[n]
        tname = f'CLIENT_{n}_SUBSYSTEM_TABLE'.upper()
        st = f'extern const STATIC_SUBSYSTEM_ENTRY {tname}[];\nconst STATIC_SUBSYSTEM_ENTRY {tname}[] =\n{{'
        for s in subs:
            sname, _, stype = s.partition('-')
            fn = (f'{sname}_freerdp_{n}_client_{stype}_subsystem_entry' if stype
                  else f'{sname}_freerdp_{n}_client_subsystem_entry')
            st += f'\n\t{{ "{sname}", "{stype}", {fn} }},'
            sub_imports += f'\nextern UINT VCAPITYPE {fn}(void*);'
        st += '\n\t{ nullptr, nullptr, nullptr }\n};'
        sub_tables += '\n' + st
        for e in entries:
            addin += f'\n\t{{ "{n}", "{e}", {{ {ENTRY[e][2]} = {n}_{e} }}, {tname} }},'
    addin += '\n\t{ nullptr, nullptr, { .cse = nullptr }, nullptr }\n};'

    text = open(template, encoding='utf-8').read()
    for var, value in [('CLIENT_STATIC_TYPEDEFS', ''), ('CLIENT_STATIC_ENTRY_IMPORTS', entry_imports),
                       ('CLIENT_STATIC_SUBSYSTEM_IMPORTS', sub_imports),
                       ('CLIENT_STATIC_ENTRY_TABLES', entry_tables),
                       ('CLIENT_STATIC_ENTRY_TABLES_LIST', tables_list),
                       ('CLIENT_STATIC_SUBSYSTEM_TABLES', sub_tables),
                       ('CLIENT_STATIC_ADDIN_TABLE', addin)]:
        text = text.replace('${' + var + '}', value)
    open(out, 'w', encoding='utf-8').write(text)


if __name__ == '__main__':
    if sys.argv[1] == 'settings-keys':
        settings_keys(*sys.argv[2:5])
    elif sys.argv[1] == 'tables':
        tables(sys.argv[2], sys.argv[3], sys.argv[4:])
    else:
        sys.exit(f'unknown mode {sys.argv[1]}')
