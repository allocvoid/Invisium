//! One WebSocket session after the handshake: frame reading and validation, message
//! reassembly, ping/pong, the closing handshake and back-pressure. A writer task owns the
//! sending half; this task owns the reading half and every decision.

use crate::connect::{ConnGuard, Io};
use crate::service::Inner;
use crate::types::{WsEvent, WsRequest};
use crate::ws::{handshake, Cmd};
use crate::wsproto::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::mpsc;
use tokio::time::Instant;

const MAX_MESSAGE: usize = 16 * 1024 * 1024;
const MAX_BUFFERED: u64 = 64 * 1024 * 1024;
const CLOSE_WAIT: Duration = Duration::from_secs(5);

type Events = Box<dyn FnMut(WsEvent) + Send>;

/// Why reading stopped: the peer broke the protocol (close with this code) or the transport
/// ended.
enum Stop {
    Fail(u16),
    Gone,
}

struct Frame {
    fin: bool,
    opcode: u8,
    payload: Vec<u8>,
}

struct FrameReader {
    io: ReadHalf<Box<dyn Io>>,
    buf: Vec<u8>,
    tmp: Box<[u8]>,
}

impl FrameReader {
    /// Makes `n` bytes available in `buf`. Cancel-safe: bytes are appended only once read.
    async fn need(&mut self, n: usize) -> Result<(), Stop> {
        while self.buf.len() < n {
            let got = self.io.read(&mut self.tmp).await.map_err(|_| Stop::Gone)?;
            if got == 0 {
                return Err(Stop::Gone);
            }
            self.buf.extend_from_slice(&self.tmp[..got]);
        }
        Ok(())
    }

    /// The next frame. Nothing is consumed until the whole frame is buffered, so dropping this
    /// future (another event won the select) loses nothing. `pending` is the size of the
    /// message being reassembled.
    async fn frame(&mut self, pending: usize) -> Result<Frame, Stop> {
        self.need(2).await?;
        let s = frame_start(self.buf[0], self.buf[1]);
        if s.rsv != 0 || !matches!(s.opcode, OP_CONT | OP_TEXT | OP_BINARY | OP_CLOSE | OP_PING | OP_PONG) {
            return Err(Stop::Fail(1002));
        }
        let control = s.opcode >= 8;
        if control && (!s.fin || s.len7 > 125) {
            return Err(Stop::Fail(1002));
        }
        // Servers must not mask (RFC 6455 section 5.1).
        if s.masked {
            return Err(Stop::Fail(1002));
        }
        let (len, header) = match s.len7 {
            126 => {
                self.need(4).await?;
                (u64::from(u16::from_be_bytes([self.buf[2], self.buf[3]])), 4)
            }
            127 => {
                self.need(10).await?;
                let mut b = [0u8; 8];
                b.copy_from_slice(&self.buf[2..10]);
                let len = u64::from_be_bytes(b);
                if len >> 63 != 0 {
                    return Err(Stop::Fail(1002));
                }
                (len, 10)
            }
            n => (u64::from(n), 2),
        };
        if !control && (pending as u64).saturating_add(len) > MAX_MESSAGE as u64 {
            return Err(Stop::Fail(1009));
        }
        let total = header + len as usize;
        self.need(total).await?;
        let payload = self.buf[header..total].to_vec();
        self.buf.drain(..total);
        Ok(Frame { fin: s.fin, opcode: s.opcode, payload })
    }
}

enum Out {
    Data { opcode: u8, payload: Vec<u8> },
    Control { opcode: u8, payload: Vec<u8> },
    Shutdown,
}

