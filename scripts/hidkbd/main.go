// A keyboard the phone believes is real.
//
// Registers a USB HID keyboard on the device over uhid and drives it from HTTP,
// so a script can hold a chord, toggle Num Lock and read the lock state back.
// `adb shell input keyevent` cannot do any of that: it synthesises an event
// with an empty metaState from a virtual device, so nothing that depends on a
// lock, on a modifier being held, or on a key's own layout is reachable through
// it. This is a real HID device as far as Android's input stack is concerned —
// it has a KeyCharacterMap, it tracks its own lock state, and its left and
// right modifiers are different keys.
//
// The lock state comes back for free: Android sets a keyboard's LEDs, and a
// uhid output report arrives here as JSON on the hid command's stdout. So
// "is Num Lock on" is answered by the phone rather than assumed.
//
//	go run . &                              # register, serve on :8099
//	curl -s 'localhost:8099/tap?k=NUMLOCK'
//	curl -s localhost:8099/leds             # num=1 caps=0 scroll=0
//	curl -s 'localhost:8099/tap?k=LCTRL,LSHIFT,ESCAPE'
//
// The device stays until this process exits, and goes away when it does.
package main

import (
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/signal"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/pgaskin/go-adb/adb"
	"github.com/pgaskin/go-adb/adb/adbhost"
	adbexec "github.com/pgaskin/go-adb/adblib/adbexec/v2"
)

var (
	listen = flag.String("listen", "localhost:8099", "address to serve on")
	serial = flag.String("s", os.Getenv("ANDROID_SERIAL"), "device serial (default: the only one)")
	name   = flag.String("name", "rvnc-hid-keyboard", "the name the device reports")
	server = flag.String("adb", "", "ADB server address (default: localhost:5037)")
)

// Two collections, so the media keys are reachable: report 1 is the boot
// keyboard and report 2 is a consumer control. Everything is one array field of
// usages, and the keyboard's is 8 bits wide over the whole page rather than the
// boot descriptor's 101, since F13-F24, the international keys and the
// Japanese/Korean ones are the point of the exercise.
var descriptor = []byte{
	0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, // Generic Desktop / Keyboard / Application
	0x85, 0x01, //   Report ID 1
	0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, //   the eight modifiers, one bit each
	0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x08, 0x81, 0x02,
	0x95, 0x01, 0x75, 0x08, 0x81, 0x01, //   the byte the boot report reserves
	0x05, 0x08, 0x19, 0x01, 0x29, 0x05, //   LEDs, which is how the lock state comes back
	0x95, 0x05, 0x75, 0x01, 0x91, 0x02,
	0x95, 0x01, 0x75, 0x03, 0x91, 0x01,
	0x05, 0x07, 0x19, 0x00, 0x2A, 0xFF, 0x00, //   six usages, 0x00-0xFF
	0x15, 0x00, 0x26, 0xFF, 0x00,
	0x75, 0x08, 0x95, 0x06, 0x81, 0x00,
	0xC0,

	0x05, 0x0C, 0x09, 0x01, 0xA1, 0x01, // Consumer / Consumer Control / Application
	0x85, 0x02, //   Report ID 2
	0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x10, //   sixteen bits, one per key
	0x09, 0xB5, 0x09, 0xB6, 0x09, 0xB7, 0x09, 0xB8, //   next, prev, stop, eject
	0x09, 0xCD, 0x09, 0xE2, 0x09, 0xE9, 0x09, 0xEA, //   play/pause, mute, vol +/-
	0x09, 0x6F, 0x09, 0x70, //   brightness up, down
	0x0A, 0x23, 0x02, 0x0A, 0x24, 0x02, //   AC Home, AC Back
	0x0A, 0x25, 0x02, 0x0A, 0x21, 0x02, //   AC Forward, AC Search
	0x0A, 0x27, 0x02, 0x0A, 0x92, 0x01, //   AC Refresh, AL Calculator
	0x81, 0x02,
	0xC0,
}

