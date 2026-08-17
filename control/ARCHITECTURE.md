# `control` — the cursor, viewport and gesture model

This library is a clean re-implementation of the touchpad-style pointer control
in RealVNC Viewer for Android (4.9.1.60165), reverse-engineered and then rebuilt
rather than ported line by line. It owns the cursor, the viewport and the whole
touch-gesture stack, and it emits nothing but **absolute pointer events**:
a position in remote-desktop pixels plus a button mask. It knows nothing about
VNC, about the app around it, or — outside the four adapters named at the end of
this document and the drawing in `ui` — about Android at all, and the model runs
on a plain JVM under a virtual clock, which is how it is tested.

[README.md](README.md) is how to build it and run its tests. This document is
what it does, in three sections: what the original does (a specification,
complete enough to reimplement from), the bugs in it that we fix, and the things
we do that it does not.

Everything that differs is behind a flag in `input/Config`. `Config.faithful()`
restores the original's behaviour, bug for bug; `Config.improved()` is what ships.

Sections 2 and 3 say where each item came from. Unless a ***Source*** line says
otherwise, it came out of the reverse engineering itself and the work that
followed it here; a *Source* line names the earlier investigation, or the person
whose direction or verdict it was.

---

## 1. The original behaviour

### 1.1 The model

The remote desktop is a framebuffer of `W × H` pixels. The viewer shows a
rectangular window onto it, scaled uniformly:

```
screen = desktop × scale + origin
```

The **cursor** is owned by the client. It has a position in desktop pixels,
clamped to `[0, W-1] × [0, H-1]`, and it is the client that decides where that
position goes; the remote machine is only ever told the result, as an absolute
position. Touch input never moves the cursor to where a finger is: fingers
supply *relative* motion, like a laptop trackpad.

**The centre-follow rule is the entire trick of the interface.** After every
cursor movement, the viewport is re-centred on the cursor and then clamped to
the desktop bounds:

```
ox = -(cursor.x × scale - viewportWidth / 2)
origin.x = (W × scale < viewportWidth)
         ? (viewportWidth - W × scale) / 2            // desktop narrower than the window: centre it
         : clamp(ox, viewportWidth - W × scale, 0)    // otherwise never show past an edge
```

and the same for `y`. In the interior of the desktop the clamp is inactive, so
the cursor sits *exactly* at the centre of the window and the desktop slides
underneath it. Near a desktop edge the clamp saturates and the cursor slides off
the centre towards that edge instead. There is no special case for "cursor near
the edge": it falls out of the clamp.

### 1.2 Touch events and pointer tracking

Touches are tracked per pointer. At most **two** pointers are tracked in detail,
in the order they arrive (which matters — see §2.3). A running high-water mark of
how many fingers have been down at once during the current gesture is kept, and
it is what selects the mouse button. Both are reset only when the last finger
lifts.

### 1.3 Tap versus drag

Each tracked pointer carries a "has moved" flag, set the first time either test
passes:

- its displacement from its **initial** position is at least **12 dp** on either
  axis (per-axis, not Euclidean distance); or
- it has produced more than **10** move events, regardless of distance.

A gesture counts as moving when one finger is down and that finger's flag is
set, or two fingers are down and **both** flags are set. (With one finger down
the original always reads the *first* slot's flag rather than the surviving
finger's — see §2.3.) Cursor motion, wheel scrolling and zooming only happen
while the gesture counts as moving; a gesture that never does is a tap.

Both flags are cleared on every touch-down, including the second finger's.

### 1.4 Clicks, and the 250 ms window

Clicks are decided when the **last** finger lifts, never on touch-down. If the
gesture never moved, a button is pressed according to the high-water finger
count:

| Fingers | Button |
|---|---|
| 1 | left |
| 2 | right |
| 3 | middle |
| 4 or more | nothing |

The press is emitted immediately and a release is scheduled **250 ms** later.
Lifting the fingers of a multi-finger tap one at a time is fine — only the last
one triggers the decision, and the high-water mark still holds the count.

The one rule that produces every compound gesture: **a touch-down that arrives
while a release is pending cancels that release.** The button then stays held.
Everything below is that single mechanism, with nothing else special-casing it —
there is no double-tap timer and no tap counter anywhere:

| Sequence | Result |
|---|---|
| tap, then tap again within 250 ms | the second lift releases the held button and presses again: down, up, down, up — a **double click** |
| tap, then press and drag within 250 ms | the gesture moves, so no new click is decided; the button stays held for the drag and is released at the end — a **left-button drag** |
| two-finger tap, then drag within 250 ms | the tap pressed the right button and it was never released — a **right-button drag** |
| three-finger tap, then drag within 250 ms | the same, with the middle button |

A drag also releases any held button when it ends, and emits no click of its own.

### 1.5 Single-finger drag

While a one-finger gesture is moving, each move event's raw delta (in screen
pixels) is:

1. passed through the acceleration curve and emitted as cursor motion, and
2. fed **unaccelerated** to the momentum sampler.

The emitted delta is converted to desktop pixels by dividing by the current
scale, added to the cursor position, clamped to the framebuffer, sent to the
remote, and then the viewport is re-centred per §1.1.

#### Acceleration

The curve is driven by **jerk** — the change in per-event displacement — not by
speed. A three-sample history of `(dx, dy, t)` is kept. Whenever it is full:

```
jerk   = |delta[1] - delta[0]|            // the two oldest samples, as a vector difference
dt     = t[2] - t[0]                      // the span of all three
factor = clamp((√1.1 + jerk / dt) ^ 2.6, 1.1, 5.0)
```

If `dt` is zero the previous non-zero `dt` is reused. The history is then
**emptied**, not slid, so a computed factor only occurs on every third move
event; the other two get the floor of 1.1. The emitted motion is therefore a
`1.1, 1.1, F, 1.1, 1.1, F` pattern.

Consequences worth stating explicitly, because they are the feel:

- Movement at constant velocity has zero jerk and is amplified by 1.1 no matter
  how fast it is. Only *changes* in speed are amplified. This is what makes the
  interface snappy on a flick and precise on a slow drag without any explicit
  precision mode.
- The factor saturates readily: at a 16 ms sample span, a jerk of 0.5 px/ms
  already gives ≈3.0, and 1 px/ms saturates at 5.0.
- Because `jerk / dt` falls as the sampling interval shrinks, the same physical
  flick amplifies **less** on a 120 Hz touch digitiser than on a 60 Hz one. The
  curve is sampling-rate dependent.

#### Momentum

Raw deltas are also pushed into a separate three-sample history. When the last
finger lifts, a glide starts if the last two touch **positions** were more than
**2 px** apart, after a **50 ms** delay — a delay any further move cancels, so a
glide only ever follows a finger that really left the screen.

The glide's direction and speed come from the two **oldest** of the three
samples, over the span of all three:

```
v     = delta[0] + delta[1]
speed = min(|v| × 25 / (t[2] - t[0]), 200)
dir   = v / |v|
```

Then every **10 ms**: emit `dir × speed`, multiply `speed` by **0.85**, and stop
once it drops to **3** or below. Total glide is about `6.7 × speed` pixels — a
speed of 60 carries ~400 px over ~185 ms, and the 200 cap carries ~1330 px over
~260 ms.

Using the oldest samples rather than the newest adds lag, and combined with the
50 ms delay it means deliberately slowing down at the end of a flick kills the
momentum. That is a feature: it is how you throw the cursor without being unable
to stop it. Glide output does **not** go through the acceleration curve.

### 1.6 Bump scroll

Armed only when a pending click release was cancelled — that is, during a
tap-then-drag of any button, which is exactly when the user is dragging
something and may need to reach past the edge of the window.

