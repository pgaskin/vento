// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The command line's words: the options [`split`] takes apart, GNU-style,
//! the help text, and the two listings of the backends (the help's
//! `backends:` section and `--list`), all formatted from plain data so that
//! they can be tested without opening a library; and `--probe`'s spec,
//! taken apart into its [`Step`]s.

use std::path::PathBuf;
use std::time::Duration;

/// An option the command line takes: its long name without the dashes, its
/// one-letter name if it has one, and whether it takes a value.
pub struct Flag {
    pub long: &'static str,
    pub short: Option<char>,
    pub value: bool,
}

const fn flag(long: &'static str, short: Option<char>, value: bool) -> Flag {
    Flag { long, short, value }
}

/// The options the help leaves out: a developer's, which behave as any
/// other option otherwise.
#[cfg(test)]
const HIDDEN: &[&str] = &["dev-force-socketpair"];

/// Every option, in the help's order, then the ones it leaves out
/// (`HIDDEN`).
pub const FLAGS: &[Flag] = &[
    flag("help", Some('h'), false),
    flag("version", Some('V'), false),
    flag("list", Some('L'), false),
    flag("forget", Some('F'), true),
    flag("probe", Some('P'), true),
    flag("connect", Some('c'), false),
    flag("user", Some('u'), true),
    flag("domain", None, true),
    flag("password", Some('p'), true),
    flag("option", Some('o'), true),
    flag("trust", None, false),
    flag("platform", None, true),
    flag("video", None, true),
    flag("audio", None, true),
    flag("cursor", None, true),
    flag("extlib-dir", None, true),
    flag("state-dir", None, true),
    flag("verbose", None, false),
    flag("scale", None, true),
    flag("view-only", None, false),
    flag("hw-decode", None, false),
    flag("no-send-clipboard", None, false),
    flag("no-recv-clipboard", None, false),
    flag("no-grab-keyboard", None, false),
    // Every connection a backend makes through vento is relayed through a
    // socketpair (`runtime::net::Socketpair`), to test the backends on a
    // socket that is not TCP.
    flag("dev-force-socketpair", None, false),
];

/// One word of the command line, taken apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Arg {
    /// An option by its long name, with its value if it takes one.
    Flag(&'static str, Option<String>),
    Positional(String),
}

/// Takes `args` apart as GNU's getopt_long does: `--name value`,
/// `--name=value`, `-n value` and `-nvalue` for an option that takes a
/// value, and short options stacked (`-cu NAME`, `-cuNAME`) up to the first
/// one that takes a value, which takes the rest of the word or the next one.
/// A lone `-` is a positional argument, and everything after `--` is one too.
/// Positional arguments may come between the options.
pub fn split(args: Vec<String>, flags: &'static [Flag]) -> Result<Vec<Arg>, String> {
    let mut out = Vec::new();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        if arg == "--" {
            out.extend(it.by_ref().map(Arg::Positional));
            break;
        }
        if let Some(long) = arg.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value.to_string())),
                None => (long, None),
            };
            let f = flags
                .iter()
                .find(|f| f.long == name)
                .ok_or_else(|| format!("unknown option --{name}"))?;
            let value = match (f.value, inline) {
                (true, Some(value)) => Some(value),
                (true, None) => Some(it.next().ok_or_else(|| format!("--{name} needs a value"))?),
                (false, Some(_)) => return Err(format!("--{name} takes no value")),
                (false, None) => None,
            };
            out.push(Arg::Flag(f.long, value));
        } else if let Some(shorts) = arg.strip_prefix('-').filter(|s| !s.is_empty()) {
            for (at, c) in shorts.char_indices() {
                let f = flags
                    .iter()
                    .find(|f| f.short == Some(c))
                    .ok_or_else(|| format!("unknown option -{c}"))?;
                if !f.value {
                    out.push(Arg::Flag(f.long, None));
                    continue;
                }
                let rest = &shorts[at + c.len_utf8()..];
                let value = if rest.is_empty() {
                    it.next().ok_or_else(|| format!("-{c} needs a value"))?
                } else {
                    rest.to_string()
                };
                out.push(Arg::Flag(f.long, Some(value)));
                break;
            }
        } else {
            out.push(Arg::Positional(arg));
        }
    }
    Ok(out)
}

/// The defaults the help shows, which depend on the machine.
pub struct Defaults {
    pub extlib_dir: String,
    pub state_dir: String,
}

/// The default `--scale`, `wait-connect=` and thumbnail scale (1/N).
pub const SCALE: &str = "auto";
pub const CONNECT_TIMEOUT: u64 = 30;
pub const THUMB_STEP: usize = 8;

/// The version `--version` and the help show: `v` and the package's
/// version, without the `0.0.` of a release's `0.0.N`.
pub fn version() -> String {
    display_version(env!("CARGO_PKG_VERSION"))
}

fn display_version(package: &str) -> String {
    format!("v{}", package.strip_prefix("0.0.").unwrap_or(package))
}

const USAGE: &str = r#"vento <version>

