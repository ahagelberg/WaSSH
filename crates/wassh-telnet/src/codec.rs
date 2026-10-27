//! Telnet negotiation.
//!
//! A byte-level filter that strips IAC commands out of the stream and answers option
//! requests: echo and suppress-go-ahead from the
//! server are accepted, NAWS (window size) and terminal type are offered, everything else
//! is refused.

/// Interpret As Command.
pub const IAC: u8 = 255;
/// Refuse to enable an option on the sender's side.
pub const DONT: u8 = 254;
/// Ask the peer to enable an option.
pub const DO: u8 = 253;
/// Refuse to enable an option on our side.
pub const WONT: u8 = 252;
/// Offer to enable an option.
pub const WILL: u8 = 251;
/// Subnegotiation start.
pub const SB: u8 = 250;
/// Go ahead.
pub const GA: u8 = 249;
/// Subnegotiation end.
pub const SE: u8 = 240;
/// No operation.
pub const NOP: u8 = 241;

/// Server-side echo.
pub const OPT_ECHO: u8 = 1;
/// Suppress go-ahead.
pub const OPT_SUPPRESS_GA: u8 = 3;
/// Terminal type.
pub const OPT_TERMINAL_TYPE: u8 = 24;
/// Negotiate window size.
pub const OPT_NAWS: u8 = 31;
/// Terminal-type subnegotiation: "what is your terminal type?".
pub const TTYPE_SEND: u8 = 1;
/// Terminal-type subnegotiation: "my terminal type is …".
pub const TTYPE_IS: u8 = 0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Data,
    Iac,
    Will,
    Wont,
    Do,
    Dont,
    Sub,
    SubIac,
}

/// Stateful IAC filter.
pub struct TelnetFilter {
    state: State,
    sub: Vec<u8>,
    replies: Vec<u8>,
    /// Whether the server asked for (and we agreed to send) window size updates.
    pub naws_enabled: bool,
    /// Terminal type reported to the server.
    pub term_type: String,
    /// Current window size in columns.
    pub cols: u16,
    /// Current window size in rows.
    pub rows: u16,
}

impl TelnetFilter {
    /// Create a filter for a terminal of the given size.
    pub fn new(term_type: impl Into<String>, cols: u16, rows: u16) -> Self {
        Self {
            state: State::Data,
            sub: Vec::new(),
            replies: Vec::new(),
            naws_enabled: false,
            term_type: term_type.into(),
            cols,
            rows,
        }
    }

    /// Bytes to send once connected, offering the options we support.
    pub fn start(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        for (command, option) in [
            (DO, OPT_SUPPRESS_GA),
            (WILL, OPT_SUPPRESS_GA),
            (DO, OPT_ECHO),
            (WILL, OPT_NAWS),
            (WILL, OPT_TERMINAL_TYPE),
        ] {
            out.extend_from_slice(&[IAC, command, option]);
        }
        out
    }