While armed, on every move event, a fixed delta of **12 dp** is emitted towards
each edge the finger is currently within **24 dp** of, and a timer repeats that
same delta every **100 ms** (first repeat after 100 ms) until the finger lifts.
It lets a window be dragged to the far side of a desktop larger than the window
without lifting a finger.

### 1.7 Two-finger gestures

When a second finger lands, the gesture is armed: the distance between the two
fingers is recorded as the pinch reference, their midpoint is recorded, and that
midpoint (converted to desktop coordinates) becomes the fixed **scale focus**
for the rest of the gesture. The mode starts undecided.

While both fingers are down and both have moved, and while the mode is still
undecided, it is decided from a single ratio:

```
travel = |finger0 - finger0.start| + |finger1 - finger1.start|
spread = | |finger0.start - finger1.start| - |finger0 - finger1| |
ratio  = spread / travel
```

- `ratio > 0.7` → **zoom**
- `ratio < 0.2` → **scroll**
- in between → stays undecided, and is re-tested on the next move.

Once decided it does not change for the rest of the gesture.

**Zoom.** Every subsequent move sets the scale to `scaleAtGestureStart ×
(currentSeparation / initialSeparation)`, holding the scale focus point fixed,
and then re-centres the cursor on the middle of the window (§1.9).

**Scroll.** Two-finger scrolling sends **wheel events to the remote**; it does
not pan the viewport. The midpoint's movement since the last event is
accumulated per axis, and one wheel click is emitted per **8 px** of accumulated
raw (unscaled) travel, the remainder being carried. Wheel clicks are a press and
an immediate release of the corresponding button. A "natural scrolling"
preference inverts both axes; it applies to two-finger scrolling only, not to a
physical wheel.

**Three or more fingers** moving does nothing at all — no cursor motion, no
zoom, no scroll. The click decision on release still happens by finger count.

### 1.8 Scale limits, quantisation and snapping

- Maximum scale is `2 × floor(display density)`.
- Minimum scale is the scale that fits the whole desktop in the window.
- Every requested scale is quantised to **1/128**.
- If the resulting desktop width lands within **±4%** of a "nice" width, it snaps
  to it. The nice widths are integer pixel-doubling (`W × k` for
  `k = 1 … floor(density)`), fit-width, and fit-height.

The zoom buttons step through a separate sorted ladder, rebuilt whenever the
framebuffer or the window changes size:

```
1.0, 0.66, 0.66², … down to the fit scale, plus 1.5, 2.0, fit and fill
```

A host may add rungs to it, by handing over **sizes worth fitting** — the scale
at which a region of the desktop that big fills the window, one rung each. What
a region is is the host's business and this library does not ask; a size whose
scale the ladder already stops on is dropped like any other collision.

The rung index is re-derived from the current scale on **every** scale change,
not only on a button press, so a zoom button pressed after a pinch steps from
where the scale actually is rather than from the last rung visited. A zoom
button uses the **cursor** as the scale focus and then re-centres the cursor.

### 1.9 Re-centring the cursor

"Centre the cursor" places it at the middle of the window: the window centre is
converted to desktop coordinates, that becomes the cursor position, it is sent
to the remote, and the viewport is re-centred on it. A pinch step and a zoom
button both do this, so zooming keeps the cursor under the middle of the screen
rather than letting it drift with the scale.

### 1.10 Cursor rendering

The remote sends a cursor bitmap and an offset. The bitmap is drawn at
`cursorScreenPosition + offset × cursorScale`, where the offset is the cursor's
hotspot **already negated** by the protocol layer, and

```
cursorScale = max(display density, 1) × (largest bitmap side > 32 ? 32 / largest side : 1)
```

so a large remote cursor is capped at 32 logical pixels.

### 1.11 Absolute-position input (hover, stylus, physical mouse)

Input that carries an absolute position does not go through any of the above: it
sets the cursor to that position directly, and the viewport pans by a fixed step
whenever the cursor is inside a band at the window edge — a **60 dp** band, a
**30 dp** step, repeating every **32 ms**, scaled by how far into the band the
cursor is, and stopping when the desktop edge is reached or the cursor leaves the
band.

### 1.12 The mouse button / wheel overlay

An optional widget in the bottom-right corner, shown and hidden from the
toolbar. It is the only way to hold a mouse button for longer than the 250 ms
window of §1.4, and the only reliable middle click. Its layout is an L: a
**60 dp** strip up the right edge for the wheel, and a **72 dp** row across the
bottom holding left, middle and right in weights **2:1:2** — the middle never
narrower than 100 dp — with a 40 dp dismiss button in the corner. The rest of
the screen keeps working as the touchpad, so one finger holds a button while
another drags the cursor.

**The buttons follow the touch**; they do not latch. Press emits a button-down,
release a button-up, and sliding off a button does not release it. Showing the
overlay hides the extension keyboard, and vice versa.

**Showing either overlay insets the desktop.** The viewport size and the
cursor's clamp rectangle both come off a content rect that subtracts, by layout
mode: the system insets when the keyboard is up, and a fixed
`Rect(0, 0, 0, 46 dp)` when *either* overlay is up. So the desktop shrinks and
re-centres rather than sliding underneath — though see §2.10 for what that
constant does to the mouse overlay, which is not 46 dp tall.

**The wheel strip is a rate control, not a per-pixel wheel.** A drag handle,
`stripHeight/3 + 40 dp` tall, sits in the middle of the strip; where the finger
is *is* the rate, from 0 in the middle to **±4** at the ends:

```
rate  = clamp((y − stripCentre) / (stripHeight/2), −1, 1) × 4
period = ceil(3 / |rate|) ticks of 40 ms
```

One click is emitted the moment scrolling starts; then **8 ticks (320 ms)** pass
before the repeat clock starts counting, which is a key-repeat delay and is what
makes a single click easy to land. So the strip runs from ~25 clicks/s at the
ends to one click every few hundred ms near the middle, and it never needs the
8 px-per-click rule of §1.7.

### 1.13 The extension keyboard

The other optional widget, and the keyboard's counterpart to §1.12: a **46 dp**
row of the keys a soft keyboard has no concept of, above a **30 dp** info bar,
above the system IME. Showing it hides the mouse overlay, and dismissing it
brings that overlay back if it was up.

The keys are one horizontally scrolling row of six groups, separated by 16 dp
either side of a divider: the six modifiers (Shift, Ctrl, Alt, Windows, Option,
CMD — the last two of which share `XK_Super_L` and differ only in label);
Backspace, Del, Esc, Tab, Ins; Enter; the four arrows; Home, End, Page Up, Page
Down; and F1 … F12. Keys fire on **release**, both edges back to back.

**Modifiers are sticky in three states — off, one-shot, locked — and in the
latter two the key is genuinely held down at the remote.** Entering either state
sends a key-down, leaving it sends a key-up, and every one-shot is released
either by the next key of any kind or by the next **mouse button release** — a
click consumes an armed modifier exactly as a key does, which is what makes
Ctrl+click a chord and, being the release rather than the press, leaves a
Shift+drag its Shift for the whole drag. Every producer of buttons goes through
the one release call, the wheel included (§3.19). That, and nothing else, is what joins a Ctrl pressed
here to a C typed on the system IME: the two never meet on the client. A single
tap arms a one-shot or disarms; a **double tap** locks (see §2.11 for what that
costs), and so does a hold — the second gesture is ours, and free. A key of any
kind is the only thing that consumes one; ours adds the click (§3.19).

**A character typed on the soft keyboard is put in the case the held modifiers
imply.** The soft keyboard cannot see them, so it reports the character it would
have produced on its own — and a far end asked for `c` while holding a Shift that
says otherwise resolves the disagreement by letting go of the Shift, which turns
Ctrl+Shift+C into Ctrl+C. So: Shift held, upper case; any other modifier held
without Shift, lower case, so that a keyboard which capitalised on its own does
not turn Ctrl+c into Ctrl+Shift+c; nothing held, exactly what was typed. Letters
only — which character Shift and `1` make is a property of a layout at the far
end. (`Keysym.forCharacter`, from the original's `z2.m`, which logs
*Capitalising* and *Converting to lower case* as it does the same thing.)

