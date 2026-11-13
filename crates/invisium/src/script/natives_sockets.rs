//! Native primitives behind `WebSocket` (see `page_sockets.rs` for who does what).

use super::natives::{arg, def, Cx, NRes, Native};
use invisium_js::{Runtime, Value};
use std::rc::Rc;

/// The longest address, the most subprotocols and the longest close reason a script may give.
const MAX_URL: usize = 8192;
const MAX_PROTOCOLS: usize = 32;

/// `wsOpen(url, protocols)`: ask for a connection to an absolute `ws:`/`wss:` address,
/// offering the comma-separated subprotocols. Returns the socket's id, or 0 if the document
/// may not have another one.
fn n_ws_open(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let url = rt.to_string_lossy(arg(args, 0));
    let offered = rt.to_string_lossy(arg(args, 1));
    let protocols: Vec<String> = offered.split(',').map(str::trim).filter(|p| !p.is_empty()).map(str::to_string).collect();
    if url.len() > MAX_URL || protocols.len() > MAX_PROTOCOLS {
        return Ok(Value::Number(0.0));
    }
    let id = cx.st.borrow_mut().page.socket_open(url, protocols);
    Ok(Value::Number(id as f64))
}

/// `wsSend(id, data)`: a string goes out as text, a buffer or a view of one as binary.
fn n_ws_send(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let id = rt.to_number(arg(args, 0)) as u32;
    let v = arg(args, 1);
    let (text, data) = match rt.buffer_bytes(v) {
        Some(bytes) => (None, bytes),
        None => (Some(rt.to_string_lossy(v)), Vec::new()),
    };
    cx.st.borrow_mut().page.socket_send(id, text, data);
    Ok(Value::Undefined)
}

/// `wsClose(id, code, reason)`: start closing; `code` is undefined when the script gave none.
fn n_ws_close(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let id = rt.to_number(arg(args, 0)) as u32;
    let code = match arg(args, 1) {
        Value::Undefined | Value::Null => None,
        v => Some(rt.to_number(v)).filter(|c| c.is_finite() && *c >= 0.0 && *c <= 65535.0).map(|c| c as u16),
    };
    let reason = rt.to_string_lossy(arg(args, 2));
    cx.st.borrow_mut().page.socket_close(id, code, reason);
    Ok(Value::Undefined)
}

/// `wsBuffered(id)`: bytes given to `wsSend` that the network has not taken yet.
fn n_ws_buffered(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let id = rt.to_number(arg(args, 0)) as u32;
    Ok(Value::Number(cx.st.borrow().page.socket_buffered(id) as f64))
}

pub(crate) fn install(rt: &mut Runtime, n: Value, cx: &Rc<Cx>) {
    let table: &[(&str, u32, Native)] = &[("wsOpen", 2, n_ws_open), ("wsSend", 2, n_ws_send), ("wsClose", 3, n_ws_close), ("wsBuffered", 1, n_ws_buffered)];
    for &(name, arity, f) in table {
        def(rt, n, cx, name, arity, f);
    }
}