// The consumer page as bit positions in report 2, in descriptor order.
//
// A bitmap of named usages rather than an array over the page, which is what it
// was first: the kernel has no key for a usage it does not recognise, so an
// array field over 0x000-0x29C becomes an absolute axis, and the device
// arrives at Android as a keyboard, a stylus and a joystick at once.
var consumer = map[string]uint{
	"SCANNEXT": 0, "SCANPREV": 1, "STOP": 2, "EJECT": 3,
	"PLAYPAUSE": 4, "MUTE": 5, "VOLUMEUP": 6, "VOLUMEDOWN": 7,
	"BRIGHTNESSUP": 8, "BRIGHTNESSDOWN": 9,
	"AC_HOME": 10, "AC_BACK": 11, "AC_FORWARD": 12, "AC_SEARCH": 13,
	"AC_REFRESH": 14, "AL_CALCULATOR": 15,
}

// The keyboard page (0x07). Named by what is printed on the key rather than by
// the HID spec's own wording, since these names are what a test script reads.
var keys = map[string]byte{
	"A": 0x04, "B": 0x05, "C": 0x06, "D": 0x07, "E": 0x08, "F": 0x09,
	"G": 0x0A, "H": 0x0B, "I": 0x0C, "J": 0x0D, "K": 0x0E, "L": 0x0F,
	"M": 0x10, "N": 0x11, "O": 0x12, "P": 0x13, "Q": 0x14, "R": 0x15,
	"S": 0x16, "T": 0x17, "U": 0x18, "V": 0x19, "W": 0x1A, "X": 0x1B,
	"Y": 0x1C, "Z": 0x1D,

	"1": 0x1E, "2": 0x1F, "3": 0x20, "4": 0x21, "5": 0x22,
	"6": 0x23, "7": 0x24, "8": 0x25, "9": 0x26, "0": 0x27,

	"ENTER": 0x28, "ESCAPE": 0x29, "BACKSPACE": 0x2A, "TAB": 0x2B, "SPACE": 0x2C,
	"MINUS": 0x2D, "EQUAL": 0x2E, "LEFTBRACE": 0x2F, "RIGHTBRACE": 0x30,
	"BACKSLASH": 0x31, "NONUS_HASH": 0x32, "SEMICOLON": 0x33, "APOSTROPHE": 0x34,
	"GRAVE": 0x35, "COMMA": 0x36, "DOT": 0x37, "SLASH": 0x38, "CAPSLOCK": 0x39,

	"F1": 0x3A, "F2": 0x3B, "F3": 0x3C, "F4": 0x3D, "F5": 0x3E, "F6": 0x3F,
	"F7": 0x40, "F8": 0x41, "F9": 0x42, "F10": 0x43, "F11": 0x44, "F12": 0x45,

	"PRINTSCREEN": 0x46, "SCROLLLOCK": 0x47, "PAUSE": 0x48, "INSERT": 0x49,
	"HOME": 0x4A, "PAGEUP": 0x4B, "DELETE": 0x4C, "END": 0x4D, "PAGEDOWN": 0x4E,
	"RIGHT": 0x4F, "LEFT": 0x50, "DOWN": 0x51, "UP": 0x52,

	"NUMLOCK": 0x53, "KP_SLASH": 0x54, "KP_ASTERISK": 0x55, "KP_MINUS": 0x56,
	"KP_PLUS": 0x57, "KP_ENTER": 0x58,
	"KP_1": 0x59, "KP_2": 0x5A, "KP_3": 0x5B, "KP_4": 0x5C, "KP_5": 0x5D,
	"KP_6": 0x5E, "KP_7": 0x5F, "KP_8": 0x60, "KP_9": 0x61, "KP_0": 0x62,
	"KP_DOT": 0x63,

	"NONUS_BACKSLASH": 0x64, "APPLICATION": 0x65, "POWER": 0x66, "KP_EQUAL": 0x67,

	"F13": 0x68, "F14": 0x69, "F15": 0x6A, "F16": 0x6B, "F17": 0x6C, "F18": 0x6D,
	"F19": 0x6E, "F20": 0x6F, "F21": 0x70, "F22": 0x71, "F23": 0x72, "F24": 0x73,

	"EXECUTE": 0x74, "HELP": 0x75, "MENU": 0x76, "SELECT": 0x77, "STOP": 0x78,
	"AGAIN": 0x79, "UNDO": 0x7A, "CUT": 0x7B, "COPY": 0x7C, "PASTE": 0x7D,
	"FIND": 0x7E, "MUTE": 0x7F, "VOLUMEUP": 0x80, "VOLUMEDOWN": 0x81,

	"KP_COMMA": 0x85,
	// The keys a US keyboard does not have, which are where a layout stops
	// being a detail: RO and YEN on a Japanese board, HANGUL on a Korean one.
	"INTL_RO": 0x87, "INTL_KANA": 0x88, "INTL_YEN": 0x89,
	"INTL_HENKAN": 0x8A, "INTL_MUHENKAN": 0x8B,
	"LANG_HANGUL": 0x90, "LANG_HANJA": 0x91,
	"LANG_KATAKANA": 0x92, "LANG_HIRAGANA": 0x93,

	"LCTRL": 0xE0, "LSHIFT": 0xE1, "LALT": 0xE2, "LGUI": 0xE3,
	"RCTRL": 0xE4, "RSHIFT": 0xE5, "RALT": 0xE6, "RGUI": 0xE7,
}