The **info bar** carries one status light per modifier — drawn from the key's
symbol (⇧ ⌃ ⎇ ⊞ ⌥ ⌘) rather than its name, since six words in a 30 dp bar is
clutter — a readout of the line being typed (Backspace erases, Return clears the
whole buffer, anything with a character appends), a privacy toggle that masks
every character as `●`, and a dismiss button.

### 1.14 Output to the remote

Every cursor movement produces one absolute pointer event. There is **no
coalescing, no deduplication and no rate limiting** anywhere in the client:
neither in the cursor driver, nor at the native boundary, nor in the protocol
library's rate limiter, which exists but defaults to a pass-through and is never
configured. Held buttons are re-sent with every position.

### 1.15 Constants, in one place

| Constant | Value |
|---|---|
| Tap → drag displacement | 12 dp, per axis, from the initial position |
| Tap → drag move count | more than 10 move events |
| Click auto-release, and the compound-gesture window | 250 ms |
| Buttons by finger count | 1 → left, 2 → right, 3 → middle, ≥4 → none |
| Acceleration history | 3 samples, emptied when full |
| Acceleration curve | `clamp((√1.1 + jerk/dt) ^ 2.6, 1.1, 5.0)` |
| Momentum start delay / tick | 50 ms / 10 ms |
| Momentum speed | `min(\|v\| × 25 / dt, 200)`, decay 0.85 per tick, stop at 3 |
| Momentum gate | last two touch positions more than 2 px apart |
| Bump scroll | 24 dp border, 12 dp step, every 100 ms |
| Pinch vs. scroll | `spread / travel` > 0.7 zoom, < 0.2 scroll |
| Wheel | one click per 8 px of midpoint travel |
| Scale | max `2 × floor(density)`, min fit, quantum 1/128, snap ±4% |
| Zoom ladder | `0.66ⁿ` from 1.0 down to fit, plus 1.5, 2.0, fit, fill, and any size the host asks to fit |
| Edge auto-scroll (absolute input only) | 60 dp band, 30 dp step, 32 ms |
| Cursor bitmap cap | 32 logical px |
| Overlay | 60 dp wheel strip, 72 dp button row 2:1:2 (middle ≥ 100 dp), 40 dp dismiss |
| Overlay desktop inset | a constant `Rect(0,0,0,46 dp)` for either overlay |
| Overlay wheel | rate `±4` across the strip, `ceil(3/\|rate\|)` ticks of 40 ms, 8-tick start delay |
| Extension keyboard | 46 dp key row, 30 dp info bar, 8 dp key padding (12 dp for arrows and F-keys), 16 dp either side of a group divider |
| Extension keyboard gestures | tap (on release) sends; two taps within 300 ms lock a modifier, as does a hold; a hold repeats at 75 ms on the arrows, Backspace, Del, Tab and the page keys |

---

## 2. Bugs we fix

Each of these is a defect rather than a taste difference: the behaviour is
either unreachable, silently wrong, or contradicts what the rest of the code is
trying to do. All are switchable, so the original behaviour can still be felt
side by side.

### 2.1 Stationary taps are swallowed on high-report-rate screens

*The bug.* The "more than 10 move events ⇒ drag" test has no distance component,
and a modern digitiser reports sub-pixel jitter at 120 Hz or more. A perfectly
ordinary 150 ms tap easily exceeds ten move events, is classified as a drag, and
emits no click at all. Measured on a Pixel 9: one-finger taps produce 11–12 move
events against a limit of 10, so the failure is not marginal, it is the common
case. Two- and three-finger taps stay under five, which is why right-click keeps
working while left-click does not.

*The fix.* Drop the count test. In its place, a touch is promoted to a drag when
its **accumulated path length** reaches **24 dp** — twice the displacement
threshold. That still catches the case the count test was accidentally covering
(a slow scrub that never gets 12 dp from where it started) without being
reachable by jitter in place. It is not immune in principle — enough wobble is
still enough wobble — but it takes a deliberately wobbly tap to trip it.

*Source.* Patrick's own `pgaskin/vncpatch#1`, the Pixel 9 missed-taps
investigation: both the measurement above and the diagnosis are its. The fix
here is a different one — that patch raises the count, this drops it for a
distance.

### 2.2 Coordinates are truncated to integers

*The bug.* Touch coordinates are truncated to `int` as they enter the gesture
layer. On a high-density screen that is a real loss of accuracy for slow
movement, and it interacts badly with the jerk-based acceleration curve: slow
motion produces jerk values of 0 or 1 px, quantising the amplification.

*The fix.* Floats end to end.

### 2.3 Lifting the first of two fingers kills the drag

*The bug.* Pointer slots are assigned by arrival order and matched on subsequent
events by comparing the previous position for equality. Two consequences: with
one finger left down, the match for the second slot is unreachable, and the
"has moved" flag consulted is always the first slot's. So lifting the finger
that went down **first** and continuing to drag with the other silently discards
every subsequent move event.

*The fix.* Slots are keyed by pointer id, and the moving test consults whichever
slot is actually still down.

### 2.4 The acceleration and momentum histories are never cleared

*The bug.* Neither three-sample history is reset between gestures. The first
move of a new drag can therefore be paired with samples from a completely
different, older gesture — producing a spurious jerk (and so a spurious
amplification), or a glide computed from a mix of two gestures.

*The fix.* Both are cleared on touch-down. A gesture with fewer than two move
events then simply cannot glide, which is the desired behaviour anyway.

### 2.5 Bump scroll never stops

*The bug.* Once armed, the repeating timer is only cancelled when the fingers
lift. Moving back out of the border zone recomputes the step to `(0, 0)` and the
timer keeps firing zero-motion events every 100 ms for the rest of the gesture.

*The fix.* Leaving the border stops the timer.

### 2.6 A glide runs through the next touch

*The bug.* Putting a finger down does not cancel a running glide, so a new
gesture fights the tail of the previous one.

*The fix.* Touch-down cancels the glide. (This one is a judgement call more than
a defect, and is flagged as such — it is switchable like the rest.)

### 2.7 The zoom ladder walks sideways, and has dead rungs

*The bug.* Two problems, both visible on the first screen anyone looks at.
Ladder rungs are compared against the raw requested scale rather than the
snapped, quantised scale that is actually applied, so stepping to a rung lands
on a slightly different number and the "nearest rung" lookup can then pick a
neighbour — the ladder walks. And rungs that coincide are kept, so the ladder
contains steps that do nothing: fit and fill are identical whenever the desktop
and the window share an aspect ratio, and on a desktop smaller than the window
everything below fit collapses onto fit.

*The fix.* Rungs are snapped at build time, through the same snapping the scale
setter applies, and colliding rungs are dropped.

### 2.8 The overlay's wheel keeps scrolling after a cancelled touch

*The bug.* The wheel strip stops scrolling on touch-up and on nothing else. A
cancelled touch — the gesture being taken over by a parent, the window losing
focus mid-drag — leaves the rate set and the 40 ms timer running, scrolling the
remote for as long as the session lasts.

*The fix.* Cancel stops it, and so does hiding the overlay, which likewise
releases any button it is holding: a button held down by a widget that is no
longer on the screen can never be released by the user.

### 2.9 One producer of button presses can release another's button

*The bug.* Both the overlay and the gesture layer write the same button mask,
last writer wins. Hold left with the overlay, tap the touchpad during the drag,
and the tap's own press-and-release of left — 250 ms later, per §1.4 — releases
the button the overlay is still holding, dropping whatever was being dragged.
(The original has the same structure; it is simply hard to reach, because you
rarely tap while dragging.)