/// Writes frames in order. Data frames count down `buffered` once written, and are dropped
/// unwritten once `abort` is set (the connection is being failed).
async fn writer(
    mut io: WriteHalf<Box<dyn Io>>,
    mut rx: mpsc::UnboundedReceiver<Out>,
    buffered: Arc<AtomicU64>,
    abort: Arc<AtomicBool>,
    _guard: ConnGuard,
) {
    while let Some(out) = rx.recv().await {
        let (opcode, payload, user) = match out {
            Out::Shutdown => break,
            Out::Data { opcode, payload } => {
                let n = payload.len() as u64;
                if abort.load(Ordering::SeqCst) {
                    buffered.fetch_sub(n, Ordering::SeqCst);
                    continue;
                }
                (opcode, payload, n)
            }
            Out::Control { opcode, payload } => (opcode, payload, 0),
        };
        let Some(mask) = random::<4>() else { break };
        let ok = io.write_all(&encode_frame(opcode, &payload, mask)).await.is_ok() && io.flush().await.is_ok();
        buffered.fetch_sub(user, Ordering::SeqCst);
        if !ok {
            break;
        }
    }
    // Whatever stays queued is never written.
    while let Ok(out) = rx.try_recv() {
        if let Out::Data { payload, .. } = out {
            buffered.fetch_sub(payload.len() as u64, Ordering::SeqCst);
        }
    }
    let _ = tokio::time::timeout(CLOSE_WAIT, io.shutdown()).await;
}

fn closed(code: u16, reason: String, clean: bool) -> WsEvent {
    WsEvent::Closed { code, reason, clean }
}

/// The parsed payload of a server Close frame, or the failure code it deserves.
fn parse_close(p: &[u8]) -> Result<(u16, String), u16> {
    match p.len() {
        0 => Ok((1005, String::new())),
        1 => Err(1002),
        _ => {
            let code = u16::from_be_bytes([p[0], p[1]]);
            if !valid_close_code(code) {
                return Err(1002);
            }
            String::from_utf8(p[2..].to_vec()).map(|r| (code, r)).map_err(|_| 1007)
        }
    }
}

pub(crate) async fn run(
    inner: Arc<Inner>,
    req: WsRequest,
    mut cmds: mpsc::UnboundedReceiver<Cmd>,
    buffered: Arc<AtomicU64>,
    events: Events,
) {
    session(inner, req, &mut cmds, buffered.clone(), events).await;
    // Data the page queued that will never be sent no longer counts as buffered; later sends
    // fail in the handle and are not counted at all.
    cmds.close();
    while let Ok(cmd) = cmds.try_recv() {
        if let Cmd::Data(_, payload) = cmd {
            buffered.fetch_sub(payload.len() as u64, Ordering::SeqCst);
        }
    }
}

