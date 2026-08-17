//! `spice-vdagent`: the half of a SPICE session that runs inside the machine.
//!
//! Nothing in here is SPICE's own. The main channel carries opaque bytes
//! between this client and a program in the guest, and what those bytes mean is
//! the agent's protocol (`vd_agent.h`) — so the clipboard, the resize and the
//! monitor layout are answered by a guest that has the agent installed and by
//! nothing else. That is why every capability here is conditional on something
//! the far end announced rather than on the protocol version.
//!
//! Three things about the framing that the code below would otherwise not
//! explain:
//!
//! * A message is split and the pieces are the **flow control**. Each one costs
//!   a token the server grants, and a message bigger than a piece is split
//!   across several with no boundary of its own on the wire — so what arrives
//!   has to be reassembled out of a running buffer rather than parsed per
//!   message. The `VDIChunkHeader` in the agent's own header file is *not* part
//!   of this: it is how the server frames the guest's virtio port, which it
//!   writes on the way in and strips on the way out. A client that sends one is
//!   sending a header where a message should start, and what the server does
//!   about it is drop the agent and announce it again — for ever, with nothing
//!   said about why.
//! * The clipboard messages have **two wire forms**, with and without a leading
//!   selection byte, and which one is in use is decided by both ends
//!   announcing `CLIPBOARD_SELECTION`. This end announces it, so a guest that
//!   does too separates what a person copied from what they merely selected —
//!   and only the first of those is a clipboard on a phone.
//! * A grab is an **announcement, not the text**. The seam above takes text, so
//!   this end asks for it as soon as the guest says it has some, where a
//!   desktop client would wait until somebody pressed paste.

use std::collections::VecDeque;

/// `VD_AGENT_PROTOCOL`, which has been 1 for the life of the agent.
const PROTOCOL: u32 = 1;
/// `VD_AGENT_MAX_DATA_SIZE`: one message on the wire, and one token.
const MAX_PIECE: usize = 2048;
/// `VDAgentMessage`: protocol, type, opaque, size.
const HEADER: usize = 20;

/// What a message is. The ones this client neither sends nor reads — the file
/// transfer, the audio volume, the display config — are not here.
mod message {
    pub const MONITORS_CONFIG: u32 = 2;
    pub const REPLY: u32 = 3;
    pub const CLIPBOARD: u32 = 4;
    pub const ANNOUNCE_CAPABILITIES: u32 = 6;
    pub const CLIPBOARD_GRAB: u32 = 7;
    pub const CLIPBOARD_REQUEST: u32 = 8;
    pub const CLIPBOARD_RELEASE: u32 = 9;
}

/// Bit numbers in the capability mask, of which this end announces five.
mod cap {
    pub const MONITORS_CONFIG: u32 = 1;
    pub const REPLY: u32 = 2;
    pub const CLIPBOARD_BY_DEMAND: u32 = 5;
    pub const CLIPBOARD_SELECTION: u32 = 6;
    pub const GUEST_LINEEND_LF: u32 = 8;
}

/// The clipboard type this client has: a phone's clipboard is text.
const UTF8_TEXT: u32 = 1;
/// `VD_AGENT_CLIPBOARD_SELECTION_CLIPBOARD`, which is what a person copied —
/// as against `PRIMARY`, which on X11 is anything they dragged a mouse over.
const SELECTION_CLIPBOARD: u8 = 0;

/// A text bigger than this is not sent: the guest's clipboard is not where a
/// phone puts a file, and every chunk of it costs a token the session shares
/// with everything else on the main channel.
const MAX_TEXT: usize = 512 * 1024;

/// What the session has to act on. Everything else the agent says is answered
/// in here.
pub enum Event {
    /// The guest's clipboard, which this end asked for when the guest said it
    /// had something.
    Clipboard(String),
    /// What the far end can do has changed: the capabilities arrived, or the
    /// agent went away with them.
    Capabilities,
}

