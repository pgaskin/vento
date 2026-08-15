# control

Library for implementing touchpad-style remote desktop controls.

Feel free to take this and use it in other apps, as I'd love to see nicer controls in other apps too. I licensed this part MIT to make it easier.

### Features

- Intuitive gestures:
  - Pinch to zoom.
  - Tap to click.
  - Two-finger tap to right click.
  - Three-finger tap to middle click.
  - Double-tap (one/two/three-finger) then drag with one finger to drag.
  - Two finger drag to scroll.
  - Cursor momentum.
- Accuracy assists:
  - Increased precision for slow motion.
  - Axis locking.
  - Adaptive acceleration.
- Optimizations:
  - Deduplicated/coalesced pointer events.
- UI features:
  - Window insets.
  - Tap regions for actions.
  - Release keys/touches when losing focus.
- Physical keyboard/mouse support.
  - Proper modifier handling.
  - Best-effort layout handling.
- Android IME fixes:
  - Composition workarounds.
  - Send newline as enter key.
  - Workarounds for various buggy implementations.
- Customizable keycodes.
- Mouse buttons/wheel.
- Extension keyboard.
- Simulated text paste.

### Idea

The initial idea is inspired by the RealVNC Android app, which, IMO, is the nicest one, but I've made many fixes and improvements on top of it.

I've been wanting to do this for a while, but never ended up finding it worth the time due to the complexity of gesture handling alone, nevermind the difficulty of reverse-engineering math-heavy obfuscated code.

I gave Claude a high-level list of the features (based on what I'd noticed while using it over the years), then told it to reverse the RealVNC app (given a jadx decompilation and an apktool disassembly) and produce a clean spec of the math and gesture handling state machine, then to implement it in Java, in a way suitable for integrating it into an Android app. As it did it, I tested it in real-time (which it had the idea to record and create a test suite with) to ensure it worked the same way.

I also gave it my [vncpatch](https://github.com/pgaskin/vncpatch) repo so it could implement a few of my fixes from there and also use my reversing notes.

Next, I gave it a bunch of ideas for improving the controls and had it implement them so I could test it, with it fine-tuning it as I gave it feedback (this would have taken a lot of effort without Claude since I discarded a lot of ideas).

Afterwards, I had it consolidate it and my feedback into [ARCHITECTURE.md](./ARCHITECTURE.md) for future reference and started using it to build the rest of the app around it.

Some of the bugs in RealVNC which are fixed here:

- Missing taps on screens with high report rates (due to de-jittering based on report count vs total movement) (also originally worked around this myself in [pgaskin/vncpatch#1](https://github.com/pgaskin/vncpatch/issues/1)).
- Jagged motion on high-density screens when moving slowly (due to integer truncation).
- Unreliable dragging in some cases (due to it depending on the finger order).
- Cursor with momentum not fully stopping when touching.
- Various extension keyboard button weirdness.

### Usage

It depends on Java 21 and the Android SDK (but most of it is pure Java).

```bash
# run the test suite
./gradlew :control:test

# build the library
./gradlew :control:assembleRelease
```

See the playground activity for more detailed usage information.