*The fix.* Each producer keeps its own mask and the remote is told the union.
A button stays down until everybody holding it has let go.

### 2.10 The overlay inset is the wrong overlay's height

*The bug.* The rectangle subtracted from the desktop when an overlay is showing
(§1.12) is a constant built once at startup from `extension_keyboard_height`:
`Rect(0, 0, 0, 46 dp)`, whichever overlay is actually up. For the mouse overlay
that is wrong in both directions — its button row is **72 dp**, so 26 dp of live
desktop stays under it, and its wheel strip is **60 dp of the right-hand edge**,
which is not inset at all. The cursor can therefore be centred behind the
buttons, which is precisely what the inset is for. (It is also short for the
keyboard, whose info bar adds 30 dp above it.)

*The fix.* The overlay reports what it actually covers and the viewport insets
by that. The extension keyboard likewise reports its two bars **and the IME's
height under them**, which the original's constant ignores entirely.

### 2.11 Pressing an extension key twice quickly sends it once

*The bug.* Locking a modifier is a double tap, so every key on the row is behind
a double-tap detector — and the double-tap handler returns immediately for keys
that are not modifiers. The second tap of a rapid pair is routed there instead of
to the single-tap handler, and is discarded. Pressing Backspace twice quickly
deletes one character.

*The fix.* Keep the gesture and move the question: only a **modifier** is ever
asked whether it was double-tapped, so an ordinary key's second tap goes down the
same path as its first. Locking is still two rapid taps.

*Source.* Patrick's `pgaskin/vncpatch`, patch 0003, which reported it. The fix
there removes the double-tap detector from the keys that are not modifiers;
moving the question instead keeps the original's locking gesture on the keys
that are.

### 2.12 Two keys sharing a keysym release it twice

*The bug.* Windows and CMD are two keys with one keysym (`XK_Super_L`). Arming
both and then disarming one sends a key-up for a key the other is still holding.
Hard to reach, and harmless against a forgiving remote, but wrong.

*The fix.* The remote hears one press when the first holder arms and one release
when the last lets go.

## 3. Our enhancements

These are additions, not corrections. The ones that change how the cursor moves
were measured against every recorded gesture before being turned on, and then
judged on a device against the original running beside them; the measurement is
`AccelSweepTest`, which is checked in, and the verdicts are recorded here as
*Source* lines.

### 3.1 Adaptive acceleration

The original's curve is jerk-based, so a *steady* slow drag already sits at the
1.1 floor and needs no help. What it mishandles is a slow drag with jitter in
it: small direction changes at low speed **are** jerk, and get amplified exactly
where precision was wanted.

So the amplified part of the factor — never the floor — is faded out at low
speed:

```
factor = 1.1 + (factor - 1.1) × smoothstep((speed - 0.15 dp/ms) / (0.60 - 0.15 dp/ms))
```

against a smoothed speed estimate kept independently of the three-sample jerk
window. Below 0.15 dp/ms the cursor tracks the finger at the floor; above
0.60 dp/ms the original's factor comes through untouched. Those thresholds sit
either side of nothing: the recorded precision gestures run at 0.02–0.20 dp/ms
and the purposeful ones at 0.6–4.4 dp/ms, two decades apart.

Measured across every recorded gesture, this is exactly inert on all
nine gestures faster than 0.5 dp/ms — identical factors, peak included — and on
the slow ones it removes the 2–3× spikes that appear mid-gesture.

*Source.* **Patrick's, idea and design.** The problem it solves is the one he
named — the original amplifies exactly where precision was wanted — and it ships on his
verdict from a session with this and the original on the same phone: "feels nice
and makes it easier to drag precisely". The curve above is ours, and its
measured effect is the smallest of the three mechanisms that went into that
session; it is one of the two that survived it.

### 3.2 Axis locking

Below **0.25 dp/ms**, when the smoothed per-axis magnitudes are lopsided enough
(minor/major at or below **0.30**, releasing again above **0.55**), the minor
axis of the movement is zeroed. Dragging along a row of text, or down a column,
stops wandering.

Three things make it feel like assistance rather than rails:

- **The ratio is measured from smoothed magnitudes, not the current event.** At
  these speeds one delta is a pixel or two and its own ratio is noise; a
  per-event test chatters in and out of lock every frame.
- **The estimates are fed the raw delta, never the locked one**, or the minor
  axis's estimate would decay to zero and the lock would latch permanently.
- **A curve is not an axis.** A circle is near-axial four times per revolution,
  so the ratio test on its own flattens four arcs of it — 35–50% of a slow
  circle. What distinguishes a circle from a drag along an axis is not the
  instantaneous direction, which is identical, but that the direction *keeps
  turning*. So the direction is low-passed over two different path lengths and
  the angle between the two estimates gates the lock: no locking while the path
  has turned more than **10°** over the last **36 dp**. Smoothing by path length
  rather than by event count makes that a curvature threshold that means the
  same thing across the speed band; magnitude-weighted vectors keep sub-pixel
  jitter from dominating it.
- **A corner is not a curve.** A turn sharper than **45°** drops the direction
  history outright, so "down the page, then across" locks on both legs a few
  events after the turn instead of a whole span later.

The zeroed motion is discarded, not carried: releasing it on unlock would
conserve distance but deliver it as a sideways jump, which is the opposite of
the point.

*Source.* **Patrick's, idea and design, as §3.1 is.** The feature is his — the
original has nothing like it — and so is the rule that saves it from being rails: do not lock
while the direction is turning, so a proper circle can still be drawn, but still
lock when selecting text up, then across. Built and measured here, and kept on
his verdict from the device — "it makes it much easier to do precise movements
while not feeling like it's on rails for normal motion". The thresholds are
ours, chosen from a sweep over synthetic circles and corners with the recorded
straight drags holding their locking unchanged.

### 3.3 Coalescing and deduplicating pointer events

The original sends one absolute pointer event per accelerated move event, with
no throttling anywhere. We coalesce motion to one event per frame — a button
change flushes immediately, so a click is never delayed — and drop any event
whose position **and** button mask are identical to the last one sent.

Both are safe only because the protocol is absolute: a dropped duplicate cannot
lose distance the way a dropped relative delta would. (The protocol library's
own rate limiter follows exactly this rule when enabled, and refuses to coalesce
relative-pointer messages for the same reason.)

Deduplication alone removes **31%** of the pointer events across the recorded
fixture set, all of them exact repeats. Most come from pinch: re-centring the
cursor on every zoom step re-sends a position the remote already has. Holding a
finger still — the case that motivated it, since a stationary finger keeps
producing move events and a cursor pressed against a desktop edge has every
delta clamped away — is not even in the fixture set, so 31% is a floor.

### 3.4 Window insets in the viewport clamp

The original applies layout insets to the viewport rectangle only in some of its
fullscreen modes, and the mode it normally runs in passes zero. We always apply
them, so the content rectangle is the window minus whatever the system bars or
an on-screen keyboard occupy, and the clamp keeps the cursor inside the part of
the window the user can actually see.

The clamp also takes **pan margins**, which are the opposite question and are
off by default. A phone's window is not a rectangle: a rounded corner, a camera
cutout and a system bar all sit over a desktop drawn edge to edge, and the row
of pixels against one of them can be seen but not squarely looked at or
comfortably tapped. A margin is room to slide the picture *past* its own edge,
leaving blank beside it, so that edge can be brought out from under the
obstruction. Unlike an inset it changes nothing derived from the window — not
the fit scale, the zoom ladder, or the centre the cursor sits at.

It is a distance from the edge of the window rather than an amount of travel, so
blank that is already there counts towards it: an axis whose desktop overflows
may be slid the whole margin, one where the desktop is smaller than the window
and the gap beside it is narrower than the margin may be slid the difference,
and one where the gap is already wider than the margin does not move, because it
is where the margin wanted it anyway. Which makes the rule continuous across the
size at which the desktop exactly fills its window — a float, and one the fit
scale lands either side of by a fraction of a pixel, whereas whether an edge is
under a rounded corner plainly is not that question.