/// The agent as this client sees it: what it can do, what it is owed, and what
/// is half-said in either direction.
pub struct Agent {
    connected: bool,
    /// How many chunks this end may still send. The server grants them and
    /// takes none back; a client that sends without one is a client the server
    /// disconnects.
    tokens: u32,
    /// The guest's capability mask, which is what decides whether there is a
    /// resize or a clipboard at all.
    caps: u32,
    /// Bytes of agent message that have arrived and not yet made a whole one.
    inbound: Vec<u8>,
    /// Pieces waiting for a token.
    outbound: VecDeque<Vec<u8>>,
    /// The text this end has offered the guest, kept because a grab says only
    /// that there is some and the guest asks for it afterwards.
    ours: Option<String>,
    /// Whether the guest has been told about it. The app pushes its clipboard
    /// the moment a session connects, which is before the agent has said what
    /// it can do, so an offer waits for the capabilities rather than being
    /// dropped for arriving a few hundred milliseconds early.
    offered: bool,
}

impl Agent {
    pub fn new() -> Agent {
        Agent {
            connected: false,
            tokens: 0,
            caps: 0,
            inbound: Vec::new(),
            outbound: VecDeque::new(),
            ours: None,
            offered: false,
        }
    }

    /// The agent came up, with the tokens the server grants for what this end
    /// sends. Its capabilities are asked for here and arrive later.
    pub fn connected(&mut self, tokens: u32) {
        self.connected = true;
        self.tokens = tokens;
        self.caps = 0;
        self.inbound.clear();
        self.outbound.clear();
        self.announce(true);
    }

    /// The agent went away — a guest logging out, or the program being killed.
    /// Anything half-received with it is discarded rather than kept for the
    /// next one, which is a different agent with different capabilities.
    pub fn disconnected(&mut self) {
        self.connected = false;
        self.caps = 0;
        self.inbound.clear();
        self.outbound.clear();
        self.ours = None;
        self.offered = false;
    }

    pub fn is_connected(&self) -> bool {
        self.connected
    }

    pub fn add_tokens(&mut self, tokens: u32) {
        self.tokens = self.tokens.saturating_add(tokens);
    }

    /// Whether the guest will take a desktop size, which is
    /// `Backend.canResize`'s answer for this protocol.
    pub fn can_resize(&self) -> bool {
        self.connected && self.has(cap::MONITORS_CONFIG)
    }

    pub fn has_clipboard(&self) -> bool {
        self.connected && self.has(cap::CLIPBOARD_BY_DEMAND)
    }

    /// What to put in the next `MSG_MAIN_AGENT_DATA`, if there is something and
    /// a token to spend on it.
    pub fn next_piece(&mut self) -> Option<Vec<u8>> {
        if self.tokens == 0 {
            return None;
        }
        let piece = self.outbound.pop_front()?;
        self.tokens -= 1;
        Some(piece)
    }

    /// What this phone has on its clipboard, offered to the guest. The text
    /// itself goes over when the guest asks for it.
    pub fn offer_clipboard(&mut self, text: String) {
        if text.is_empty() || text.len() > MAX_TEXT {
            return;
        }
        if self.offered && self.ours.as_deref() == Some(text.as_str()) {
            // A grab the guest already has. Announcing the same text again
            // makes the guest drop its selection and take ours a second time,
            // which over there is a clipboard manager with two of everything in
            // it.
            return;
        }
        self.ours = Some(text);
        self.offered = false;
        self.grab();
    }