    /// Consume incoming bytes, returning the application data and queuing replies.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<u8> {
        let mut app = Vec::with_capacity(chunk.len());
        for &byte in chunk {
            self.step(byte, &mut app);
        }
        app
    }

    /// Take any negotiation bytes produced since the last call.
    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.replies)
    }

    /// Re-escape outgoing application data (doubling any `IAC` byte).
    pub fn escape(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len());
        for &byte in data {
            out.push(byte);
            if byte == IAC {
                out.push(IAC);
            }
        }
        out
    }

    /// Update the window size, returning a NAWS subnegotiation when enabled.
    pub fn resize(&mut self, cols: u16, rows: u16) -> Option<Vec<u8>> {
        if (cols, rows) == (self.cols, self.rows) {
            return None;
        }
        self.cols = cols;
        self.rows = rows;
        self.naws_enabled.then(|| self.naws_payload())
    }

    fn naws_payload(&self) -> Vec<u8> {
        let mut out = vec![IAC, SB, OPT_NAWS];
        out.extend(Self::escape(&[
            (self.cols >> 8) as u8,
            (self.cols & 0xff) as u8,
            (self.rows >> 8) as u8,
            (self.rows & 0xff) as u8,
        ]));
        out.extend_from_slice(&[IAC, SE]);
        out
    }

    fn send(&mut self, command: u8, option: u8) {
        self.replies.extend_from_slice(&[IAC, command, option]);
    }

    fn step(&mut self, byte: u8, app: &mut Vec<u8>) {
        match self.state {
            State::Data => {
                if byte == IAC {
                    self.state = State::Iac;
                } else {
                    app.push(byte);
                }
            }
            State::Iac => match byte {
                IAC => {
                    app.push(IAC);
                    self.state = State::Data;
                }
                WILL => self.state = State::Will,
                WONT => self.state = State::Wont,
                DO => self.state = State::Do,
                DONT => self.state = State::Dont,
                SB => {
                    self.sub.clear();
                    self.state = State::Sub;
                }
                NOP | GA => self.state = State::Data,
                _ => self.state = State::Data,
            },
            State::Will => {
                if byte == OPT_ECHO || byte == OPT_SUPPRESS_GA {
                    self.send(DO, byte);
                } else {
                    self.send(DONT, byte);
                }
                self.state = State::Data;
            }
            State::Wont => {
                if byte == OPT_NAWS {
                    self.naws_enabled = false;
                }
                self.state = State::Data;
            }
            State::Do => {
                match byte {
                    OPT_SUPPRESS_GA | OPT_TERMINAL_TYPE => self.send(WILL, byte),
                    OPT_NAWS => {
                        self.naws_enabled = true;
                        self.send(WILL, OPT_NAWS);
                        let payload = self.naws_payload();
                        self.replies.extend_from_slice(&payload);
                    }
                    other => self.send(WONT, other),
                }
                self.state = State::Data;
            }
            State::Dont => {
                if byte == OPT_NAWS {
                    self.naws_enabled = false;
                }
                self.state = State::Data;
            }
            State::Sub => {
                if byte == IAC {
                    self.state = State::SubIac;
                } else {
                    self.sub.push(byte);
                }
            }
            State::SubIac => {
                if byte == IAC {
                    self.sub.push(IAC);
                    self.state = State::Sub;
                } else if byte == SE {
                    self.on_subnegotiation();
                    self.sub.clear();
                    self.state = State::Data;
                } else {
                    self.state = State::Sub;
                }
            }
        }
    }

    fn on_subnegotiation(&mut self) {
        if self.sub.len() >= 2
            && self.sub[0] == OPT_TERMINAL_TYPE
            && self.sub[1] == TTYPE_SEND
        {
            let mut reply = vec![IAC, SB, OPT_TERMINAL_TYPE, TTYPE_IS];
            reply.extend(self.term_type.bytes());
            reply.extend_from_slice(&[IAC, SE]);
            self.replies.extend_from_slice(&reply);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_passes_through() {
        let mut filter = TelnetFilter::new("xterm-256color", 80, 24);
        assert_eq!(filter.feed(b"hello"), b"hello".to_vec());
        assert!(filter.take_replies().is_empty());
    }

    #[test]
    fn escaped_iac_becomes_a_literal_byte() {
        let mut filter = TelnetFilter::new("xterm-256color", 80, 24);
        assert_eq!(filter.feed(&[IAC, IAC]), vec![IAC]);
    }

    #[test]
    fn server_echo_and_suppress_go_ahead_are_accepted() {
        let mut filter = TelnetFilter::new("xterm-256color", 80, 24);
        filter.feed(&[IAC, WILL, OPT_ECHO, IAC, WILL, OPT_SUPPRESS_GA]);
        assert_eq!(
            filter.take_replies(),
            vec![IAC, DO, OPT_ECHO, IAC, DO, OPT_SUPPRESS_GA]
        );
    }

    #[test]
    fn unknown_server_options_are_refused() {
        let mut filter = TelnetFilter::new("xterm-256color", 80, 24);
        filter.feed(&[IAC, WILL, 99]);
        assert_eq!(filter.take_replies(), vec![IAC, DONT, 99]);
    }

    #[test]
    fn naws_is_offered_and_sent_when_asked() {
        let mut filter = TelnetFilter::new("xterm-256color", 120, 40);
        filter.feed(&[IAC, DO, OPT_NAWS]);
        assert!(filter.naws_enabled);
        let replies = filter.take_replies();
        assert_eq!(&replies[..3], &[IAC, WILL, OPT_NAWS]);
        assert_eq!(
            &replies[3..],
            &[IAC, SB, OPT_NAWS, 0, 120, 0, 40, IAC, SE]
        );
    }

    #[test]
    fn resize_emits_naws_only_when_enabled() {
        // Before the server negotiates NAWS the size is recorded but nothing is sent.
        let mut filter = TelnetFilter::new("xterm-256color", 80, 24);
        assert!(filter.resize(100, 30).is_none());
        assert_eq!((filter.cols, filter.rows), (100, 30));

        filter.feed(&[IAC, DO, OPT_NAWS]);
        filter.take_replies();
        assert_eq!(
            filter.resize(132, 43).unwrap(),
            vec![IAC, SB, OPT_NAWS, 0, 132, 0, 43, IAC, SE]
        );
        assert!(filter.resize(132, 43).is_none());
    }

    #[test]
    fn terminal_type_is_reported() {
        let mut filter = TelnetFilter::new("xterm-256color", 80, 24);
        filter.feed(&[IAC, SB, OPT_TERMINAL_TYPE, TTYPE_SEND, IAC, SE]);
        let replies = filter.take_replies();
        assert_eq!(&replies[..4], &[IAC, SB, OPT_TERMINAL_TYPE, TTYPE_IS]);
        assert!(replies.ends_with(&[IAC, SE]));
        assert!(replies.windows(5).any(|window| window == b"xterm"));
    }

    #[test]
    fn outgoing_iac_bytes_are_escaped() {
        assert_eq!(TelnetFilter::escape(&[1, IAC, 2]), vec![1, IAC, IAC, 2]);
    }

    #[test]
    fn partial_sequences_survive_chunk_boundaries() {
        let mut filter = TelnetFilter::new("xterm-256color", 80, 24);
        assert!(filter.feed(&[IAC]).is_empty());
        assert!(filter.feed(&[WILL]).is_empty());
        assert!(filter.feed(&[OPT_ECHO]).is_empty());
        assert_eq!(filter.take_replies(), vec![IAC, DO, OPT_ECHO]);
        assert_eq!(filter.feed(b"ok"), b"ok".to_vec());
    }

    #[test]
    fn greeting_offers_the_documented_options() {
        let mut filter = TelnetFilter::new("xterm-256color", 80, 24);
        assert_eq!(
            filter.start(),
            vec![
                IAC, DO, OPT_SUPPRESS_GA,
                IAC, WILL, OPT_SUPPRESS_GA,
                IAC, DO, OPT_ECHO,
                IAC, WILL, OPT_NAWS,
                IAC, WILL, OPT_TERMINAL_TYPE,
            ]
        );
    }
}