The app sizes them from two settings, which are two reasons to want the same
thing and are added rather than gated on each other, so either alone is a
margin. One is what that side costs: the bars and cutout inset, or the larger
radius of the two rounded corners on that side, whichever is bigger, less any
inset that has already moved the picture clear. The other is a flat number of
pixels on every side, about where an edge is comfortable rather than what is
over it. Both are off — false and zero — by default.

### 3.5 Toolbar tap regions

The original puts a floating toolbar on top of the desktop. Instead, rectangles
of the touch surface — given in fractions of the view, so rotation moves them
with it — can claim a **tap** and suppress the click it would otherwise have
made. The default layout is a top band of 2/22 of the height split 1:4 and a
bottom band of 3/22 split 4:1, i.e. the parts of the screen you do not normally
drag through.

The hook is on the tap decision itself, not a view over the desktop. So a drag
that merely *starts* in a band reaches the cursor untouched, which matters
because bump scroll (§1.6) operates in those same bands by design. Two further
rules: only a one-finger tap can claim a region, so two- and three-finger clicks
still work everywhere; and the handler returns whether it consumed the tap, so
an inactive region falls through to an ordinary click.

*Source.* The **layout** is Patrick's, from `pgaskin/vncpatch` patch 0006, taken
wholesale. The mechanism is not: that patch puts clickable `View`s over the
desktop, and a clickable view eats the whole touch stream, so a drag that merely
starts in a band never reaches the cursor — in the very bands where bump scroll
is supposed to work.

### 3.6 The overlay shares the touch surface instead of covering it

The original's overlay is made of real `View`s, which swallow every touch that
lands on them, and its touch dispatcher then needs an explicit special case to
notice a second finger landing on the desktop and feed it to the gesture layer.
Ours inverts that: the overlay gets first refusal on each new pointer
(`TouchRouter.Claim`), takes the ones inside its own geometry, and every other
pointer reaches the gesture layer untouched. Same outcome for the case both
support, one mechanism instead of two, and it is testable off Android.

Two behaviours fall out of it that the original does not have. A **tap anywhere
on the wheel strip is exactly one click** — the original ignores a press unless
it lands on the handle, and then does nothing until the finger moves. And
**bump scroll (§1.6) arms for a drag started while the overlay holds a button**,
not only inside the 250 ms window: dragging past the edge of the window is
exactly what bump scroll is for, and a long drag is what the overlay is for.

The overlay also reports the rectangle it covers rather than a constant, so the
viewport insets by its real geometry — the strip's width on the right as well as
the row's height at the bottom (§2.10, §3.4).

### 3.7 The keyboard's key list belongs to the caller

The original hard-codes an X11 key set. Ours takes the list — `standardKeys()`
is that same set, as a default, and `twoLineKeys()` those keys in the two-line
grouping of §3.21 — because protocols differ in which modifiers they
even have: there is no Option on an RDP session, and Super and Meta are not
everywhere the same key. A backend that cannot express a key should not draw it.
Nothing in `ExtensionKeyboard` is keyed on a particular keysym; the info bar's
status lights are one per modifier *in the list*, and the X11 spelling lives in
`Keysym`, so a backend speaking something else translates at its own edge.

Each key carries an optional **icon name** beside its label, for the same reason:
`control` must not know what a bitmap is, so it says *which* glyph and the
renderer decides how — falling back to the label for a name it does not know.
Which keys have an icon is the original's choice (Shift, Windows, Option, CMD,
Backspace, Enter, the arrows; Ctrl and Alt are words), and the info bar's status
lights use the same names, showing a bare initial for the two without one.

A drag along the row also **scrolls it rather than pressing a key**, which the
original gets from a `HorizontalScrollView` swallowing the gesture and ours gets
from the same claim mechanism as §3.6: 8 dp of horizontal movement abandons the
key. Both come to the same place, but ours is one rule in one class and runs
under the test harness — as does the fling that goes with it (§3.17).

### 3.8 A tap-duration limit (available, off)

A touch held longer than a configurable duration can be classified as a drag
rather than a tap. The original has no such rule — a long hold still clicks —
and side-by-side testing said it should stay that way, so the limit defaults to
disabled. It exists because it is the natural companion to the path threshold in
§2.1 and is cheap to keep.

*Source.* Ours; **off on Patrick's verdict**, taken with the original running
beside it.

### 3.9 Letting go of the far end when the screen goes away

Everything this package holds down is held down *there*: a button is a button at
the remote, and a locked modifier is a key the remote thinks is still pressed.
Three things outlive the gesture that started them, all deliberately — the
button inside the 250 ms click window (§1.4), a glide (§"Momentum"), and a
locked modifier (§1.13) — and each of them is wrong the moment the session is no
longer on screen, because the finger that would have ended it is gone with the
screen.

`TouchRouter.cancel` is "every pointer is gone, whatever the screen thinks", and
`GestureRecognizer.cancelAll` is that plus the three survivors. A host calls
both, with `ExtensionKeyboard.clearModifiers`, when its session leaves the
screen. The original does none of this: switching away
from it mid-drag leaves the button down at the far end.

### 3.10 The picture does not jump when the window changes shape

An overlay appearing shrinks the content rect (§3.4), and the original's response
moves the whole desktop by half of it — once when the mouse overlay is shown and
again when it is hidden. Two rules replace that, one per case:

- **Where the cursor holds the picture** (the desktop is bigger than the window
  on that axis), the origin is not independent state: it is
  `clamp(contentCentre − cursor × scale)`, so when the content centre moves,
  either the cursor moves or the desktop does. `CursorController.setInsets`
  picks the cursor, which is where the model says it lives anyway. Only on an
  axis where the cursor really is at the centre — against a desktop edge the
  clamp owns the origin, the desktop has to follow the content edge, and moving
  the pointer off what it was aimed at would be worse.
- **Where nothing holds it** (the desktop is smaller than the window on that
  axis — a 1920×1200 desktop is exactly this vertically on a portrait phone),
  `Viewport.centreOn` centres it in the *view* and then pushes it inside the
  content rect, rather than centring it in the content rect. It cannot move
  while there is room, it moves the minimum when there is not, and it returns
  on its own when the inset goes.

### 3.11 Action keys

A key on the extension row can carry a **name instead of a keysym**
(`Key.action`): tapping it reports the name to the listener and sends nothing,
so the app decides what it means. Same shape as §3.5's caller-supplied region
names, and for the same reason — the one action `standardKeys()` carries is
Paste, which has to read a clipboard and may have to put a dialog on screen,
neither of which belongs in a plain-JVM model. It still consumes the armed
modifiers, because it is a key press as far as anyone watching the row is
concerned, and it stays out of the info bar's readout, which is a record of
typing. The original has no such thing.

*Source.* The mechanism is ours; **Paste's shape is Patrick's**, from a device
session. It was first a Ctrl+V chord, which works only if the far end's
clipboard is reachable and its desktop pastes with that shortcut; typing the
clipboard out is what works against a machine whose clipboard this end cannot
reach.

### 3.12 Haptics that can be turned off

`Config.keyboardHaptics` gates the row's feedback callbacks at the model rather
than at the view, so the view is not asked at all. A soft keyboard's buzz is a
matter of taste, and the system's own keyboard-haptics setting does not reach a
view that draws its own keys.

### 3.13 A physical mouse, captured, with no gesture recognition in its path

`PhysicalMouse`. Not §1.11's design at all: the original feeds a mouse through
the *touch* path, so its buttons go through tap/drag disambiguation and its
position comes from the local pointer, which fights the client-side cursor the
rest of this package is built on.