    /// Ask the guest to make its desktop this many monitors of these sizes.
    ///
    /// A request and not a setting: the agent hands it to whatever the guest
    /// uses for displays, and what comes back is a new mode, a different one,
    /// or nothing at all.
    pub fn request_monitors(&mut self, monitors: &[(i32, i32, u32, u32)]) {
        if !self.can_resize() || monitors.is_empty() {
            return;
        }
        let mut body = Vec::new();
        body.extend_from_slice(&(monitors.len() as u32).to_le_bytes());
        // `USE_POS`, because a layout of more than one head is only a layout if
        // it says where they are; for a single monitor at the origin it makes
        // no difference and is sent anyway rather than conditionally.
        body.extend_from_slice(&1u32.to_le_bytes());
        for (x, y, width, height) in monitors {
            body.extend_from_slice(&height.to_le_bytes());
            body.extend_from_slice(&width.to_le_bytes());
            body.extend_from_slice(&32u32.to_le_bytes()); // depth, which QXL has one of
            body.extend_from_slice(&(*x as u32).to_le_bytes());
            body.extend_from_slice(&(*y as u32).to_le_bytes());
        }
        self.queue(message::MONITORS_CONFIG, &body);
    }

    /// Bytes out of one `MSG_MAIN_AGENT_DATA`, which is a piece of a message
    /// and not a message: what this returns is whatever whole ones completed.
    pub fn receive(&mut self, body: &[u8]) -> Vec<Event> {
        let mut events = Vec::new();
        self.inbound.extend_from_slice(body);

        while self.inbound.len() >= HEADER {
            let protocol = u32::from_le_bytes(self.inbound[0..4].try_into().unwrap());
            let message_type = u32::from_le_bytes(self.inbound[4..8].try_into().unwrap());
            let size = u32::from_le_bytes(self.inbound[16..20].try_into().unwrap()) as usize;
            if self.inbound.len() < HEADER + size {
                break;
            }
            if protocol != PROTOCOL {
                // Nothing can be done with a stream whose framing is not
                // understood: the length that would skip this message is inside
                // the part that is not understood.
                log::warn!("agent: protocol {protocol} is not this one");
                self.inbound.clear();
                break;
            }
            let payload: Vec<u8> = self.inbound[HEADER..HEADER + size].to_vec();
            self.inbound.drain(..HEADER + size);
            self.message(message_type, &payload, &mut events);
        }
        events
    }

    fn message(&mut self, message_type: u32, payload: &[u8], events: &mut Vec<Event>) {
        match message_type {
            message::ANNOUNCE_CAPABILITIES => {
                if payload.len() < 8 {
                    return;
                }
                let request = u32::from_le_bytes(payload[0..4].try_into().unwrap());
                // Only the first word of the mask is read: every capability
                // this client knows about is in it, and a guest announcing a
                // second word is announcing things it will not be asked for.
                self.caps = u32::from_le_bytes(payload[4..8].try_into().unwrap());
                log::info!(
                    "agent: capabilities {:#x} — clipboard {}, resize {}",
                    self.caps,
                    self.has_clipboard(),
                    self.can_resize()
                );
                if request != 0 {
                    self.announce(false);
                }
                // Whatever the app pushed before the guest had said it has a
                // clipboard, which is the ordinary case for a session that
                // connects with something already copied on the phone.
                self.grab();
                events.push(Event::Capabilities);
            }
            message::CLIPBOARD_GRAB => {
                // The guest has something. The types follow the selection where
                // there is one, and this end wants exactly one of them.
                let types = self.after_selection(payload);
                let wanted = types
                    .chunks_exact(4)
                    .map(|t| u32::from_le_bytes(t.try_into().unwrap()))
                    .any(|t| t == UTF8_TEXT);
                if !self.is_clipboard_selection(payload) || !wanted {
                    return;
                }
                // Theirs now: what this end had offered is no longer the
                // clipboard, and the same text copied here again is a new offer
                // rather than a repeat of one the guest has.
                self.ours = None;
                self.offered = false;
                let mut body = Vec::new();
                self.selection(&mut body);
                body.extend_from_slice(&UTF8_TEXT.to_le_bytes());
                self.queue(message::CLIPBOARD_REQUEST, &body);
            }
            message::CLIPBOARD => {
                if !self.is_clipboard_selection(payload) {
                    return;
                }
                let rest = self.after_selection(payload);
                if rest.len() < 4 {
                    return;
                }
                let kind = u32::from_le_bytes(rest[0..4].try_into().unwrap());
                if kind != UTF8_TEXT {
                    return;
                }
                let text = String::from_utf8_lossy(&rest[4..]).to_string();
                events.push(Event::Clipboard(text));
            }
            message::CLIPBOARD_REQUEST => {
                // The guest wants what this end offered. Whatever was last
                // grabbed is what goes: if it is gone, the request is answered
                // with a release rather than with silence, which is what stops
                // a paste over there from hanging.
                let mut body = Vec::new();
                self.selection(&mut body);
                match self.ours.clone() {
                    Some(text) => {
                        body.extend_from_slice(&UTF8_TEXT.to_le_bytes());
                        body.extend_from_slice(text.as_bytes());
                        self.queue(message::CLIPBOARD, &body);
                    }
                    None => self.queue(message::CLIPBOARD_RELEASE, &body),
                }
            }
            message::CLIPBOARD_RELEASE => {}
            message::REPLY => {
                // The guest's yes or no to a request, and the only one this
                // client makes is the resize.
                if payload.len() >= 8 {
                    let error = u32::from_le_bytes(payload[4..8].try_into().unwrap());
                    log::info!("agent: reply {}", if error == 0 { "ok" } else { "refused" });
                }
            }
            other => log::debug!("agent: message {other} is not handled here"),
        }
    }