// What somebody types rather than what the page calls it.
var aliases = map[string]string{
	"CTRL": "LCTRL", "SHIFT": "LSHIFT", "ALT": "LALT",
	"GUI": "LGUI", "META": "LGUI", "SUPER": "LGUI", "WIN": "LGUI",
	"ALTGR": "RALT", "ESC": "ESCAPE", "RET": "ENTER", "RETURN": "ENTER",
	"DEL": "DELETE", "INS": "INSERT", "PGUP": "PAGEUP", "PGDN": "PAGEDOWN",
	"PRTSCR": "PRINTSCREEN", "SYSRQ": "PRINTSCREEN", "BREAK": "PAUSE",
	"PERIOD": "DOT", "KP_PERIOD": "KP_DOT", "KP_STAR": "KP_ASTERISK",
	"COMPOSE": "APPLICATION", "CONTEXT": "APPLICATION",
}

// A phone carries ~170 per-device key layouts and key character maps, matched
// on vendor and product, and one of them decides whether this is a keyboard at
// all: Android reads the layout, asks it for the scancode behind `Q` and marks
// the device alphabetic only if it has one. 18d1:4f80 — Google's own vendor id
// and a plausible product — is `Vendor_18d1_Product_4f80.kl`, "Android Stylus",
// under which the keyboard arrives as a non-alphabetic stylus, Caps Lock and
// Num Lock are never tracked and the LEDs are never written. So the pair has to
// be one nothing claims. pid.codes' 1209 is not allocated to anything on a
// phone, which leaves Generic.kl and Generic.kcm, which is a US PC keyboard.
const (
	vendorID  = 0x1209
	productID = 0xB1D1
)

const (
	ledNumLock    = 0x01
	ledCapsLock   = 0x02
	ledScrollLock = 0x04
)

// One HID keyboard, and everything about it that is not the device itself.
//
// The held set is ordered because a report is: the modifier bits, then up to
// six usages. More than six non-modifier keys at once is a rollover the boot
// report cannot express, and is refused rather than silently truncated.
type keyboard struct {
	mu    sync.Mutex
	out   io.Writer
	held  []byte
	leds  byte
	ledAt time.Time
}

func (k *keyboard) event(v any) error {
	b, err := json.Marshal(v)
	if err != nil {
		return err
	}
	k.mu.Lock()
	defer k.mu.Unlock()
	_, err = k.out.Write(append(b, '\n'))
	return err
}