Here a mouse is its own producer feeding the same `MouseSink` the gesture layer
does, so the centre-follow desktop, the edge clamp and the zoom come free, and
`PointerAccel` and `PointerInertia` are not in the path at all. `Config.mouseSpeed`
is the only dial, defaulting to 1:1 — the phone has already applied its own
pointer profile. Natural scrolling, which §1.7 says the original applies to
two-finger scrolling and not to a wheel, applies to this wheel too: a preference
about which way scrolling goes that only some of the phone's input devices obey
is worse than either answer.

`Config.mouseCapture` asks for `View.requestPointerCapture()`, which delivers
**relative** deltas and stops the local pointer leaving the window. That is the
correct primitive for a remote-desktop client and the original never asks for it,
which is precisely why it needs §1.11's edge auto-scroll: without capture the
remote pointer can only go where the local one can. Capture ends on every focus
loss and there is no callback offering it back, so it is re-requested on every
focus gain. The uncaptured path is kept, derives deltas from successive hover
positions, and drives the same model.

Buttons come from `MotionEvent.getButtonState()` as a whole mask — which is why
`MouseSink` takes a mask — and include `Button.BACK` and `Button.FORWARD`,
buttons 8 and 9, which no gesture can produce. The wheel accumulates
`AXIS_VSCROLL`/`AXIS_HSCROLL` in notches, so a high-resolution wheel produces the
same clicks per turn as a detented one; `Config.mouseWheelStep` is the gearing.

*Source.* Ours end to end — the original has no design for a mouse beyond
§1.11's — and driven with a real HID mouse rather than injected events, which is
the only way to see a captured pointer at all. Several of the behaviours above
are corrections Patrick asked for from that session.

### 3.14 A physical keyboard, with the phone owning the layout

`PhysicalKeyboard`. The rule is one sentence — **a printing key sends the
character the phone's own layout produces for it, and a non-printing key sends
the keysym for its position** — and it is the rule `ui/TextInput` already
followed, which is what lets a hardware keyboard, the system IME and the
extension row all reach the far end as one keyboard.

Three things follow, each of which could have gone the other way:

- **The lock keys are not forwarded.** Caps Lock, Num Lock and Scroll Lock are
  applied to the character here. Sending them too applies them twice and leaves
  two lock states to drift apart.
- **Ctrl and left Alt are masked out of the character lookup**, so Ctrl+C is `c`
  with `Control_L` held. A layout asked about Ctrl+C usually answers with
  nothing, so handing it the whole `metaState` loses every shortcut. Right Alt is
  *not* masked out: on a PC layout it is AltGr and the character is the point,
  and `ISO_Level3_Shift` goes out beside it so the far end finds the level held.
  Where the layout has *nothing* on the third level — which is every US one — the
  key falls back to its own character, so that a level the far end has and this
  phone does not is still reachable rather than doing nothing at all.
- **The keypad is its own set of keysyms** (`Keysym.keypadKeysym`), chosen by the
  Num Lock state: with the lock off the keypad *is* the navigation cluster.

The left and right modifiers are **different keys**, and go out as `Shift_L` and
`Shift_R`, `Control_L` and `Control_R`, `Super_L` and `Super_R` — Android
distinguishes them and there is no reason to throw that away. Right Alt is the
exception above, and is a level rather than a modifier.

A layout with dead keys answers the character lookup with the accent and bit 31
set rather than with a code point, and what goes out for it is the X11 dead key
(`Keysym.fromKeyChar`): a dead key is a key, and what makes it worth pressing is
that the far end composes it with whatever follows.

An auto-repeat re-sends **the keysym the key went down with**, not the one its
current modifiers imply. Android reports repeats with the metaState as it is now,
so holding `a` and then pressing Shift would otherwise overwrite the far end's
held-key entry with `A` and leave `a` down for ever.

`PhysicalKeyboard.reserved` is the list this client keeps for itself: Back above
all, since it is how a session is left, plus the volume and media keys and the
lock keys.

Two things a walk of a real keyboard settles, neither of them guessable. Android's
keycodes are a list that grows with each release and is not in keyboard order —
F13 is 326, where F12 is 142 — so the held-key table is grown to fit rather than
sized once, since a press it cannot record is a key held down at the far end for
ever. And the Meta keys never arrive at all: the window manager takes the key,
alone and in every chord, so `Super` reaches a far end only from the extension
row's own key.

*Source.* Ours, from the same session as §3.13, with the same eight behaviour
fixes asked for while it ran. The auto-repeat rule is the one thing nothing
pointed at: the symptom is a key stuck down at the far end, minutes later, and
nothing about it names the cause.

### 3.15 The system IME, without lying to it

`ui/TextInput`, `input/TextDelta`. The row of §1.13 is a *supplement* to a soft
keyboard, so the screen hosting it opens a real IME — against an editor declaring
`TYPE_NULL`, which is a client saying it has no document. An IME told that falls
back to sending key events, which is exactly what a remote machine wants, and it
is why a Gboard switched to Korean sends `KEYCODE_G KEYCODE_K KEYCODE_S` for
ㅎ ㅏ ㄴ: the position is reported and the far end's own input method composes it.

**The original does the opposite** — a hidden 1 dp `EditText` with real text in
it, a `TextWatcher` that diffs the old string against the new one, and the
password input variation to stop the IME learning what is typed. That buys
autocorrect (behind a preference that defaults to off) at the price of three
device-specific workarounds for an IME that can see the editable: a Gboard that
deletes twice, a Samsung Shift that was never pressed, and a "delete nothing"
that has to mean one Backspace. None of them applies to an editor that is not
there.

The one thing worth taking from it is the diff, and it is taken. An IME revises
its composing region by sending it again in full, so `TextDelta` keeps what was
sent and emits the *difference* — common prefix kept, the rest of the old region
backspaced, the rest of the new one typed, one key press per code point. Without
it a revising IME turns `h`, `he`, `hel` into `hhehel` at the far end.

Four consequences of having no document, each answered rather than left:

- **`setComposingRegion` is refused.** Text already sent cannot be taken back
  into a region, so honouring it would type the replacement and leave the
  original.
- **A deletion ends the region**, because where it lands relative to a document
  this end cannot read is unknowable.
- **The IME's clipboard key** asks the *editor* to paste rather than committing
  text; there is no editor, so it types the clipboard out (§3.11).
- **A committed newline is Return**, not the well-formed keysym for code point
  10 that no server has a key for (`Keysym.forCharacter`).

Both paths in — a key event and committed text — also ask the row what it is
holding, and put the character in the case that implies (§1.13). Everything else
about a modifier is the far end's business, but this one cannot be: the two
keyboards are only joined there, and by then it is too late to say which `c` was
meant.

Every call in and every keysym out can be traced with
`adb shell setprop log.tag.TextInput VERBOSE`.

### 3.16 A far end that owns the cursor

`CursorController.setRelative`. Some servers do not want to be told where the
pointer is, only how far it moved — QEMU's, in practice, whenever the virtual
machine's mouse is an ordinary PS/2 one. The mode is announced by the server, not
chosen here, and it turns off the four lines of `mouseMove` that are the whole of
§1.1: the deltas go out as deltas, `x` and `y` stand still, and the viewport
follows nothing because nothing here knows where the pointer is.

What that costs is everything the position was for — the centre-follow, the
cursor drawn locally, zooming about the pointer; §3.18 is what a desktop bigger
than the window is navigated with instead — and what survives is
everything that was about the *finger* all along: the gesture recognition, the
button masks and their union, the wheel, and bump scroll, which measures the
finger against the edge of the screen and emits deltas. That the change is this
small is the finding rather than the design: the stack has emitted
`mouseMove(dx, dy)` from the beginning, and this class was the only thing
integrating it.