usage:
  vento                                       connection dialog
  vento [<backend> [<address>]] [options]     connection dialog (prefilled)
  vento <backend> <address> --connect ...     single session, auto-connect
  vento <backend> <address> --probe SPEC ...  headless probe
  vento --list                                list backends
  vento --forget <address> [--state-dir DIR]  forget host

arguments:
  <backend>                  backend name from --list
  <address>                  host, host:port, host:display, [literal]:port, etc

commands:
  -h, --help                 show this help
  -V, --version              show the version
  -L, --list                 list backends
  -F, --forget ADDRESS       forget the server with the specified address
  -P, --probe SPEC           run headless and run the steps in SPEC once connected (implies --connect)

options:
  -c, --connect              automatically connect, and quit on disconnect
  -u, --user NAME            username (for RDP, DOMAIN\user overrides --domain; .\user for a local account, MicrosoftAccount\email@address.com for a Microsoft Account)
      --domain NAME          domain (RDP only)
  -p, --password TEXT        password
  -o, --option KEY=VALUE     backend option (see backends below)
      --trust                automatically accept all trust/security/message prompts (ignoring and not writing to known_hosts)

      --platform NAME        window server protocol (auto, x11, wayland) (default: auto)
      --video NAME           video hardware decoding backend (auto, vaapi, off) (default: auto)
      --audio NAME           audio backend (auto, pipewire, alsa, off) (default: auto)
      --cursor MODE          how to draw the remote cursor (native, software) (default: native)
      --extlib-dir DIR       path to extra libraries required by some backends (default: <extlib-dir>)
      --state-dir DIR        path to store known hosts and other state (default: <state-dir>)
      --verbose              show debug logs

session options (also see backends below):
      --scale MODE           initial window scale (auto, fit, fill, fit-height, fit-width, fill-width, fill-height, none, none-resize) (default: <scale>)
      --view-only            force view-only mode
      --hw-decode            enable hardware decoding for video where supported
      --no-send-clipboard    do not send locally copied text to the server
      --no-recv-clipboard    do not update local clipboard from the server
      --no-grab-keyboard     do not automatically grab the keyboard

probe steps (SPEC is a comma-separated list of these, run in order):
  wait-connect=SECONDS       wait up to SECONDS for the connection, only as the first step (default: <wait-connect>)
  wait=SECONDS               wait, printing events (e.g., 2, 0.5)
  click=X:Y                  left-click at that position
  key=KEYSYM                 press and release X keysym (e.g., 0x61, 97, 0141 are all 'a')
  clipboard=TEXT             send this as the clipboard
  resize=WxH                 ask for a desktop this size
  layout=WxH;WxH;...         ask for these screens side by side
  sound-cycle=SECONDS        turn the far end's sound off and on every SECONDS from now on (0 stops)
  out=FILE                   save framebuffer ppm
  thumb=FILE[:N]             save framebuffer thumbnail ppm, scaled 1/N (default: <thumb-step>)

  values may be quoted with ' or " to hold commas; inside quotes, \\ \' \" \n \t are escapes
  e.g., --probe 'wait=1,click=500:300,key=0x61,clipboard="a, \"b\"",wait=2,out=shot.ppm'
"#;

/// The help without its `backends:` section, the defaults filled in.
pub fn usage(defaults: &Defaults) -> String {
    USAGE
        .replace("<version>", &version())
        .replace("<extlib-dir>", &defaults.extlib_dir)
        .replace("<state-dir>", &defaults.state_dir)
        .replace("<scale>", SCALE)
        .replace("<wait-connect>", &CONNECT_TIMEOUT.to_string())
        .replace("<thumb-step>", &THUMB_STEP.to_string())
}

/// A backend as the help lists it.
pub struct Backend {
    /// The name the command line takes.
    pub id: String,
    /// What the dialog calls it.
    pub name: String,
    /// The library's version, or "not available: <why>".
    pub version: String,
    pub description: String,
    pub options: Vec<BackendOption>,
}

/// One of a backend's options as the help lists it.
pub struct BackendOption {
    pub key: String,
    pub default: String,
    /// The values it takes, where it is a choice or a switch; empty for a
    /// number or text, whose description says what it takes.
    pub values: Vec<String>,
    pub description: String,
}

/// Where the descriptions of the backends' options start, unless every row
/// ends before it: a row longer than this has its description on the next
/// line, at this column.
const OPTION_COLUMN: usize = 50;

/// The help's `backends:` section: per backend a line with its name,
/// what the dialog calls it and its version, a line with its description,
/// and its options as `-o` takes them, the descriptions in one column.
pub fn backends(list: &[Backend]) -> String {
    let left = |o: &BackendOption| {
        let mut s = format!("    -o {}={}", o.key, o.default);
        if !o.values.is_empty() {
            s.push_str(&format!(" [{}]", o.values.join("|")));
        }
        s
    };
    let column = list
        .iter()
        .flat_map(|b| &b.options)
        .map(|o| left(o).len() + 2)
        .filter(|&w| w <= OPTION_COLUMN)
        .max()
        .unwrap_or(OPTION_COLUMN);
    let mut out = String::from("backends:\n");
    for (i, b) in list.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&format!("  {}  {}  {}\n", b.id, b.name, b.version));
        out.push_str(&format!("  {}\n", b.description));
        for o in &b.options {
            let left = left(o);
            if left.len() + 2 <= column {
                out.push_str(&format!("{left:column$}{}\n", o.description));
            } else {
                out.push_str(&format!("{left}\n{:column$}{}\n", "", o.description));
            }
        }
    }
    out
}