// The current held set as a report, sent as it stands.
func (k *keyboard) send() error {
	k.mu.Lock()
	report := []int{1, 0, 0, 0, 0, 0, 0, 0, 0}
	n := 3
	for _, u := range k.held {
		if u >= 0xE0 && u <= 0xE7 {
			report[1] |= 1 << (u - 0xE0)
			continue
		}
		if n >= len(report) {
			k.mu.Unlock()
			return errors.New("more than six keys held at once")
		}
		report[n] = int(u)
		n++
	}
	k.mu.Unlock()
	return k.event(map[string]any{"id": 1, "command": "report", "report": report})
}

func (k *keyboard) press(usages []byte) error {
	k.mu.Lock()
	for _, u := range usages {
		if !contains(k.held, u) {
			k.held = append(k.held, u)
		}
	}
	k.mu.Unlock()
	return k.send()
}

func (k *keyboard) release(usages []byte) error {
	k.mu.Lock()
	kept := k.held[:0]
	for _, h := range k.held {
		if usages == nil || !contains(usages, h) {
			kept = append(kept, h)
		}
	}
	k.held = kept
	k.mu.Unlock()
	return k.send()
}

// A chord: pressed in the order given and let go in the reverse, which is what
// a person does and what a modifier needs.
func (k *keyboard) tap(usages []byte, hold time.Duration) error {
	for _, u := range usages {
		if err := k.press([]byte{u}); err != nil {
			return err
		}
	}
	time.Sleep(hold)
	for i := len(usages) - 1; i >= 0; i-- {
		if err := k.release([]byte{usages[i]}); err != nil {
			return err
		}
	}
	return nil
}

func (k *keyboard) consumer(bit uint, hold time.Duration) error {
	set := uint16(1) << bit
	down := map[string]any{"id": 1, "command": "report",
		"report": []int{2, int(set & 0xFF), int(set >> 8)}}
	up := map[string]any{"id": 1, "command": "report", "report": []int{2, 0, 0}}
	if err := k.event(down); err != nil {
		return err
	}
	time.Sleep(hold)
	return k.event(up)
}

func contains(s []byte, v byte) bool {
	for _, x := range s {
		if x == v {
			return true
		}
	}
	return false
}

// A comma-separated key list as usages. Case-insensitive, since a script that
// says `ctrl,c` means the same thing as one that says `LCTRL,C`.
func parseKeys(s string) ([]byte, error) {
	var out []byte
	for _, part := range strings.Split(s, ",") {
		n := strings.ToUpper(strings.TrimSpace(part))
		if n == "" {
			continue
		}
		if a, ok := aliases[n]; ok {
			n = a
		}
		u, ok := keys[n]
		if !ok {
			// A raw usage as well as a name, so a key this table has never
			// heard of is still reachable.
			if v, err := strconv.ParseUint(n, 0, 8); err == nil {
				out = append(out, byte(v))
				continue
			}
			return nil, fmt.Errorf("unknown key %q", part)
		}
		out = append(out, u)
	}
	if len(out) == 0 {
		return nil, errors.New("no keys given")
	}
	return out, nil
}

// The hid command reports what the host wrote to the device — the LED report is
// the one that matters here — as JSON objects on its stdout.
func (k *keyboard) readOutputs(r io.Reader, verbose bool) {
	dec := json.NewDecoder(r)
	for {
		var ev struct {
			EventID    int   `json:"eventId"`
			DeviceID   int   `json:"deviceId"`
			ReportType int   `json:"reportType"`
			ReportData []int `json:"reportData"`
		}
		if err := dec.Decode(&ev); err != nil {
			if err != io.EOF {
				fmt.Fprintln(os.Stderr, "hid output:", err)
			}
			return
		}
		// With report IDs in the descriptor the first byte is the id, but a
		// SET_REPORT arrives with the id stripped and delivered out of band.
		var led byte
		switch {
		case len(ev.ReportData) >= 2 && ev.ReportData[0] == 1:
			led = byte(ev.ReportData[1])
		case len(ev.ReportData) == 1:
			led = byte(ev.ReportData[0])
		default:
			continue
		}
		k.mu.Lock()
		k.leds, k.ledAt = led, time.Now()
		k.mu.Unlock()
		if verbose {
			fmt.Fprintf(os.Stderr, "leds %s\n", ledString(led))
		}
	}
}

