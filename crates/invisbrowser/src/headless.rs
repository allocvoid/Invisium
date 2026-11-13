//! Headless mode: load one page without a window and write a PNG.
//!
//!   invisbrowser --screenshot <url> <out.png> [--width N] [--height N] [--scale F]
//!                [--wait MS] [--full] [--scroll Y] [--dump] [--js] [--console]
//!
//! `--full` renders the whole document height (capped), `--dump` prints page statistics,
//! `--settle MS` is how long timers may keep running once the network is quiet (default 1500),
//! `--js` runs the page's scripts, `--console` prints what they log, `--eval <code>` runs
//! one more script after the page has loaded (for diagnosing a page; implies nothing else).

use crate::internal::{self, Info, Internal};
use crate::loader::{self, NetEvent, Sink};
use crate::settings::Settings;
use invisium::draw::{DrawList, Layer, Rect, Rgba};
use invisium::page::{Page, PageConfig};
use invisium::paint::PaintImage;
use invisium::text::FontSystem;
use invisium_gfx::{GpuOptions, Renderer};
use invisium_net::{NetService, Url};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub fn register_fonts(fonts: &mut FontSystem) {
    fonts.register_bundled("Geist", include_bytes!("../../invis-chrome/assets/fonts/geist-latin-var.woff2"));
    fonts.register_bundled("Geist Mono", include_bytes!("../../invis-chrome/assets/fonts/geist-mono-latin-var.woff2"));
}

fn arg_value<T: std::str::FromStr>(args: &[String], name: &str, default: T) -> T {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// Which navigation a network result belongs to.
fn event_nav(e: &NetEvent) -> u64 {
    match e {
        NetEvent::Document { nav, .. } | NetEvent::Sheet { nav, .. } | NetEvent::Script { nav, .. } | NetEvent::Fetch { nav, .. } | NetEvent::Image { nav, .. } | NetEvent::Font { nav, .. } | NetEvent::Frame { nav, .. } | NetEvent::Socket { nav, .. } | NetEvent::Download { nav, .. } => *nav,
    }
}

/// Fetch the document at `url` (following internal redirects) and turn it into markup; `url`
/// is updated to where the document really came from. `Err` is the process exit code.
#[allow(clippy::too_many_arguments)]
fn load_document(
    net: &NetService,
    sink: &Sink,
    rx: &mpsc::Receiver<NetEvent>,
    settings: &mut Settings,
    url: &mut String,
    body: Option<loader::FormBody>,
    nav: u64,
    deadline: Instant,
    dump: bool,
) -> Result<(String, bool), i32> {
    let mut hops = 0;
    let mut body = body;
    loop {
        if url.starts_with("invis:") {
            match internal::handle(url, settings, &Info::default()) {
                Internal::Html(h) => return Ok((h, false)),
                Internal::Redirect { url: u, .. } => {
                    *url = u;
                    hops += 1;
                    if hops > 5 {
                        eprintln!("internal redirect loop");
                        return Err(1);
                    }
                    continue;
                }
            }
        }
        let parsed = match Url::parse(url) {
            Ok(u) => u,
            Err(e) => {
                eprintln!("invalid address {url}: {e:?}");
                return Err(1);
            }
        };
        let _h = loader::fetch_document(net, sink, 1, nav, parsed, body.take(), None, false);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(NetEvent::Document { nav: n, result, .. }) if n == nav => match result {
                    Ok(resp) => {
                        *url = resp.url.as_str().to_string();
                        if dump {
                            eprintln!("document: {} {} ({} bytes, {:?})", resp.status, resp.mime, resp.body.len(), resp.via);
                        }
                        return Ok((loader::document_html(&resp), resp.served_local));
                    }
                    Err(e) => {
                        eprintln!("navigation failed: {e:?}");
                        return Ok((loader::error_html(&e, url), false));
                    }
                },
                Ok(_) => continue, // a result for a page that has been left
                Err(_) => {
                    eprintln!("timed out waiting for the document");
                    return Err(1);
                }
            }
        }
    }
}