/// `--list`'s line for a backend: its name and the library it runs from,
/// or the library it looked for and why that cannot be opened.
pub fn list_line(id: &str, library: &Result<std::path::PathBuf, (std::path::PathBuf, String)>) -> String {
    match library {
        Ok(path) => format!("{id} {}", path.display()),
        Err((path, why)) => format!("{id} {} (error: {why})", path.display()),
    }
}

/// One step of `--probe`'s spec.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// `wait-connect=SECONDS`: how long to wait for the connection. Only
    /// the first step can be this one.
    WaitConnect(Duration),
    /// `wait=SECONDS`: events printed, prompts answered, for this long.
    Wait(Duration),
    /// `click=X:Y`: a left click there.
    Click(i32, i32),
    /// `key=KEYSYM`: the key pressed and released.
    Key(u32),
    /// `clipboard=TEXT`: the text sent as the clipboard.
    Clipboard(String),
    /// `resize=WxH`: a desktop this size asked for.
    Resize(u32, u32),
    /// `layout=WxH;WxH;...`: these screens asked for, left to right.
    Layout(Vec<(u32, u32)>),
    /// `sound-cycle=SECONDS`: the far end's sound turned off and on this
    /// often from here on; zero stops it.
    SoundCycle(Duration),
    /// `out=FILE`: the framebuffer written there.
    Out(PathBuf),
    /// `thumb=FILE[:N]`: the framebuffer's thumbnail at 1/N written there.
    Thumb(PathBuf, usize),
}

/// The names of the steps [`steps`] takes, in the help's order.
pub const STEPS: &[&str] =
    &["wait-connect", "wait", "click", "key", "clipboard", "resize", "layout", "sound-cycle", "out", "thumb"];

/// `--probe`'s spec: steps separated by commas, each `name=value`. Space
/// around a name is ignored. A value runs to the next comma outside quotes;
/// a part of it in single or double quotes is taken as written, commas and
/// the other quote included, except for the escapes `\\`, `\'`, `\"`, `\n`
/// and `\t`. Outside quotes, a backslash is itself. An empty spec has no
/// steps.
pub fn steps(spec: &str) -> Result<Vec<Step>, String> {
    if spec.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let mut chars = spec.chars();
    loop {
        let (name, value, more) = next_step(&mut chars)?;
        let name = name.trim();
        if name.is_empty() && value.is_none() {
            return Err(format!("step {} is empty", out.len() + 1));
        }
        let Some(value) = value else {
            if seconds(name).is_ok() {
                return Err(format!("{name:?} is not a step (wait={name} waits {name} seconds)"));
            }
            return Err(if STEPS.contains(&name) {
                format!("{name} needs a value ({name}=...)")
            } else {
                format!("unknown step {name:?}")
            });
        };
        let step = step(name, &value)?;
        if matches!(step, Step::WaitConnect(_)) && !out.is_empty() {
            return Err("wait-connect must be the first step".into());
        }
        out.push(step);
        if !more {
            return Ok(out);
        }
    }
}

/// The next step's name, its value (none without an `=`), and whether a
/// comma ended it.
fn next_step(chars: &mut std::str::Chars) -> Result<(String, Option<String>, bool), String> {
    let mut name = String::new();
    let mut value: Option<String> = None;
    while let Some(c) = chars.next() {
        match (&mut value, c) {
            (_, ',') => return Ok((name, value, true)),
            (None, '=') => value = Some(String::new()),
            (None, c) => name.push(c),
            (Some(v), quote @ ('\'' | '"')) => {
                if let Err(e) = quoted(chars, quote, v) {
                    let name = name.trim();
                    return Err(if STEPS.contains(&name) {
                        format!("{name}: {e}")
                    } else {
                        format!("unknown step {name:?}")
                    });
                }
            }
            (Some(v), c) => v.push(c),
        }
    }
    Ok((name, value, false))
}

/// The rest of a quoted part, after its opening `quote`, onto `into`.
fn quoted(chars: &mut std::str::Chars, quote: char, into: &mut String) -> Result<(), String> {
    let open = || format!("no closing {quote}");
    loop {
        match chars.next().ok_or_else(open)? {
            c if c == quote => return Ok(()),
            '\\' => into.push(match chars.next().ok_or_else(open)? {
                c @ ('\\' | '\'' | '"') => c,
                'n' => '\n',
                't' => '\t',
                c => return Err(format!("unknown escape \\{c} (only \\\\, \\', \\\", \\n and \\t)")),
            }),
            c => into.push(c),
        }
    }
}