func ledString(b byte) string {
	on := func(bit byte) int {
		if b&bit != 0 {
			return 1
		}
		return 0
	}
	return fmt.Sprintf("num=%d caps=%d scroll=%d raw=%#02x",
		on(ledNumLock), on(ledCapsLock), on(ledScrollLock), b)
}

func main() {
	verbose := flag.Bool("v", false, "log every LED report")
	flag.Parse()

	dialer := &adbhost.Dialer{Addr: *server}
	var transport adbhost.Transport = adbhost.TransportAny
	if *serial != "" {
		transport = adbhost.Serial(*serial)
	}
	dev := adbhost.Server(dialer, transport)

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	// Shell v2 is what keeps stdin open and stdout unmangled. Both feature
	// lists have to be asked for: what a transport supports is intersected
	// with what the host server does, and an unloaded list is an empty one.
	if err := dialer.LoadFeatures(ctx); err != nil {
		fatal(err)
	}
	if err := dev.LoadFeatures(ctx); err != nil {
		fatal(err)
	}

	// `hid -` holds the uhid device open for as long as its stdin is: the
	// device exists exactly as long as this process does.
	cmd := adbexec.CommandContext(ctx, dev, "hid", "-")
	stdin, err := cmd.StdinPipe()
	if err != nil {
		fatal(err)
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		fatal(err)
	}
	cmd.Stderr = os.Stderr
	if err := cmd.Start(); err != nil {
		fatal(fmt.Errorf("start hid on the device: %w", err))
	}

	kb := &keyboard{out: stdin}
	go kb.readOutputs(stdout, *verbose)

	desc := make([]int, len(descriptor))
	for i, b := range descriptor {
		desc[i] = int(b)
	}
	if err := kb.event(map[string]any{
		"id": 1, "command": "register", "name": *name,
		"vid": vendorID, "pid": productID, "bus": "usb", "descriptor": desc,
	}); err != nil {
		fatal(err)
	}
	if err := awaitDevice(ctx, dev, *name); err != nil {
		fatal(err)
	}

	http.HandleFunc("/press", handle(kb, func(k *keyboard, r *http.Request) error {
		u, err := parseKeys(r.FormValue("k"))
		if err != nil {
			return err
		}
		return k.press(u)
	}))
	http.HandleFunc("/release", handle(kb, func(k *keyboard, r *http.Request) error {
		if r.FormValue("k") == "" {
			return k.release(nil) // everything, which is how a run starts clean
		}
		u, err := parseKeys(r.FormValue("k"))
		if err != nil {
			return err
		}
		return k.release(u)
	}))
	http.HandleFunc("/tap", handle(kb, func(k *keyboard, r *http.Request) error {
		u, err := parseKeys(r.FormValue("k"))
		if err != nil {
			return err
		}
		return k.tap(u, holdOf(r))
	}))
	http.HandleFunc("/consumer", handle(kb, func(k *keyboard, r *http.Request) error {
		n := strings.ToUpper(strings.TrimSpace(r.FormValue("k")))
		u, ok := consumer[n]
		if !ok {
			return fmt.Errorf("unknown consumer key %q", n)
		}
		return k.consumer(u, holdOf(r))
	}))
	// The escape hatch: a report this tool has no vocabulary for.
	http.HandleFunc("/report", handle(kb, func(k *keyboard, r *http.Request) error {
		var report []int
		for _, f := range strings.Split(r.FormValue("b"), ",") {
			v, err := strconv.ParseUint(strings.TrimSpace(f), 0, 8)
			if err != nil {
				return err
			}
			report = append(report, int(v))
		}
		return k.event(map[string]any{"id": 1, "command": "report", "report": report})
	}))
	http.HandleFunc("/leds", func(w http.ResponseWriter, r *http.Request) {
		kb.mu.Lock()
		leds, at := kb.leds, kb.ledAt
		kb.mu.Unlock()
		if at.IsZero() {
			// Nothing has set them, which is not the same as all off: a phone
			// that has never touched this keyboard's LEDs says nothing at all.
			fmt.Fprintf(w, "%s unset\n", ledString(leds))
			return
		}
		fmt.Fprintf(w, "%s\n", ledString(leds))
	})
	http.HandleFunc("/state", func(w http.ResponseWriter, r *http.Request) {
		kb.mu.Lock()
		held, leds := append([]byte(nil), kb.held...), kb.leds
		kb.mu.Unlock()
		var names []string
		for _, u := range held {
			names = append(names, nameOf(u))
		}
		fmt.Fprintf(w, "held=%s\n%s\n", strings.Join(names, ","), ledString(leds))
	})
	http.HandleFunc("/keys", func(w http.ResponseWriter, r *http.Request) {
		for _, n := range sortedNames() {
			fmt.Fprintf(w, "%-16s %#02x\n", n, keys[n])
		}
	})
	http.HandleFunc("/quit", func(w http.ResponseWriter, r *http.Request) {
		fmt.Fprintln(w, "bye")
		stop()
	})

	srv := &http.Server{Addr: *listen}
	go func() {
		<-ctx.Done()
		srv.Close()
		stdin.Close()
	}()
	fmt.Fprintf(os.Stderr, "%s registered; serving on %s\n", *name, *listen)
	if err := srv.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
		fatal(err)
	}
	cmd.Wait()
}

