//! Native primitives behind the windows of other documents (`window.parent`, `window.top`,
//! `iframe.contentWindow`): they can be sent messages and nothing else.

use super::natives::{arg, def, opt_node, Cx, NRes, Native};
use crate::page::frames::FrameMsg;
use invisium_js::{Runtime, Value};
use std::rc::Rc;

/// Messages waiting to be delivered, per document, and the size of one.
const MAX_QUEUED: usize = 4096;
const MAX_MESSAGE: usize = 16 << 20;

/// `frameDepth()`: how many documents this one is framed in (0 for a tab's own document).
fn n_frame_depth(_rt: &mut Runtime, cx: &Cx, _args: &[Value]) -> NRes {
    Ok(Value::Number(cx.st.borrow().page.frame_depth as f64))
}

/// `framePost(kind, frameElement, json, targetOrigin)`: queue a message for the parent
/// document (kind 0), the top document (1) or the document in one of this page's frames (2).
fn n_frame_post(rt: &mut Runtime, cx: &Cx, args: &[Value]) -> NRes {
    let kind = rt.to_number(arg(args, 0)) as u8;
    let node = opt_node(rt, cx, arg(args, 1))?;
    let json = rt.to_string_lossy(arg(args, 2));
    let target = rt.to_string_lossy(arg(args, 3));
    if kind > 2 || json.len() > MAX_MESSAGE {
        return Ok(Value::Undefined);
    }
    let mut st = cx.st.borrow_mut();
    let page = &mut st.page;
    if page.outbox.len() < MAX_QUEUED {
        let origin = page.origin();
        page.outbox.push(FrameMsg { to: kind, node, json, origin, target });
    }
    Ok(Value::Undefined)
}

pub(crate) fn install(rt: &mut Runtime, n: Value, cx: &Rc<Cx>) {
    let table: &[(&str, u32, Native)] = &[("frameDepth", 0, n_frame_depth), ("framePost", 4, n_frame_post)];
    for &(name, arity, f) in table {
        def(rt, n, cx, name, arity, f);
    }
}