pub fn run(args: &[String]) -> i32 {
    let (Some(url_arg), Some(out)) = (args.first(), args.get(1)) else {
        eprintln!("usage: invisbrowser --screenshot <url> <out.png> [--width N] [--height N] [--scale F] [--wait MS] [--full] [--scroll Y] [--dump] [--js] [--console]");
        return 2;
    };
    let width: u32 = arg_value(args, "--width", 1280);
    let height: u32 = arg_value(args, "--height", 800);
    let scale: f32 = arg_value(args, "--scale", 1.0);
    let wait_ms: u64 = arg_value(args, "--wait", 15_000);
    let scroll_y: f32 = arg_value(args, "--scroll", 0.0);
    let full = args.iter().any(|a| a == "--full");
    let dump = args.iter().any(|a| a == "--dump");
    let scripting = args.iter().any(|a| a == "--js");
    let show_console = args.iter().any(|a| a == "--console");
    let eval: Option<String> = args.iter().position(|a| a == "--eval").and_then(|i| args.get(i + 1)).cloned();
    // --patch-script FIND REPLACE: edit the page's scripts as they arrive (to add logging to
    // code that cannot be reached from outside)
    let patch: Option<(String, String)> = args
        .iter()
        .position(|a| a == "--patch-script")
        .and_then(|i| Some((args.get(i + 1)?.clone(), args.get(i + 2)?.clone())));
    // like --eval, but run before any --scroll (to set up logging for what scrolling causes)
    let pre: Option<String> = args.iter().position(|a| a == "--pre").and_then(|i| args.get(i + 1)).cloned();

    let mut settings = Settings::default();
    settings.letterbox = false;
    // for test pages served from this machine (`python -m http.server`): allow plain HTTP
    if args.iter().any(|a| a == "--http") {
        settings.https_only = false;
    }
    let net = Arc::new(NetService::new(settings.net_config()));
    let (tx, rx) = mpsc::channel::<NetEvent>();
    let sink: Sink = Arc::new(move |e| {
        let _ = tx.send(e);
    });
    // shared with the page: scripts can force layout
    let fonts = std::rc::Rc::new(std::cell::RefCell::new(FontSystem::new()));
    register_fonts(&mut fonts.borrow_mut());

    let started = Instant::now();
    let deadline = started + Duration::from_millis(wait_ms);
    let viewport = (width as f32, height as f32);
    let cfg = PageConfig { dark: false, load_images: settings.load_images, scripting };

    // ── the document ──
    let mut url = loader::fix_url(url_arg, &settings.search);
    let mut nav: u64 = 1;
    let (html, mut served_local) = match load_document(&net, &sink, &rx, &mut settings, &mut url, None, nav, deadline, dump) {
        Ok(h) => h,
        Err(code) => return code,
    };

    let mut page = Page::new(&url, &html, loader::url_join(), viewport, cfg.clone());
    page.set_fonts(fonts.clone());
    if scripting {
        page.set_cookie_access(loader::cookie_access(net.clone()));
        page.set_script_stack(loader::SCRIPT_STACK);
    }
    let mut renderer = match pollster::block_on(Renderer::new_headless(
        (width as f32 * scale) as u32,
        (height as f32 * scale) as u32,
        GpuOptions { low_power: true, vsync: false, max_atlas_size: 4096 },
    )) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("no GPU renderer: {e}");
            return 1;
        }
    };
    if scripting {
        // WebGL canvases draw with the renderer's device
        page.set_gl_device(renderer.gl_device());
    }

    // ── subresources and scripts ──
    let mut outstanding = 0usize;
    let mut handles = Vec::new();
    let (mut sheets, mut images, mut scripts, mut fetches, mut blocked, mut failed) = (0u32, 0u32, 0u32, 0u32, 0u32, 0u32);
    let mut web_fonts = 0u32;
    let mut console: Vec<invisium::script::ConsoleMessage> = Vec::new();
    // how long to keep running timers once the network is quiet (pages with intervals never settle)
    let mut settle_left = Duration::from_millis(arg_value(args, "--settle", 1500));
    let mut script_hops = 0;
    let now_ms = |t: Instant| t.duration_since(started).as_secs_f64() * 1000.0;
    loop {
        if scripting {
            for _ in 0..16 {
                if !page.pump(now_ms(Instant::now())) {
                    break;
                }
            }
            if page.wants_frame() {
                page.frame(now_ms(Instant::now()));
            }
            console.extend(page.take_console());
            if let Some(action) = page.take_action() {
                let (target, body) = match action {
                    invisium::page::PageAction::Navigate { url, .. } => (url, None),
                    invisium::page::PageAction::Submit { url, body, content_type } => (url, body.map(|b| (b, content_type))),
                    _ => (String::new(), None),
                };
                if target.is_empty() || script_hops >= 4 || !loader::page_may_open(&page.url, &target) {
                    eprintln!("script asked to navigate to {target}; not followed");
                } else {
                    // leave this page: results still in flight for it are ignored from here on
                    script_hops += 1;
                    nav += 1;
                    if dump {
                        eprintln!("script navigated to {target}");
                    }
                    url = target;
                    let (html, local) = match load_document(&net, &sink, &rx, &mut settings, &mut url, body, nav, deadline, dump) {
                        Ok(h) => h,
                        Err(code) => return code,
                    };
                    served_local = local;
                    handles.clear();
                    outstanding = 0;
                    page = Page::new(&url, &html, loader::url_join(), viewport, cfg.clone());
                    page.set_cookie_access(loader::cookie_access(net.clone()));
                    page.set_fonts(fonts.clone());
                    page.set_script_stack(loader::SCRIPT_STACK);
                    page.set_gl_device(renderer.gl_device());
                    continue;
                }
            }
        }
        let new = loader::fetch_subresources(&net, &sink, 1, nav, &mut page, served_local);
        outstanding += new.len();
        handles.extend(new);
        let mut wait = deadline.saturating_duration_since(Instant::now());
        if outstanding == 0 {
            // layout may reveal more (background images); stop when it asks for nothing new
            page.update(&mut fonts.borrow_mut());
            let more = loader::fetch_subresources(&net, &sink, 1, nav, &mut page, served_local);
            if more.is_empty() {
                // the network is quiet: give timers that are due soon a chance, then stop
                // a page that animates gets a frame every 16 ms, as it would on screen
                let frame_due = if page.wants_frame() { Some(now_ms(Instant::now()) + 16.0) } else { None };
                let next = match (page.next_timer_ms(), frame_due) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                };
                let due = next.map(|t| Duration::from_secs_f64((t - now_ms(Instant::now())).max(0.0) / 1000.0));
                match due {
                    Some(d) if d <= settle_left && d < wait => {
                        settle_left -= d;
                        std::thread::sleep(d);
                        continue;
                    }
                    // an open socket may still bring news: listen for it while settle time is left
                    _ if page.sockets() > 0 && !settle_left.is_zero() && !wait.is_zero() => {
                        let step = settle_left.min(Duration::from_millis(50)).min(wait);
                        settle_left -= step;
                        wait = step;
                    }
                    _ => break,
                }
            }
            outstanding += more.len();
            handles.extend(more);
        }
        if wait.is_zero() {
            eprintln!("timed out with {outstanding} resources still loading");
            break;
        }
        // wake for the next timer even if no network result arrives
        if let Some(t) = page.next_timer_ms() {
            let until = Duration::from_secs_f64((t - now_ms(Instant::now())).max(1.0) / 1000.0);
            wait = wait.min(until);
        }
        // and for the next frame, if the page is animating or has frame callbacks queued
        if scripting && page.wants_frame() {
            wait = wait.min(Duration::from_millis(16));
        }
        let mut count_failure = |e: &invisium_net::NetError| {
            if loader::is_blocked(e) {
                blocked += 1;
            } else {
                failed += 1;
            }
        };
        match rx.recv_timeout(wait) {
            Ok(ev) if event_nav(&ev) != nav => {} // belongs to a page that has been left
            Ok(NetEvent::Sheet { page: pid, req, result, .. }) => {
                let Some(page) = page.page_by_id(pid) else { continue };
                outstanding = outstanding.saturating_sub(1);
                match result {
                    Ok(resp) if resp.status < 400 => {
                        sheets += 1;
                        page.stylesheet_loaded(req, &invisium::page::decode_text(&resp.body, resp.charset.as_deref()));
                    }
                    Ok(_) => {
                        failed += 1;
                        page.resource_failed(req);
                    }
                    Err(e) => {
                        count_failure(&e);
                        page.resource_failed(req);
                    }
                }
            }
            Ok(NetEvent::Script { page: pid, req, result, .. }) => {
                let Some(page) = page.page_by_id(pid) else { continue };
                outstanding = outstanding.saturating_sub(1);
                match result {
                    Ok(resp) if resp.status < 400 => {
                        scripts += 1;
                        let mut source = invisium::page::decode_text(&resp.body, resp.charset.as_deref());
                        if let Some((find, with)) = &patch {
                            if source.contains(find.as_str()) {
                                eprintln!("patched a script ({} bytes)", source.len());
                                source = source.replace(find.as_str(), with);
                            }
                        }
                        page.script_loaded(req, &source);
                    }
                    Ok(_) => {
                        failed += 1;
                        page.resource_failed(req);
                    }
                    Err(e) => {
                        count_failure(&e);
                        page.resource_failed(req);
                    }
                }
            }
            Ok(NetEvent::Fetch { page: pid, id, result, .. }) => {
                let Some(page) = page.page_by_id(pid) else { continue };
                outstanding = outstanding.saturating_sub(1);
                if let Err(e) = &result {
                    count_failure(e);
                } else {
                    fetches += 1;
                }
                page.fetch_done(id, loader::fetch_result(result));
            }
            Ok(NetEvent::Socket { page: pid, id, event, .. }) => {
                let Some(page) = page.page_by_id(pid) else { continue };
                page.socket_event(id, loader::socket_event(event));
            }
            Ok(NetEvent::Image { page: pid, url, result, .. }) => {
                let Some(page) = page.page_by_id(pid) else { continue };
                outstanding = outstanding.saturating_sub(1);
                match result {
                    Ok(img) => {
                        images += 1;
                        let id = renderer.create_image(img.width, img.height, &img.rgba);
                        page.image_ready(&url, PaintImage { id, width: img.orig_w as f32, height: img.orig_h as f32 });
                    }
                    Err(e) => count_failure(&e),
                }
            }
            Ok(NetEvent::Font { page: pid, req, result, .. }) => {
                let Some(page) = page.page_by_id(pid) else { continue };
                outstanding = outstanding.saturating_sub(1);
                match result {
                    Ok(resp) if resp.status < 400 => {
                        if page.font_loaded(req, &resp.body, &mut fonts.borrow_mut()) {
                            web_fonts += 1;
                        }
                    }
                    Ok(_) => {
                        failed += 1;
                        page.resource_failed(req);
                    }
                    Err(e) => {
                        count_failure(&e);
                        page.resource_failed(req);
                    }
                }
            }
            Ok(NetEvent::Frame { page: pid, node, result, .. }) => {
                outstanding = outstanding.saturating_sub(1);
                let Some(owner) = page.page_by_id(pid) else { continue };
                match result {
                    Ok(resp) => {
                        let html = loader::document_html(&resp);
                        owner.frame_loaded(node, resp.url.as_str(), &html, resp.served_local);
                    }
                    Err(e) => {
                        count_failure(&e);
                        owner.frame_failed(node);
                    }
                }
            }
            // not asked for here (see `load_document`): nothing is ever saved in screenshot mode
            Ok(NetEvent::Document { .. }) | Ok(NetEvent::Download { .. }) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => {} // a timer is due, or the deadline passed (checked above)
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    drop(handles);
    if let (true, Some(code)) = (scripting, pre.as_ref()) {
        page.update(&mut fonts.borrow_mut());
        page.eval(code);
        console.extend(page.take_console());
    }
    page.update(&mut fonts.borrow_mut());
    let (_, doc_h) = page.content_size();
    let mut out_h = height as f32;
    if full {
        out_h = doc_h.clamp(height as f32, 12_000.0);
        renderer.resize((width as f32 * scale) as u32, (out_h * scale) as u32);
        page.set_viewport(width as f32, out_h);
        page.update(&mut fonts.borrow_mut());
    } else if scroll_y > 0.0 {
        // look at another part of the page at the real viewport height (`--full` grows the
        // viewport, which inflates everything sized in viewport units)
        page.scroll_to(0.0, scroll_y);
        page.update(&mut fonts.borrow_mut());
        if scripting {
            // scroll listeners and the animations they start get a second to run
            let until = Instant::now() + Duration::from_millis(arg_value(args, "--after", 1000));
            while Instant::now() < until {
                page.pump(now_ms(Instant::now()));
                page.frame(now_ms(Instant::now()));
                page.update(&mut fonts.borrow_mut());
                std::thread::sleep(Duration::from_millis(16));
            }
            console.extend(page.take_console());
            page.update(&mut fonts.borrow_mut());
        }
    }
    // `--pointer "x,y x,y d x,y u w300"`: move the pointer along these points of the viewport,
    // with `d`/`u` for the primary button going down and up and `wN` for a pause of N ms.
    // Frames run for 50 ms after every step, as they would while a user moves the mouse.
    if let Some(path) = args.iter().position(|a| a == "--pointer").and_then(|i| args.get(i + 1)) {
        let mut at = (0.0f32, 0.0f32);
        for step in path.split_whitespace() {
            let mut pause = 50;
            if step == "d" {
                page.press(at.0, at.1, invisium::page::PointerButton::Primary);
            } else if step == "u" {
                page.release(at.0, at.1, invisium::page::PointerButton::Primary, false);
            } else if let Some(ms) = step.strip_prefix('w').and_then(|n| n.parse::<u64>().ok()) {
                pause = ms;
            } else if let Some((x, y)) = step.split_once(',') {
                at = (x.parse().unwrap_or(0.0), y.parse().unwrap_or(0.0));
                page.mouse_move(at.0, at.1);
            }
            let until = Instant::now() + Duration::from_millis(pause);
            while Instant::now() < until {
                page.pump(now_ms(Instant::now()));
                page.frame(now_ms(Instant::now()));
                page.update(&mut fonts.borrow_mut());
                std::thread::sleep(Duration::from_millis(16));
            }
        }
        console.extend(page.take_console());
        page.update(&mut fonts.borrow_mut());
    }
    // the diagnostic script sees the page as it will be drawn (after any scrolling)
    if let (true, Some(code)) = (scripting, eval.as_ref()) {
        page.update(&mut fonts.borrow_mut());
        page.eval(code);
        console.extend(page.take_console());
        page.update(&mut fonts.borrow_mut());
    }
    {
        let mut upload = |w: u32, h: u32, px: &[u8]| renderer.create_image(w, h, px);
        if loader::rasterize_inline_svgs(&mut page, scale, &mut upload) {
            page.update(&mut fonts.borrow_mut());
        }
        if loader::upload_canvases(&mut page, &mut upload) {
            page.update(&mut fonts.borrow_mut());
        }
    }
    let empty = DrawList::new();
    let _ = &empty;
    let layer = Layer {
        list: page.display_list(),
        glyphs: page.glyphs(),
        viewport: Rect::new(0.0, 0.0, width as f32 * scale, out_h * scale),
        scroll: page.scroll(),
        scale,
    };
    let canvas: Rgba = page.canvas_color();
    if let Err(e) = renderer.render(&[layer], &*fonts.borrow(), canvas) {
        eprintln!("render failed: {e}");
        return 1;
    }
    if let Err(e) = renderer.save_png(std::path::Path::new(out)) {
        eprintln!("could not write {out}: {e}");
        return 1;
    }
    if invisium::draw::prof::enabled() {
        eprintln!("profile (main thread, whole run):
{}", invisium::draw::prof::report());
        let rows = page.take_script_profile();
        let total: f64 = rows.iter().map(|r| r.1).sum();
        eprintln!("script functions by their own time ({total:.0} ms sampled):");
        for (what, ms) in rows.iter().take(60) {
            eprintln!("  {ms:>8.1} ms  {what}");
        }
    }
    if dump {
        let m = page.memory();
        let g = renderer.stats();
        let n = net.stats();
        eprintln!("title: {}", page.title);
        eprintln!("url: {url}");
        eprintln!("loaded in {} ms: {sheets} stylesheets, {images} images, {scripts} scripts, {fetches} script requests, {blocked} blocked, {failed} failed", started.elapsed().as_millis());
        eprintln!("document: {} nodes, {} boxes, {} unique styles, height {:.0}px", m.nodes, m.boxes, m.unique_styles, doc_h);
        eprintln!(
            "memory: dom {} KB, styles {} KB, layout {} KB, display list {} KB ({} items)",
            m.dom / 1024,
            m.styles / 1024,
            m.layout / 1024,
            m.display_list / 1024,
            page.display_list().items.len()
        );
        eprintln!(
            "gpu: {} | atlas {} KB ({} glyphs), images {} KB, {} instances in {} draw calls",
            g.adapter,
            g.atlas_bytes / 1024,
            g.atlas_glyphs,
            g.image_bytes / 1024,
            g.last_frame_instances,
            g.last_frame_draw_calls
        );
        eprintln!("network: {} requests, {} blocked, {} KB received", n.requests, n.blocked, n.bytes_received / 1024);
        eprintln!("web fonts: {web_fonts} loaded, {} KB", fonts.borrow().web_font_bytes() / 1024);
        // canvases with a context, in every document of the page, and what WebGL holds on the GPU
        let (mut canvases, mut canvas_bytes) = (0usize, 0usize);
        page.for_each_page(&mut |p| {
            let (n, b) = p.canvas_stats();
            canvases += n;
            canvas_bytes += b;
        });
        if canvases > 0 {
            let gl = renderer.gl_device().borrow().stats();
            eprintln!(
                "canvas: {canvases} with a context, {} KB on the CPU | WebGL: {} contexts, buffers {} KB, textures {} KB, {} programs, {} pipelines",
                canvas_bytes / 1024,
                gl.contexts,
                gl.buffer_bytes / 1024,
                gl.texture_bytes / 1024,
                gl.programs,
                gl.pipelines
            );
        }
        // what one fully invalidated frame costs
        let mut best = [f32::MAX; 3];
        for _ in 0..5 {
            page.invalidate();
            page.update(&mut fonts.borrow_mut());
            let t = page.pass_times();
            for i in 0..3 {
                best[i] = best[i].min(t[i]);
            }
        }
        eprintln!("frame cost: style {:.2} ms, layout {:.2} ms, paint {:.2} ms (best of 5 full passes)", best[0], best[1], best[2]);
        if scripting {
            let errors = console.iter().filter(|m| m.level == invisium::script::ConsoleLevel::Error).count();
            eprintln!(
                "script: heap {} KB, {} console messages ({errors} errors), {} scripts not run, {} forced layouts",
                m.script / 1024,
                console.len(),
                page.pending_scripts(),
                page.forced_layouts()
            );
        }
    }
    if show_console {
        for m in &console {
            eprintln!("[js {:?}] {}", m.level, m.text);
        }
    }
    println!("wrote {out}");
    0
}