func holdOf(r *http.Request) time.Duration {
	if v := r.FormValue("hold"); v != "" {
		if d, err := time.ParseDuration(v); err == nil {
			return d
		}
	}
	return 40 * time.Millisecond
}

func handle(k *keyboard, fn func(*keyboard, *http.Request) error) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if err := fn(k, r); err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}
		fmt.Fprintln(w, "ok")
	}
}

func nameOf(u byte) string {
	for n, v := range keys {
		if v == u {
			return n
		}
	}
	return fmt.Sprintf("%#02x", u)
}

func sortedNames() []string {
	names := make([]string, 0, len(keys))
	for n := range keys {
		names = append(names, n)
	}
	for i := 1; i < len(names); i++ {
		for j := i; j > 0 && keys[names[j]] < keys[names[j-1]]; j-- {
			names[j], names[j-1] = names[j-1], names[j]
		}
	}
	return names
}

// Registration is asynchronous and silent: uhid takes the descriptor, the
// kernel probes it and Android's input reader picks it up some time later. A
// tool that returns before that has handed the caller a keyboard whose first
// few keys go nowhere.
func awaitDevice(ctx context.Context, dev adb.Dialer, name string) error {
	deadline := time.Now().Add(15 * time.Second)
	for {
		out, err := adbexec.Command(dev, "dumpsys", "input").Output()
		if err == nil && strings.Contains(string(out), name) {
			for _, line := range strings.Split(string(out), "\n") {
				if strings.Contains(line, name) && strings.Contains(line, "Device") {
					fmt.Fprintln(os.Stderr, strings.TrimSpace(line))
					return nil
				}
			}
			return nil
		}
		if time.Now().After(deadline) {
			return errors.New("the device never appeared in dumpsys input")
		}
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-time.After(250 * time.Millisecond):
		}
	}
}

func fatal(err error) {
	fmt.Fprintln(os.Stderr, "error:", err)
	os.Exit(1)
}
