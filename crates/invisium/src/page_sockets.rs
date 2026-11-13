//! The WebSockets of a page's scripts.
//!
//! The page does not talk to the network. A script that makes a `WebSocket` leaves a request
//! here; the embedder opens the connection (`take_socket_requests`), hands back something to
//! send through (`socket_opened`) and reports what arrives (`socket_event`). Whether the
//! connection is allowed at all (mixed content, local addresses, blocking) is the network
//! stack's decision; a refused connection simply closes.

use super::Page;
use std::collections::HashMap;

/// Sockets one document may have open or opening at a time.
pub(crate) const MAX_SOCKETS: usize = 64;
/// With more than this waiting to be written on all of a document's sockets together, the
/// socket that asks for more is closed instead: a page cannot make the browser hold data
/// without bound.
const MAX_BUFFERED: u64 = 256 << 20;

/// A connection a script asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SocketRequest {
    /// Names the socket in `socket_opened` and `socket_event`.
    pub id: u32,
    /// Absolute `ws:` or `wss:` address.
    pub url: String,
    /// The subprotocols offered, in order.
    pub protocols: Vec<String>,
}

/// The sending side of an open (or opening) connection, as the embedder provides it.
pub trait PageSocket {
    fn send_text(&self, text: String);
    fn send_binary(&self, data: Vec<u8>);
    /// Start the closing handshake. `code` is 1000 or 3000..=4999 when the script gave one.
    fn close(&self, code: Option<u16>, reason: &str);
    /// Bytes taken by `send_*` and not yet written to the network.
    fn buffered(&self) -> u64;
}

/// What happened on a connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SocketEvent {
    /// The handshake succeeded; `protocol` is the subprotocol the server chose ("" if none).
    Open { protocol: String },
    Text(String),
    Binary(Vec<u8>),
    /// Always the last event of a socket. `clean` = the closing handshake completed.
    Closed { code: u16, reason: String, clean: bool },
}

pub(crate) struct Socket {
    handle: Option<Box<dyn PageSocket>>,
    /// The script closed it before the embedder had opened it.
    close: Option<(Option<u16>, String)>,
    /// It took too much to send and was closed for that.
    cut: bool,
}

impl Page {
    /// A script made a `WebSocket`. Returns its id, or 0 if the document has too many.
    pub(crate) fn socket_open(&mut self, url: String, protocols: Vec<String>) -> u32 {
        if self.sockets.len() >= MAX_SOCKETS {
            return 0;
        }
        self.next_socket = self.next_socket.wrapping_add(1).max(1);
        let id = self.next_socket;
        self.sockets.insert(id, Socket { handle: None, close: None, cut: false });
        self.socket_requests.push(SocketRequest { id, url, protocols });
        id
    }

    /// `send()` of a script. Text if `text` is given, the bytes otherwise.
    pub(crate) fn socket_send(&mut self, id: u32, text: Option<String>, data: Vec<u8>) {
        let waiting: u64 = self.sockets.values().filter_map(|s| s.handle.as_ref()).map(|h| h.buffered()).fold(0, u64::saturating_add);
        let Some(s) = self.sockets.get_mut(&id) else { return };
        let Some(h) = s.handle.as_ref() else { return };
        if s.cut {
            return;
        }
        let len = text.as_ref().map_or(data.len(), |t| t.len()) as u64;
        if waiting.saturating_add(len) > MAX_BUFFERED {
            s.cut = true;
            h.close(None, "");
            return;
        }
        match text {
            Some(t) => h.send_text(t),
            None => h.send_binary(data),
        }
    }

    /// `close()` of a script.
    pub(crate) fn socket_close(&mut self, id: u32, code: Option<u16>, reason: String) {
        let Some(s) = self.sockets.get_mut(&id) else { return };
        match s.handle.as_ref() {
            Some(h) => h.close(code, &reason),
            None => s.close = Some((code, reason)),
        }
    }

    /// `bufferedAmount`.
    pub(crate) fn socket_buffered(&self, id: u32) -> u64 {
        self.sockets.get(&id).and_then(|s| s.handle.as_ref()).map_or(0, |h| h.buffered())
    }

    /// Connections scripts asked for since the last call. The embedder opens each one and
    /// answers with `socket_opened`; events of the connection go to `socket_event`.
    pub fn take_socket_requests(&mut self) -> Vec<SocketRequest> {
        std::mem::take(&mut self.socket_requests)
    }

    /// The embedder started connecting socket `id`: this is how the page sends through it.
    pub fn socket_opened(&mut self, id: u32, handle: Box<dyn PageSocket>) {
        match self.sockets.get_mut(&id) {
            Some(s) => {
                if let Some((code, reason)) = s.close.take() {
                    handle.close(code, &reason);
                }
                s.handle = Some(handle);
            }
            // the socket is gone already (its event said so): nothing to keep
            None => handle.close(None, ""),
        }
    }

    /// Something happened on socket `id`. After `Closed` the id means nothing any more.
    pub fn socket_event(&mut self, id: u32, event: SocketEvent) {
        if !self.sockets.contains_key(&id) {
            return;
        }
        if matches!(event, SocketEvent::Closed { .. }) {
            self.sockets.remove(&id);
        }
        self.script(|e| e.socket_event(id, &event));
    }

    /// Sockets of this document that are open or opening.
    pub fn sockets(&self) -> usize {
        self.sockets.len()
    }

    /// The document goes away: its connections close.
    pub(crate) fn close_sockets(&mut self) {
        for (_, s) in self.sockets.drain() {
            if let Some(h) = s.handle {
                h.close(None, "");
            }
        }
        self.socket_requests.clear();
    }
}

pub(crate) type Sockets = HashMap<u32, Socket>;