**Nothing shapes the motion either**, which is `Config.rawMotionWhenRelative`
and is on: acceleration, its adaptive gate, the axis lock and the glide after a
flick are four ways of answering *how far does the cursor go for this finger*,
and in this mode the far end answers it. It applies its own acceleration to
whatever arrives — the acceleration that machine's own user lives with — and a
curve on top of a curve is nobody's. So `GestureRecognizer.setRelative` is told
alongside the controller, and a one-finger drag goes straight to the sink. The
switch exists because "off" is a defensible taste: a phone is not a mouse, and
somebody may want the touchpad to feel like the rest of the app whatever is at
the other end.

Two details are not obvious:

- **A delta is still divided by the scale**, and that is not shaping — a finger
  crossing a zoomed-in screen covers fewer desktop pixels, and the picture is
  this end's fact whoever owns the pointer. It survives the switch above.
  (Whether it should survive for a *captured mouse*, whose pixels are somebody
  else's, is a separate question about the source rather than about the mode.)
- **The fraction is owed, not lost.** The wire carries whole pixels and a
  careful drag is a stream of deltas each smaller than one; rounding each on its
  own sends nothing at all. The remainder is kept and paid out on a later frame.

The dedupe of §3.3 inverts with the mode, exactly as that section predicted it
would have to: a second delta identical to the first is a second real movement,
so the only frame worth swallowing is one with nothing owed and no button
change.


### 3.17 Key repeat on the extension row

Holding a key on the original's row does nothing at all, and for Backspace and
the arrows — which is most of what the row is for — every character costs a tap.

A hold here repeats at **75 ms** after the long-press timeout, on the keys where
holding means "again": the four arrows, Backspace, Del, Tab, Page Up and Page
Down. Nothing else repeats, because a hold that turns into eight Escs or eight
Enters is a mistake and there is no undo at the far end. The timeout itself
sends nothing either: a key's single press is always its **release**, so a hold
that ends before the first repeat is a slow tap rather than a keystroke landing
at the moment a modifier would have locked. (A hold on a modifier locks it; the
original's gesture detector swallows the whole touch instead.)

Each repeat also plays the lightest haptic the device has — a text-cursor tick —
and none at all on hardware whose actuator cannot do better than a buzz, since
thirteen buzzes a second is a drill rather than feedback.

The row also **flings**: release velocity carries it on, decaying per tick until
it stops or reaches an end, and a new touch catches the glide instead of pressing
a key. The original inherits that from `HorizontalScrollView`; ours has to build
it, because the whole keyboard is one view that draws itself (§3.6).

*Source.* Patrick's `pgaskin/vncpatch` patch 0004 adds it to the original for
the same reason. Which keys repeat, the release-is-the-press rule and the
haptic are ours.

### 3.18 A pinch that also pans

A two-finger gesture is decided once and latches (§1.7), and a pinch then uses
only the **separation** of the two fingers. The travel of their midpoint is
thrown away, and it is a pan — the only one available where the far end owns the
cursor (§3.16), because the centre-follow is what navigates a desktop bigger
than the window and a relative pointer is precisely the absence of the position
it follows.

So `ZoomSink.zoomPanned` reports the midpoint's travel while the pinch is
engaged, and a host that wants it calls `Viewport.panBy`.

**A pan moves the focus point, not the origin.** The origin is not state: it is
`clamp(contentCentre − focus × scale)`, recomputed by every `centreOn`, and
§3.10 rests on its being derived rather than stored. A pan written to the origin
would be silently undone by the next thing that recomputes it — an inset change,
an overlay, a desktop resize. Moving the focus keeps the clamp, and with it both
edge rules at no cost: a pan cannot push the desktop off its window, and an axis
where the desktop is smaller than the window does not pan at all.

**Whether it pans is the host's**, and the answer is "when the cursor is
theirs". With the cursor ours the picture is already wherever the cursor is: a
pan would be undone by the next finger movement on the touchpad, and with the
cursor re-centred on every pinch step — §1.9, and the default — it would instead
drag the pointer across the far end's desktop, sending pointer events for a
gesture that is about looking rather than pointing. Which is also why the
expressible alternative in that mode, panning by dragging the cursor along with
the picture, is wrong rather than merely unimplemented.

Stated so it is not reported as a bug: a pinch still zooms about the point the
fingers *started* on rather than the point they are on. Anchoring the moving
midpoint is the same arithmetic as the pan, but it needs an anchor that survives
`centreOn` redefining the focus as the centre of the window, and in absolute
mode it fights the re-centring above.

### 3.19 A scroll is not a click

The original clears the armed modifiers on **every** button release, and the
wheel is a button: each notch of a two-finger scroll is a press and a release of
button 4, 5, 6 or 7, so a Ctrl armed for Ctrl+scroll survives exactly one notch
of the gesture and the rest of it arrives unmodified. Ctrl+scroll zooms and
Shift+scroll goes sideways on most desktops, and neither is a one-notch
instruction.

So the edge is taken over the buttons somebody clicks with — `Button.CLICK_MASK`,
the three under the hand and the two at the side — and the wheel's four
pseudo-buttons are excluded from it. Everything else about the rule is §1.13's:
one-shots go, locked ones stay, and it happens on the release. This is the one
deliberate departure from the original in the whole of that rule, and it was
kept rather than reverted once the original's behaviour was established.

The edge itself belongs to `CursorController` rather than to the gesture layer,
because that is where every producer of buttons meets: the gesture layer, the
overlay and a real mouse each hold their own mask (§2.9), and "the last one has
been let go" is a question about the union — a tap during a mouse-held drag ends
no click. It reports `Listener.onButtonsReleased`, the host calls
`ExtensionKeyboard.externalClick`, and a host with no such row implements
neither.

### 3.20 Hover assist

The far end says one thing about what is under the pointer, and it says it by
changing the cursor's shape. Where that arrives while the finger is moving
slowly, the pointer **loses a little distance**: the factor steps down to
**0.25** and comes back to 1 over **12 dp** of finger travel, on a smoothstep, so
the whole cost of a detent is **4.5 dp** of movement and the thing the far end
just reported has a widened dwell region around it. A small target is easier to
stop on and harder to slide off.

It is not a snap — nothing is pulled anywhere and nothing teleports — and it is
not target detection: where the boundary was, how big the thing is and what it
is are all unknown, and the design follows from that.

**The news is a reply, and it is late.** It was caused by a position sent a
round trip ago, so the pointer is already past the boundary when it arrives.
Measured across eight clients and two kinds of far end, that interval is 10 to
30 ms where the server is told its cursor changed and about 140 ms where it
polls its own screen — 2.5 to 7.5 dp of travel against 35 dp at the speeds this
engages at. Three things follow: the detent is applied *ahead* rather than
centred on anything, the span has to be longer than the lag's travel, and a far
end late enough to be reporting somewhere else must not arm it at all.

Five tests before it arms, and four of them are the defence against a cursor
that changes shape for reasons that are not a boundary:

- **Slow.** Above **0.35 dp/ms** — a little above the axis lock's band — a
  purposeful drag is not aiming at anything.
- **Moving.** No move event in the last **120 ms** means the change was not
  caused by us. This one alone disposes of every animation on an idle desktop.
- **Somewhere new.** Less than **6 dp** of travel since the last change that
  armed is a far end cycling frames, not two boundaries.
- **Not a burst.** More than **4** changes in **600 ms** locks the whole
  mechanism out for **2 s**. This is the animated wait cursor running *while*
  the finger moves slowly, which is the one case the first three all pass. It
  counts the changes that pass those three rather than everything that arrives:
  a hand crossing a desktop full of small objects makes a dozen on the way, and
  locking out because of them locks out the aim that follows them. Four rather
  than three because a slow sweep across small things really does cross three
  of them; an animated cursor runs at ten frames a second or more and still
  trips it inside one window.
- **Not too late.** A running estimate of the far end's own lateness, above
  **60 ms** of which nothing arms. It is measured rather than configured, out
  of the changes that arrive after a gesture has ended — the only ones whose
  cause is known, because nothing has moved since. It cannot tell a late reply
  from a change the far end made for itself, and does not have to: both are
  arguments against arming.

Two more rules keep it assistance rather than rails. **A reversal cancels it**:
the direction turning back on itself is somebody coming back to what they
overshot, and slowing that down is the opposite of help. And **the withheld
distance is discarded, not banked** — conserving it would turn the assist into
a delayed jump, which is §3.2's argument for the zeroed minor axis.

**In finger travel rather than desktop pixels**, which is a decision: it costs a
constant amount of movement and buys `4.5 dp / scale` of desktop stickiness —
full strength zoomed out, where a link is four screen pixels tall and the help
is wanted, and negligible zoomed in, where the same rule would cost real travel
for nothing. It multiplies the accelerator's factor rather than replacing it,
so inside a detent in the slow band the effective factor is about 0.28. It is
off where the far end owns the cursor, for §3.16's reason, and a physical mouse
never sees it: a mouse has the precision this is for.

On in `improved()`, on his verdict from the phone, as §3.1 and §3.2 are; the
input settings have a row for turning it off.

*Source.* **Patrick's, idea and design, as §3.1 and §3.2 are** — the original
has nothing that reacts to what the far end says. The measurement is what
shaped it: the fifth test exists because the same server software on one machine
was ten times later than another on the same machine over the same network, so
lateness is neither the network's nor a thing a build can be configured for.

### 3.21 The key row can be two lines

The same keys, grouped the way a keyboard groups them: modifiers over the
editing keys, the arrows as an inverted T, the F-keys six over six.

```
  modifiers                ⇤   ↑   ⇥      PgUp    paste    f1-f6
  bksp del esc tab ins     ←   ↓   →      PgDn    return   f7-f12
```

Home and End are drawn as arrows-into-a-bar here and are words on the one-line
row, and the page keys are abbreviated: the group is three glyphs wide and a word
in the middle of it is what stops the cluster reading as one thing.

A group is a **grid whose columns are shared between its lines** — the *n*th key
of one line sits in the same column as the *n*th key of the next, and the column
is as wide as the wider. That is the whole of the layout rule, and the arrows are
what it is for: sharing the columns is what puts Home above Left and End above
Right, the two keys that mean "the far end of this line, that way" sitting on the
axis they mean. Per-line layout is one line of code shorter and gives a ragged
cluster. The modifiers are the one group whose columns do not correspond, six
over five, and that costs nothing. A useful consequence: both lines are the sum
of the same columns, so both come out the same width and the scroll, the clamp
and the fling stay one number.

A line among others is **40 dp** rather than the 46 dp of a row on its own, which
is where it stops: 40 dp is the overlay's dismiss button, the smallest target
this screen already asks a finger for. So the chrome over the IME is 110 dp
instead of 76. Measured against the phone these notes are driven on, the keys
come to **1465 dp on one line and 765 dp on two** — three screenfuls of scrolling
in a 360 dp portrait window against not quite two, and in landscape the whole
set fits with nothing to scroll at all, which needs no code because a row that
fits is already centred.

`Key.row` is a layout attribute exactly as `group` and `wide` are, and the key
list stays **flat**: a key's position in it is its state slot and its id at the
far end (§3.7), so a list of lists would make all three two-dimensional for the
sake of one of them. A list that never sets `row` is laid out exactly as it was
before there were lines.

**The list can be swapped while a session is running** (`setKeys`), which is what
lets a host offer a choice without a reconnect. Its ordering is the whole of the
work: every held modifier is let go of **first, through the old list**, since an
id is a position in that list and a release sent after the swap names an id the
far end never saw pressed — a modifier left down on somebody's machine for the
rest of the session. The active touch, the timers and the fling go with it, each
of them holding a key that may not be in the new list.

---

## What is deliberately not implemented

- The absolute-position input paths of §1.11 and their edge auto-scroll.
  Physical input is here (§3.13, §3.14) but as its own design: relative motion
  from a captured pointer, so there is nothing for an edge auto-scroll to paper
  over.
- **Stylus.** It is a third input model — absolute, with a hover of its own — not
  a variant of either, so `PhysicalMouse` refuses it deliberately. The original
  refuses it too.
- Persisting the natural-scrolling preference, which needs a settings store this
  package deliberately does not have.
- The overlay's presentation: the original's artwork, its 300 ms fade, and
  moving the zoom widget up by the button row's height while it is shown.
  `MouseOverlay` hands out rectangles and the view draws them.
- **Output-displacement spreading**, which was built, measured and then deleted.
  It smoothed the acceleration's `1.1, 1.1, F` ripple by paying an amplified
  event out over the following frames, and it was the best-behaved of the three
  mechanisms measured — exact conservation of distance, no calibration constant.
  It went on Patrick's verdict, which disagreed with the measurement: the
  smoothing was imperceptible and the latency it added to a flick was not.

## Layout

| | |
|---|---|
| `Viewport` | scale, origin, the centre-and-clamp rule, snapping, the zoom ladder, insets, the pan and its margins |
| `CursorController` | cursor position in desktop pixels, button state, event coalescing and deduplication |
| `input/TouchRouter` | touch events → per-pointer callbacks, keyed by pointer id |
| `input/GestureRecognizer` | the state machine of §1.2–1.7 |
| `input/PointerAccel` | acceleration, the adaptive gate, axis locking |
| `input/PointerInertia` | the glide, and the samples it is computed from |
| `input/Config` | every tunable, and the two presets |
| `input/TapRegions` | regions of the view that a tap activates instead of clicking |
| `input/MouseOverlay` | the button / wheel overlay: geometry, hit testing, the wheel's repeat |
| `input/PhysicalMouse` | a real mouse: capture, relative motion, the button mask, the wheel (§3.13) |
| `input/PhysicalKeyboard` | a real keyboard: keycode + modifiers → keysym, held keys, repeat, the keys we keep (§3.14) |
| `input/ExtensionKeyboard` | the key row and info bar: the key list, geometry, sticky modifiers, key repeat |
| `input/Keysym` | X11 keysyms, and the Unicode and Android-keycode mappings onto them |
| `input/MouseSink`, `input/ZoomSink`, `input/RegionSink`, `input/KeySink` | what the input layer emits. `KeySink` carries a key **id** as well as a keysym, because a release names a key: see its javadoc |
| `input/Scheduler` | timers behind an interface, so the stack runs on a virtual clock |
| `ui/Chrome` | the one place the overlay, the key row, the info bar and the cursor become ink |
| `ui/KeyIcons`, `ui/TextInput` | keycap glyphs, and the system IME turned into keysyms |
| `input/TextDelta` | the IME's composing region as a difference, so a revision corrects rather than appends |
| `ui/Hud` | the debug readout's box: the fit, the panel and a rate meter, with the lines left to the caller |
| `playground/` | the fake desktop, its widgets and the fixture recorder: the library's own demonstration, and what the tests were recorded against |

Two rules hold this shape, and neither is left to memory. **Nothing here may
depend on an app or on androidx**: the build is its own, so a reach outward is a
resolution failure rather than a convention. And **the decisions live off
Android**: `Viewport`, `CursorController` and all of `input` are plain Java, and
where a class has to meet the platform it meets it in one method at its edge —
`TouchRouter`, `PhysicalMouse` and `PhysicalKeyboard` unpack a `MotionEvent` or
a `KeyEvent` and call the same methods the tests call, and `AndroidScheduler` is
a `Handler` behind `Scheduler`. That is what lets the whole stack run under a
virtual clock, which is the only reason any of this is testable at all. `ui/`
and `playground/` are the exception and are meant to be: they draw, with
`android.graphics` alone.