/// A step from its name and value.
fn step(name: &str, value: &str) -> Result<Step, String> {
    let wrong = |e: String| format!("{name}: {e}");
    Ok(match name {
        "wait-connect" => Step::WaitConnect(seconds(value).map_err(wrong)?),
        "wait" => Step::Wait(seconds(value).map_err(wrong)?),
        "click" => {
            let (x, y) = value.split_once(':').ok_or_else(|| wrong(format!("not X:Y: {value:?}")))?;
            Step::Click(number(x).map_err(wrong)?, number(y).map_err(wrong)?)
        }
        "key" => Step::Key(keysym(value).map_err(wrong)?),
        "clipboard" => Step::Clipboard(value.to_string()),
        "resize" => {
            let (w, h) = dimensions(value).map_err(wrong)?;
            Step::Resize(w, h)
        }
        "layout" => Step::Layout(value.split(';').map(dimensions).collect::<Result<_, _>>().map_err(wrong)?),
        "sound-cycle" => Step::SoundCycle(seconds(value).map_err(wrong)?),
        "out" if value.is_empty() => return Err(wrong("no file".into())),
        "out" => Step::Out(PathBuf::from(value)),
        "thumb" => {
            // A trailing `:N` is the scale; a file whose name ends in one
            // is written with the scale given after it.
            let (file, scale) = match value.rsplit_once(':') {
                Some((file, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => {
                    let n: usize = number(n).map_err(wrong)?;
                    if n == 0 {
                        return Err(wrong("the scale is 1/N, and N is at least 1".into()));
                    }
                    (file, n)
                }
                _ => (value, THUMB_STEP),
            };
            if file.is_empty() {
                return Err(wrong("no file".into()));
            }
            Step::Thumb(PathBuf::from(file), scale)
        }
        _ => return Err(format!("unknown step {name:?}")),
    })
}

/// Seconds as digits with an optional fraction: `2`, `0.5`, `.5`, `2.`.
fn seconds(value: &str) -> Result<Duration, String> {
    let wrong = || format!("{value:?} is not a number of seconds");
    let digits = value.bytes().filter(u8::is_ascii_digit).count();
    let dots = value.bytes().filter(|&b| b == b'.').count();
    if digits == 0 || dots > 1 || digits + dots != value.len() {
        return Err(wrong());
    }
    Duration::try_from_secs_f64(value.parse().map_err(|_| wrong())?).map_err(|_| wrong())
}

fn number<T: std::str::FromStr>(value: &str) -> Result<T, String> {
    value.parse().map_err(|_| format!("{value:?} is not a number"))
}

/// `WxH`, neither of them zero.
fn dimensions(value: &str) -> Result<(u32, u32), String> {
    let (w, h) = value.split_once('x').ok_or(format!("not WxH: {value:?}"))?;
    let (w, h) = (number::<u32>(w)?, number::<u32>(h)?);
    if w == 0 || h == 0 {
        return Err(format!("a screen has an area: {value:?}"));
    }
    Ok((w, h))
}

/// A keysym as probe.c's `strtol(.., 0)` reads one, so that `key=` means
/// the same key as that probe's `--key`: hex after `0x` or `0X` (`0x61`),
/// octal after any other leading zero (`0141`), decimal otherwise (`97`),
/// after leading white space and an optional `+`, with nothing after the
/// digits. `0o` and `0b` are not prefixes to strtol (without C23's), so they
/// are not here either. What does not fit a keysym's 32 bits, or is
/// negative, is refused rather than wrapped.
fn keysym(value: &str) -> Result<u32, String> {
    let wrong = || format!("{value:?} is not a keysym");
    // C's isspace, which is not Unicode's.
    let v = value.trim_start_matches([' ', '\t', '\n', '\x0b', '\x0c', '\r']);
    let v = v.strip_prefix('+').unwrap_or(v);
    let (digits, radix) = if let Some(hex) = v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
        (hex, 16)
    } else if v.len() > 1 && v.starts_with('0') {
        (&v[1..], 8)
    } else {
        (v, 10)
    };
    // from_str_radix takes a sign of its own, which strtol does not after
    // a prefix; so the digits are checked first.
    if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
        return Err(wrong());
    }
    u32::from_str_radix(digits, radix).map_err(|_| wrong())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split_(args: &[&str]) -> Result<Vec<Arg>, String> {
        split(args.iter().map(|a| a.to_string()).collect(), FLAGS)
    }

    fn f(long: &'static str, value: Option<&str>) -> Arg {
        Arg::Flag(long, value.map(str::to_string))
    }

    fn p(s: &str) -> Arg {
        Arg::Positional(s.to_string())
    }

    #[test]
    fn a_value_is_taken_in_each_of_the_four_forms() {
        let want = vec![f("user", Some("me"))];
        assert_eq!(split_(&["--user", "me"]), Ok(want.clone()));
        assert_eq!(split_(&["--user=me"]), Ok(want.clone()));
        assert_eq!(split_(&["-u", "me"]), Ok(want.clone()));
        assert_eq!(split_(&["-ume"]), Ok(want));
        // Taken whatever it looks like.
        assert_eq!(split_(&["--user", "--verbose"]), Ok(vec![f("user", Some("--verbose"))]));
        assert_eq!(split_(&["-u", "-c"]), Ok(vec![f("user", Some("-c"))]));
        assert_eq!(split_(&["--user="]), Ok(vec![f("user", Some(""))]));
        assert_eq!(split_(&["-o", "A=b=c"]), Ok(vec![f("option", Some("A=b=c"))]));
        assert_eq!(split_(&["--option=A=b"]), Ok(vec![f("option", Some("A=b"))]));
        assert_eq!(split_(&["-oA=b"]), Ok(vec![f("option", Some("A=b"))]));
    }

    #[test]
    fn short_options_stack_up_to_one_that_takes_a_value() {
        assert_eq!(split_(&["-cL"]), Ok(vec![f("connect", None), f("list", None)]));
        assert_eq!(split_(&["-cume"]), Ok(vec![f("connect", None), f("user", Some("me"))]));
        assert_eq!(split_(&["-cu", "me"]), Ok(vec![f("connect", None), f("user", Some("me"))]));
        // The rest of the word is the value, letters of other options and all.
        assert_eq!(split_(&["-ucL"]), Ok(vec![f("user", Some("cL"))]));
        assert_eq!(split_(&["-cP5"]), Ok(vec![f("connect", None), f("probe", Some("5"))]));
    }

    #[test]
    fn positionals_go_anywhere_and_after_a_double_dash_everything_is_one() {
        assert_eq!(
            split_(&["rfb", "-c", "host:1", "-"]),
            Ok(vec![p("rfb"), f("connect", None), p("host:1"), p("-")])
        );
        assert_eq!(split_(&["--", "-c", "--user"]), Ok(vec![p("-c"), p("--user")]));
        assert_eq!(split_(&[]), Ok(vec![]));
    }

    #[test]
    fn mistakes_are_refused_by_name() {
        assert_eq!(split_(&["--x11"]), Err("unknown option --x11".into()));
        assert_eq!(split_(&["--x11=1"]), Err("unknown option --x11".into()));
        assert_eq!(split_(&["-x"]), Err("unknown option -x".into()));
        assert_eq!(split_(&["-cx"]), Err("unknown option -x".into()));
        assert_eq!(split_(&["--user"]), Err("--user needs a value".into()));
        assert_eq!(split_(&["-u"]), Err("-u needs a value".into()));
        assert_eq!(split_(&["-cu"]), Err("-u needs a value".into()));
        assert_eq!(split_(&["--connect=yes"]), Err("--connect takes no value".into()));
        assert_eq!(split_(&["--hw-decode=vaapi"]), Err("--hw-decode takes no value".into()));
        // No abbreviations.
        assert_eq!(split_(&["--conn"]), Err("unknown option --conn".into()));
    }

    /// Every option is in the help, by its names and with a value where it
    /// takes one, and the help has no option the parser does not take.
    #[test]
    fn the_help_and_the_parser_have_the_same_options() {
        let help = usage(&Defaults { extlib_dir: "E".into(), state_dir: "S".into() });
        let lines: Vec<&str> = help.lines().filter(|l| l.starts_with("  -") || l.starts_with("      --")).collect();
        for flag in FLAGS.iter().filter(|f| !HIDDEN.contains(&f.long)) {
            let names = match flag.short {
                Some(c) => format!("  -{c}, --{}", flag.long),
                None => format!("      --{}", flag.long),
            };
            let line = lines
                .iter()
                .find(|l| l.starts_with(&format!("{names} ")))
                .unwrap_or_else(|| panic!("--{} is not in the help", flag.long));
            let after = &line[names.len()..];
            let has_value = !after.starts_with("  ");
            assert_eq!(has_value, flag.value, "--{}: {line:?}", flag.long);
        }
        for line in &lines {
            let long = line.split_whitespace().find_map(|w| w.strip_prefix("--")).unwrap();
            assert!(FLAGS.iter().any(|f| f.long == long && !HIDDEN.contains(&f.long)), "{line:?}");
        }
        for long in HIDDEN {
            assert!(FLAGS.iter().any(|f| f.long == *long), "--{long} is not an option");
            assert!(!help.contains(long), "--{long} is in the help");
        }
    }

    #[test]
    fn the_help_shows_the_defaults_and_its_descriptions_line_up() {
        let help = usage(&Defaults {
            extlib_dir: "/opt/vento/lib".into(),
            state_dir: "/home/u/.local/state/vento".into(),
        });
        assert!(help.contains("(default: /opt/vento/lib)\n"), "{help}");
        assert!(help.contains("(default: /home/u/.local/state/vento)\n"), "{help}");
        assert!(help.contains("none-resize) (default: auto)\n"), "{help}");
        assert!(help.contains("(default: 30)\n"), "{help}");
        assert!(help.contains("1/N (default: 8)\n"), "{help}");
        for placeholder in ["<version>", "<extlib-dir>", "<state-dir>", "<scale>", "<wait-connect>", "<thumb-step>"] {
            assert!(!help.contains(placeholder), "{placeholder}");
        }
        // Every option's description starts in the same column.
        for line in help.lines().filter(|l| l.starts_with("  -") || l.starts_with("      --")) {
            assert_eq!(&line[27..29], "  ", "{line:?}");
            assert_ne!(line.as_bytes()[29], b' ', "{line:?}");
        }
        assert!(!help.contains(['\u{2013}', '\u{2014}']));
    }

    #[test]
    fn the_version_is_n_for_a_release_s_0_0_n() {
        assert_eq!(display_version("0.0.6"), "v6");
        assert_eq!(display_version("0.0.12"), "v12");
        assert_eq!(display_version("1.2.3"), "v1.2.3");
        assert_eq!(display_version("0.1.0"), "v0.1.0");
        let help = usage(&Defaults { extlib_dir: "E".into(), state_dir: "S".into() });
        assert!(help.starts_with(&format!("vento {}\n\nusage:\n", version())), "{help}");
    }

    fn option(key: &str, default: &str, values: &[&str], description: &str) -> BackendOption {
        BackendOption {
            key: key.into(),
            default: default.into(),
            values: values.iter().map(|v| v.to_string()).collect(),
            description: description.into(),
        }
    }

    #[test]
    fn the_backends_are_listed_with_their_options_in_one_column() {
        let list = [
            Backend {
                id: "rfb".into(),
                name: "VNC (Rust)".into(),
                version: "RFB 3.8".into(),
                description: "Rust RFB implementation.".into(),
                options: vec![
                    option("ViewOnly", "false", &["true", "false"], "Do not send input."),
                    option("KeepAlive", "30", &[], "Seconds between keep-alives."),
                ],
            },
            Backend {
                id: "realvnc".into(),
                name: "VNC (RealVNC)".into(),
                version: "not available: libvento_realvnc.so: cannot open shared object file".into(),
                description: "RealVNC's client.".into(),
                options: vec![option(
                    "DesktopSize",
                    "1920x1200",
                    &["remote", "device", "1920x1200", "1920x1080", "1600x1000"],
                    "Preferred screen resolution.",
                )],
            },
        ];
        assert_eq!(
            backends(&list),
            "\
backends:
  rfb  VNC (Rust)  RFB 3.8
  Rust RFB implementation.
    -o ViewOnly=false [true|false]  Do not send input.
    -o KeepAlive=30                 Seconds between keep-alives.

  realvnc  VNC (RealVNC)  not available: libvento_realvnc.so: cannot open shared object file
  RealVNC's client.
    -o DesktopSize=1920x1200 [remote|device|1920x1200|1920x1080|1600x1000]
                                    Preferred screen resolution.
"
        );
        assert_eq!(backends(&[]), "backends:\n");
    }

    #[test]
    fn a_list_line_is_the_name_and_the_library_or_why_not() {
        assert_eq!(list_line("rfb", &Ok(PathBuf::from("/opt/vento/vento"))), "rfb /opt/vento/vento");
        assert_eq!(
            list_line(
                "libvnc",
                &Err((
                    PathBuf::from("/opt/vento/lib/libvento_libvnc.so"),
                    "libz.so.1: cannot open shared object file".into()
                ))
            ),
            "libvnc /opt/vento/lib/libvento_libvnc.so (error: libz.so.1: cannot open shared object file)"
        );
    }

    fn secs(s: f64) -> Step {
        Step::Wait(Duration::from_secs_f64(s))
    }

    #[test]
    fn a_spec_is_its_steps_in_order() {
        assert_eq!(
            steps(concat!(
                "wait=1,click=100:200,key=0x61,clipboard=hi,resize=1280x720,layout=800x600;1024x768,",
                "sound-cycle=2,out=a.ppm,thumb=b.ppm:4,thumb=c.ppm,wait=0.5"
            )),
            Ok(vec![
                secs(1.0),
                Step::Click(100, 200),
                Step::Key(0x61),
                Step::Clipboard("hi".into()),
                Step::Resize(1280, 720),
                Step::Layout(vec![(800, 600), (1024, 768)]),
                Step::SoundCycle(Duration::from_secs(2)),
                Step::Out(PathBuf::from("a.ppm")),
                Step::Thumb(PathBuf::from("b.ppm"), 4),
                Step::Thumb(PathBuf::from("c.ppm"), THUMB_STEP),
                secs(0.5),
            ])
        );
        assert_eq!(steps(""), Ok(vec![]));
        assert_eq!(steps("  "), Ok(vec![]));
        // Space around a name is ignored; a value is as written.
        assert_eq!(steps(" wait=1, key=97"), Ok(vec![secs(1.0), Step::Key(97)]));
        assert_eq!(steps("wait=1 ,key=97"), Err("wait: \"1 \" is not a number of seconds".into()));
        assert_eq!(steps("wait=1,wait=2"), Ok(vec![secs(1.0), secs(2.0)]));
        assert_eq!(steps("click=-1:5"), Ok(vec![Step::Click(-1, 5)]));
        assert_eq!(steps("sound-cycle=0"), Ok(vec![Step::SoundCycle(Duration::ZERO)]));
        for (spec, want) in [("wait=0", 0.0), ("wait=.5", 0.5), ("wait=2.", 2.0), ("wait=007", 7.0)] {
            assert_eq!(steps(spec), Ok(vec![secs(want)]), "{spec}");
        }
    }

    #[test]
    fn wait_connect_is_taken_only_as_the_first_step() {
        let wait_connect = |s: f64| Step::WaitConnect(Duration::from_secs_f64(s));
        assert_eq!(steps("wait-connect=5"), Ok(vec![wait_connect(5.0)]));
        assert_eq!(steps(" wait-connect=0.5,wait=1"), Ok(vec![wait_connect(0.5), secs(1.0)]));
        assert_eq!(steps("wait-connect='3',out=a.ppm"), Ok(vec![wait_connect(3.0), Step::Out(PathBuf::from("a.ppm"))]));
        assert_eq!(steps("wait-connect=0"), Ok(vec![wait_connect(0.0)]));
        let err = |spec: &str| steps(spec).expect_err(spec);
        let not_first = "wait-connect must be the first step";
        assert_eq!(err("wait=1,wait-connect=5"), not_first);
        assert_eq!(err("wait-connect=5,wait-connect=5"), not_first);
        assert_eq!(err("key=97,wait-connect=x"), "wait-connect: \"x\" is not a number of seconds");
        assert_eq!(err("wait-connect"), "wait-connect needs a value (wait-connect=...)");
        for bad in ["", "-1", "1s", "1.2.3", "inf", " 5", "0x10"] {
            assert_eq!(err(&format!("wait-connect={bad}")), format!("wait-connect: {bad:?} is not a number of seconds"));
        }
    }

    #[test]
    fn quotes_hold_commas_and_the_other_quote_and_take_escapes() {
        let clip = |spec: &str| match steps(spec) {
            Ok(s) => match &s[..] {
                [Step::Clipboard(text)] => text.clone(),
                other => panic!("{spec}: {other:?}"),
            },
            Err(e) => panic!("{spec}: {e}"),
        };
        assert_eq!(clip(r#"clipboard='a, b'"#), "a, b");
        assert_eq!(clip(r#"clipboard="a, b""#), "a, b");
        assert_eq!(clip(r#"clipboard="it's""#), "it's");
        assert_eq!(clip(r#"clipboard='say "hi"'"#), r#"say "hi""#);
        assert_eq!(clip(r#"clipboard="say \"hi\"""#), r#"say "hi""#);
        assert_eq!(clip(r#"clipboard='it\'s'"#), "it's");
        assert_eq!(clip(r#"clipboard='a\\b'"#), r"a\b");
        assert_eq!(clip(r#"clipboard="one\ntwo\tthree""#), "one\ntwo\tthree");
        assert_eq!(clip(r#"clipboard='\"'"#), "\"");
        // Parts in quotes and out of them run together, as in a shell.
        assert_eq!(clip(r#"clipboard=a'b,c'd"e,f""#), "ab,cde,f");
        // Outside quotes a backslash is itself, and = is part of the value.
        assert_eq!(clip(r"clipboard=C:\dir\n"), r"C:\dir\n");
        assert_eq!(clip("clipboard=a=b"), "a=b");
        assert_eq!(clip("clipboard= x "), " x ");
        assert_eq!(clip("clipboard="), "");
        assert_eq!(clip("clipboard=''"), "");
        assert_eq!(clip(r#"clipboard="""#), "");
        assert_eq!(clip("clipboard='é, ü'"), "é, ü");
        assert_eq!(
            steps(r#"clipboard="a, \"b\"",wait=1,out='my, file.ppm'"#),
            Ok(vec![Step::Clipboard(r#"a, "b""#.into()), secs(1.0), Step::Out(PathBuf::from("my, file.ppm"))])
        );
        assert_eq!(steps("thumb='a,b.ppm':2"), Ok(vec![Step::Thumb(PathBuf::from("a,b.ppm"), 2)]));
        assert_eq!(steps("layout='800x600;800x600'"), Ok(vec![Step::Layout(vec![(800, 600), (800, 600)])]));
    }

    #[test]
    fn a_broken_spec_is_refused_with_the_step_named() {
        let err = |spec: &str| steps(spec).expect_err(spec);
        assert_eq!(err("clipboard='a, b"), "clipboard: no closing '");
        assert_eq!(err(r#"clipboard="a\""#), "clipboard: no closing \"");
        assert_eq!(err(r"clipboard='a\"), "clipboard: no closing '");
        assert_eq!(err(r"clipboard='a\x'"), r#"clipboard: unknown escape \x (only \\, \', \", \n and \t)"#);
        assert_eq!(err("bogus='a"), "unknown step \"bogus\"");
        assert_eq!(err("bogus=1"), "unknown step \"bogus\"");
        assert_eq!(err("Wait=1"), "unknown step \"Wait\"");
        assert_eq!(err("'wait'=1"), "unknown step \"'wait'\"");
        assert_eq!(err("=1"), "unknown step \"\"");
        assert_eq!(err("bogus"), "unknown step \"bogus\"");
        assert_eq!(err("wait"), "wait needs a value (wait=...)");
        assert_eq!(err("wait=1,out"), "out needs a value (out=...)");
        assert_eq!(err("5"), "\"5\" is not a step (wait=5 waits 5 seconds)");
        assert_eq!(err(","), "step 1 is empty");
        assert_eq!(err(",wait=1"), "step 1 is empty");
        assert_eq!(err("wait=1,,out=a"), "step 2 is empty");
        assert_eq!(err("wait=1,"), "step 2 is empty");
        assert_eq!(err("wait=1, "), "step 2 is empty");
        assert_eq!(err("out="), "out: no file");
        assert_eq!(err("thumb="), "thumb: no file");
        assert_eq!(err("thumb=:4"), "thumb: no file");
        assert_eq!(err("thumb=a.ppm:0"), "thumb: the scale is 1/N, and N is at least 1");
        assert_eq!(err("thumb=a.ppm:99999999999999999999999"), "thumb: \"99999999999999999999999\" is not a number");
        // A colon not followed by digits belongs to the file name.
        assert_eq!(steps("thumb=a:b.ppm"), Ok(vec![Step::Thumb(PathBuf::from("a:b.ppm"), THUMB_STEP)]));
        assert_eq!(steps("thumb=a.ppm:"), Ok(vec![Step::Thumb(PathBuf::from("a.ppm:"), THUMB_STEP)]));
    }

    #[test]
    fn a_bad_number_is_refused_by_step() {
        let err = |spec: &str| steps(spec).expect_err(spec);
        for bad in ["", "-1", "+1", "1s", "1.2.3", ".", "inf", "nan", "1e3", " 1", "1 ", "0x10"] {
            assert_eq!(err(&format!("wait={bad}")), format!("wait: {bad:?} is not a number of seconds"));
            assert_eq!(err(&format!("sound-cycle={bad}")), format!("sound-cycle: {bad:?} is not a number of seconds"));
        }
        assert!(err("wait=99999999999999999999999").starts_with("wait: "));
        // X,Y is two steps, the first of them broken.
        assert_eq!(err("click=1,2"), "click: not X:Y: \"1\"");
        assert_eq!(err("click=1"), "click: not X:Y: \"1\"");
        assert_eq!(err("click=1:"), "click: \"\" is not a number");
        assert_eq!(err("click=a:2"), "click: \"a\" is not a number");
        assert_eq!(err("click=1:2:3"), "click: \"2:3\" is not a number");
        assert_eq!(err("click=1.5:2"), "click: \"1.5\" is not a number");
        assert_eq!(err("key="), "key: \"\" is not a keysym");
        assert_eq!(err("key=a"), "key: \"a\" is not a keysym");
        assert_eq!(err("resize=1280"), "resize: not WxH: \"1280\"");
        assert_eq!(err("resize=0x720"), "resize: a screen has an area: \"0x720\"");
        assert_eq!(err("resize=1280x-1"), "resize: \"-1\" is not a number");
        assert_eq!(err("resize="), "resize: not WxH: \"\"");
        assert_eq!(err("layout="), "layout: not WxH: \"\"");
        assert_eq!(err("layout=800x600;"), "layout: not WxH: \"\"");
        assert_eq!(err("layout=800x600;0x1"), "layout: a screen has an area: \"0x1\"");
    }

    #[test]
    fn a_keysym_is_read_as_strtol_with_base_zero_reads_it() {
        assert_eq!(keysym("0x61"), Ok(0x61));
        assert_eq!(keysym("0X61"), Ok(0x61));
        assert_eq!(keysym("97"), Ok(0x61));
        assert_eq!(keysym("0141"), Ok(0x61));
        assert_eq!(keysym("0"), Ok(0));
        assert_eq!(keysym("00"), Ok(0));
        assert_eq!(keysym(" \t+0xff0d"), Ok(0xff0d));
        assert_eq!(keysym("0xffffffff"), Ok(u32::MAX));
        for refused in [
            "0o141", "0b101", "08", "0x", "", " ", "0x+61", "0x-61", "-1", "97 ", "1e3", "0x1ffffffff", "++1",
        ] {
            assert!(keysym(refused).is_err(), "{refused:?}");
        }
    }

    /// Every step is in the help, at the options' column, and the help's
    /// example is a spec the parser takes.
    #[test]
    fn the_help_lists_every_step_and_its_example_parses() {
        let help = usage(&Defaults { extlib_dir: "E".into(), state_dir: "S".into() });
        let section = &help[help.find("probe steps (").expect("no probe steps section")..];
        let rows: Vec<&str> = section.lines().skip(1).take_while(|l| !l.is_empty()).collect();
        assert_eq!(rows.len(), STEPS.len(), "{rows:?}");
        for (row, name) in rows.iter().zip(STEPS) {
            assert!(row.starts_with(&format!("  {name}=")), "{row:?} is not {name}");
            assert_eq!(&row[27..29], "  ", "{row:?}");
            assert_ne!(row.as_bytes()[29], b' ', "{row:?}");
        }
        assert!(section.contains("scaled 1/N (default: 8)\n"));
        assert!(section.contains(concat!(r#"inside quotes, \\ \' \" \n \t are escapes"#, "\n")), "{section}");
        let example = section.lines().find_map(|l| l.strip_prefix("  e.g., --probe ")).expect("no example");
        let example = example.strip_prefix('\'').and_then(|e| e.strip_suffix('\'')).expect("not in shell quotes");
        let parsed = steps(example).unwrap();
        assert!(parsed.contains(&Step::Clipboard(r#"a, "b""#.into())), "{parsed:?}");
        assert!(help.contains("  vento <backend> <address> --probe SPEC ...  headless probe\n"));
        assert!(help.contains("  -P, --probe SPEC  "));
    }
}