async fn session(
    inner: Arc<Inner>,
    req: WsRequest,
    cmds: &mut mpsc::UnboundedReceiver<Cmd>,
    buffered: Arc<AtomicU64>,
    mut events: Events,
) {
    let mut clear = inner.ws_close.subscribe();
    let est = tokio::select! {
        r = handshake(&inner, &req) => r.ok(),
        _ = clear.changed() => None,
    };
    let Some(est) = est else {
        // The page learns nothing but that the socket closed abnormally.
        events(closed(1006, String::new(), false));
        return;
    };
    let (rd, wr) = tokio::io::split(est.io);
    let (out_tx, out_rx) = mpsc::unbounded_channel();
    let abort = Arc::new(AtomicBool::new(false));
    tokio::spawn(writer(wr, out_rx, buffered.clone(), abort.clone(), est.guard));
    events(WsEvent::Open { protocol: est.protocol });
    let mut reader = FrameReader { io: rd, buf: est.rest, tmp: vec![0u8; 64 * 1024].into_boxed_slice() };
    let fail = |code: u16, events: &mut Events| {
        abort.store(true, Ordering::SeqCst);
        let _ = out_tx.send(Out::Control { opcode: OP_CLOSE, payload: close_payload(Some(code), "") });
        let _ = out_tx.send(Out::Shutdown);
        // As the WebSocket standard has it, a failed connection reports 1006 to the page.
        events(closed(1006, String::new(), false));
    };
    let start_close = |code: Option<u16>, reason: &str| {
        let _ = out_tx.send(Out::Control { opcode: OP_CLOSE, payload: close_payload(code, reason) });
        Instant::now() + CLOSE_WAIT
    };
    // Some(deadline) once this side has sent its Close frame.
    let mut closing: Option<Instant> = None;
    let mut message: Option<(u8, Vec<u8>)> = None;
    let mut cmds_open = true;
    loop {
        let pending = message.as_ref().map_or(0, |(_, m)| m.len());
        let deadline = closing.unwrap_or_else(|| Instant::now() + Duration::from_secs(86_400));
        tokio::select! {
            frame = reader.frame(pending) => {
                let frame = match frame {
                    Ok(f) => f,
                    Err(Stop::Fail(code)) => break fail(code, &mut events),
                    Err(Stop::Gone) => {
                        let _ = out_tx.send(Out::Shutdown);
                        return events(closed(1006, String::new(), false));
                    }
                };
                match frame.opcode {
                    OP_CLOSE => {
                        let (code, reason) = match parse_close(&frame.payload) {
                            Ok(c) => c,
                            Err(code) => break fail(code, &mut events),
                        };
                        if closing.is_none() {
                            // Answer the peer's Close, then give it time to end the transport.
                            let echo = (code != 1005).then_some(code);
                            let _ = out_tx.send(Out::Control { opcode: OP_CLOSE, payload: close_payload(echo, "") });
                            let _ = out_tx.send(Out::Shutdown);
                            let _ = tokio::time::timeout(CLOSE_WAIT, async {
                                while reader.frame(0).await.is_ok() {}
                            })
                            .await;
                        } else {
                            let _ = out_tx.send(Out::Shutdown);
                        }
                        return events(closed(code, reason, true));
                    }
                    _ if closing.is_some() => {} // after our Close: nothing more is delivered
                    OP_PING => {
                        let _ = out_tx.send(Out::Control { opcode: OP_PONG, payload: frame.payload });
                    }
                    OP_PONG => {}
                    OP_CONT => {
                        let Some((op, mut data)) = message.take() else { break fail(1002, &mut events) };
                        data.extend_from_slice(&frame.payload);
                        if !frame.fin {
                            message = Some((op, data));
                        } else if let Err(code) = deliver(op, data, &mut events) {
                            break fail(code, &mut events);
                        }
                    }
                    op => {
                        if message.is_some() {
                            break fail(1002, &mut events);
                        }
                        if !frame.fin {
                            message = Some((op, frame.payload));
                        } else if let Err(code) = deliver(op, frame.payload, &mut events) {
                            break fail(code, &mut events);
                        }
                    }
                }
            }
            cmd = cmds.recv(), if cmds_open => match cmd {
                None => cmds_open = false,
                Some(Cmd::Data(opcode, payload)) if closing.is_none() => {
                    if buffered.load(Ordering::SeqCst) > MAX_BUFFERED {
                        buffered.fetch_sub(payload.len() as u64, Ordering::SeqCst);
                        break fail(1009, &mut events);
                    }
                    let _ = out_tx.send(Out::Data { opcode, payload });
                }
                Some(Cmd::Data(_, payload)) => {
                    buffered.fetch_sub(payload.len() as u64, Ordering::SeqCst);
                }
                Some(Cmd::Close(code, reason)) => {
                    if closing.is_none() {
                        let code = code.filter(|c| *c == 1000 || (3000..=4999).contains(c));
                        closing = Some(start_close(code, &reason));
                    }
                }
            },
            changed = clear.changed(), if closing.is_none() => {
                // Site data cleared (or the service is gone): going away.
                let _ = changed;
                closing = Some(start_close(Some(1001), ""));
            }
            _ = tokio::time::sleep_until(deadline), if closing.is_some() => {
                abort.store(true, Ordering::SeqCst);
                let _ = out_tx.send(Out::Shutdown);
                return events(closed(1006, String::new(), false));
            }
        }
    }
    // The connection failed and our Close is on its way: keep reading (and discarding) until
    // the server closes, for at most the closing wait, so the Close is not lost to a reset.
    let _ = tokio::time::timeout(CLOSE_WAIT, async {
        while matches!(reader.io.read(&mut reader.tmp).await, Ok(n) if n > 0) {}
    })
    .await;
}

/// Hands a complete message to the page; text must be valid UTF-8.
fn deliver(opcode: u8, data: Vec<u8>, events: &mut Events) -> Result<(), u16> {
    if opcode == OP_TEXT {
        let text = String::from_utf8(data).map_err(|_| 1007u16)?;
        events(WsEvent::Text(text));
    } else {
        events(WsEvent::Binary(data));
    }
    Ok(())
}