    /// Tell the guest this end has text, if it can be told yet.
    fn grab(&mut self) {
        if self.offered || !self.has_clipboard() || self.ours.is_none() {
            return;
        }
        self.offered = true;
        let mut body = Vec::new();
        self.selection(&mut body);
        body.extend_from_slice(&UTF8_TEXT.to_le_bytes());
        self.queue(message::CLIPBOARD_GRAB, &body);
    }

    /// This end's capabilities, as a request when the agent has just come up
    /// and as an answer when it asks.
    fn announce(&mut self, request: bool) {
        let mask = (1 << cap::MONITORS_CONFIG)
            | (1 << cap::REPLY)
            | (1 << cap::CLIPBOARD_BY_DEMAND)
            | (1 << cap::CLIPBOARD_SELECTION)
            | (1 << cap::GUEST_LINEEND_LF);
        let mut body = Vec::new();
        body.extend_from_slice(&u32::from(request).to_le_bytes());
        body.extend_from_slice(&(mask as u32).to_le_bytes());
        self.queue(message::ANNOUNCE_CAPABILITIES, &body);
    }

    /// A message, split into the pieces a token is spent per.
    fn queue(&mut self, message_type: u32, body: &[u8]) {
        let mut whole = Vec::with_capacity(HEADER + body.len());
        whole.extend_from_slice(&PROTOCOL.to_le_bytes());
        whole.extend_from_slice(&message_type.to_le_bytes());
        whole.extend_from_slice(&0u64.to_le_bytes()); // opaque, which nothing here uses
        whole.extend_from_slice(&(body.len() as u32).to_le_bytes());
        whole.extend_from_slice(body);

        for piece in whole.chunks(MAX_PIECE) {
            self.outbound.push_back(piece.to_vec());
        }
    }

    fn has(&self, bit: u32) -> bool {
        self.caps & (1 << bit) != 0
    }

    /// Whether the clipboard messages in this session carry a selection byte,
    /// which they do when both ends said they could.
    fn selections(&self) -> bool {
        self.has(cap::CLIPBOARD_SELECTION)
    }

    fn selection(&self, body: &mut Vec<u8>) {
        if self.selections() {
            body.push(SELECTION_CLIPBOARD);
            body.extend_from_slice(&[0, 0, 0]); // reserved, and the guest checks it is there
        }
    }

    fn is_clipboard_selection(&self, payload: &[u8]) -> bool {
        // Without the capability there is one selection and it is the
        // clipboard; with it, PRIMARY is every drag of a mouse over text and is
        // not what a phone means by copy.
        !self.selections() || payload.first() == Some(&SELECTION_CLIPBOARD)
    }

    fn after_selection<'a>(&self, payload: &'a [u8]) -> &'a [u8] {
        match self.selections() {
            true => payload.get(4..).unwrap_or_default(),
            false => payload,
        }
    }
}

impl Default for Agent {
    fn default() -> Agent {
        Agent::new()
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    /// One agent message, as the wire carries it: a header and a body, and no
    /// framing of its own.
    fn agent_message(message_type: u32, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&PROTOCOL.to_le_bytes());
        out.extend_from_slice(&message_type.to_le_bytes());
        out.extend_from_slice(&0u64.to_le_bytes());
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(body);
        out
    }

    fn caps(mask: u32) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&mask.to_le_bytes());
        agent_message(message::ANNOUNCE_CAPABILITIES, &body)
    }

    /// What message a piece is, which is all any of these assert about the
    /// bytes going out.
    fn kind_of(piece: &[u8]) -> u32 {
        u32::from_le_bytes(piece[4..8].try_into().unwrap())
    }

    /// An agent that has come up, announced everything, and had its own
    /// messages drained.
    fn connected() -> Agent {
        let mut agent = Agent::new();
        agent.connected(10);
        while agent.next_piece().is_some() {}
        agent.receive(&caps(
            (1 << cap::MONITORS_CONFIG)
                | (1 << cap::CLIPBOARD_BY_DEMAND)
                | (1 << cap::CLIPBOARD_SELECTION),
        ));
        while agent.next_piece().is_some() {}
        agent
    }

    #[test]
    fn what_the_guest_can_do_is_what_it_announced() {
        let mut agent = Agent::new();
        assert!(!agent.can_resize(), "nothing before it is connected");
        agent.connected(10);
        assert!(!agent.can_resize(), "nor before it has said what it can do");
        agent.receive(&caps(1 << cap::MONITORS_CONFIG));
        assert!(agent.can_resize());
        assert!(!agent.has_clipboard(), "which it did not announce");
        agent.disconnected();
        assert!(!agent.can_resize(), "and it goes away with the agent");
    }

    #[test]
    fn what_goes_out_is_a_message_and_not_a_chunk() {
        // The framing bug this cost a session to find: a `VDIChunkHeader` is
        // the server's business with the guest's virtio port, and a client that
        // writes one has its agent dropped and re-announced for ever.
        let mut agent = Agent::new();
        agent.connected(10);
        let piece = agent.next_piece().expect("the capabilities announce");
        assert_eq!(
            u32::from_le_bytes(piece[0..4].try_into().unwrap()),
            PROTOCOL,
            "the first word is the protocol version, not a port"
        );
        assert_eq!(kind_of(&piece), message::ANNOUNCE_CAPABILITIES);
        assert_eq!(
            u32::from_le_bytes(piece[16..20].try_into().unwrap()) as usize,
            piece.len() - HEADER
        );
    }

    #[test]
    fn a_message_split_across_pieces_is_one_message() {
        let mut agent = connected();
        let text = "x".repeat(5000);
        let mut body = vec![SELECTION_CLIPBOARD, 0, 0, 0];
        body.extend_from_slice(&UTF8_TEXT.to_le_bytes());
        body.extend_from_slice(text.as_bytes());
        let whole = agent_message(message::CLIPBOARD, &body);

        let mut events = Vec::new();
        for piece in whole.chunks(MAX_PIECE) {
            events.extend(agent.receive(piece));
        }
        assert_eq!(events.len(), 1, "one message out of three pieces");
        match &events[0] {
            Event::Clipboard(got) => assert_eq!(got.len(), text.len()),
            _ => panic!("not the clipboard"),
        }
    }

    #[test]
    fn a_grab_is_answered_with_a_request_and_the_text_with_an_event() {
        let mut agent = connected();
        let mut grab = vec![SELECTION_CLIPBOARD, 0, 0, 0];
        grab.extend_from_slice(&UTF8_TEXT.to_le_bytes());
        let events = agent.receive(&agent_message(message::CLIPBOARD_GRAB, &grab));
        assert!(events.is_empty(), "a grab is not text");
        let out = agent.next_piece().expect("a request goes out for it");
        assert_eq!(kind_of(&out), message::CLIPBOARD_REQUEST);
    }

    #[test]
    fn the_primary_selection_is_not_a_clipboard() {
        let mut agent = connected();
        let mut grab = vec![1, 0, 0, 0]; // PRIMARY: anything a mouse dragged over
        grab.extend_from_slice(&UTF8_TEXT.to_le_bytes());
        agent.receive(&agent_message(message::CLIPBOARD_GRAB, &grab));
        assert!(agent.next_piece().is_none(), "and nothing is asked for");
    }

    #[test]
    fn what_this_end_offers_goes_over_when_the_guest_asks_for_it() {
        let mut agent = connected();
        agent.offer_clipboard("hello".into());
        let grab = agent.next_piece().expect("the offer is a grab");
        assert_eq!(kind_of(&grab), message::CLIPBOARD_GRAB);
        agent.offer_clipboard("hello".into());
        assert!(agent.next_piece().is_none(), "and the same text is not re-offered");

        let mut request = vec![SELECTION_CLIPBOARD, 0, 0, 0];
        request.extend_from_slice(&UTF8_TEXT.to_le_bytes());
        agent.receive(&agent_message(message::CLIPBOARD_REQUEST, &request));
        let sent = agent.next_piece().expect("the text follows the request");
        assert_eq!(kind_of(&sent), message::CLIPBOARD);
        assert!(sent.ends_with(b"hello"));
    }

    /// The app pushes the phone's clipboard the moment a session connects,
    /// which is before the guest has said it has one.
    #[test]
    fn an_offer_made_too_early_waits_for_the_capabilities() {
        let mut agent = Agent::new();
        agent.connected(10);
        while agent.next_piece().is_some() {}
        agent.offer_clipboard("hello".into());
        assert!(agent.next_piece().is_none(), "nothing to offer it to yet");
        agent.receive(&caps(
            (1 << cap::CLIPBOARD_BY_DEMAND) | (1 << cap::CLIPBOARD_SELECTION),
        ));
        let grab = agent.next_piece().expect("and it goes when the guest says it can");
        assert_eq!(kind_of(&grab), message::CLIPBOARD_GRAB);
    }

    #[test]
    fn a_piece_costs_a_token_and_a_client_without_one_waits() {
        let mut agent = Agent::new();
        agent.connected(1);
        assert!(agent.next_piece().is_some(), "the announce, on the one token");
        agent.receive(&caps(
            (1 << cap::CLIPBOARD_BY_DEMAND) | (1 << cap::CLIPBOARD_SELECTION),
        ));
        agent.offer_clipboard("hello".into());
        assert!(agent.next_piece().is_none(), "and nothing more without another");
        agent.add_tokens(2);
        assert!(agent.next_piece().is_some());
    }

    #[test]
    fn a_size_is_a_monitor_the_guest_is_asked_to_have() {
        let mut agent = connected();
        agent.request_monitors(&[(0, 0, 1280, 800)]);
        let out = agent.next_piece().expect("a monitors config goes out");
        assert_eq!(kind_of(&out), message::MONITORS_CONFIG);
        let body = &out[HEADER..];
        assert_eq!(u32::from_le_bytes(body[0..4].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(body[8..12].try_into().unwrap()), 800, "height first");
        assert_eq!(u32::from_le_bytes(body[12..16].try_into().unwrap()), 1280);
    }
}
