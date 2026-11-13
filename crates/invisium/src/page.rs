//! A loaded document: owns the DOM, styles, layout and display list, discovers subresources,
//! and turns input events into actions.
//!
//! The page is network-agnostic. It hands the embedder a list of `ResourceRequest`s and is told
//! when each arrives, so the whole pipeline can be driven (and tested) without sockets, and every
//! request the page wants passes through the embedder's privacy policy.

use crate::atom::{a, Atom};
use crate::css::parser::{parse_stylesheet, MediaEnv, MediaQueryList, Origin, Rule, Stylesheet};
use crate::css::tokenizer::tokenize_value;
use crate::css::values::Image;
use crate::dom::{Document, NodeId, DOCUMENT_NODE, NO_NODE};
use crate::draw::{DrawList, Glyph, Rgba};
use crate::html;
use crate::layout::{layout_document, BoxId, LayoutInput, LayoutTree, NO_BOX};
use crate::paint::{paint, PaintImage, PaintInput};
use crate::script::{ConsoleMessage, EventInit, ScriptEngine};
use crate::style::{Cursor, SheetEntry, StyleInput, StyleStore, Stylist};
use crate::text::FontSystem;
use std::collections::{HashMap, HashSet, VecDeque};
use std::cell::RefCell;
use std::rc::Rc;

#[path = "page_script.rs"]
mod scripting;
#[path = "page_anim.rs"]
mod animating;
#[path = "page_canvas.rs"]
pub(crate) mod canvas;
#[path = "page_frames.rs"]
pub(crate) mod frames;
pub use frames::FrameRequest;

/// Resolves a reference against a base URL: `(base, reference) -> absolute`.
pub type UrlJoin = Rc<dyn Fn(&str, &str) -> Option<String>>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResKind {
    Stylesheet,
    Image,
    Script,
    /// A web font (`@font-face`). Answer with `Page::font_loaded` (or `resource_failed`).
    Font,
}

/// A `@font-face` rule of the page and whether its file has been asked for.
pub(crate) struct WebFontRule {
    family: Atom,
    url: String,
    weight: (u16, u16),
    italic: bool,
    ranges: Vec<(u32, u32)>,
}

/// Font scopes: every page gets its own, so that fonts one page loads are invisible to others.
static NEXT_FONT_SCOPE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

#[derive(Clone, Debug)]
pub struct ResourceRequest {
    pub id: u32,
    pub url: String,
    pub kind: ResKind,
}

/// A network request started by page script (`fetch`, `XMLHttpRequest`). The embedder must
/// send it through a network stack that enforces the cross-origin rules, and report the
/// outcome with `Page::fetch_done`.
#[derive(Clone, Debug)]
pub struct ScriptFetch {
    pub id: u32,
    /// Absolute `http`, `https` or `data` address.
    pub url: String,
    /// Upper-case: GET, POST, HEAD, PUT, DELETE, PATCH or OPTIONS.
    pub method: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    /// The request must not leave the page's origin (`fetch` with `mode: "same-origin"`).
    pub same_origin: bool,
    /// 0 = never send cookies, 1 = only to the page's own origin, 2 = also cross-origin.
    pub credentials: u8,
}

/// The response to a `ScriptFetch`, already filtered by the network stack.
#[derive(Clone, Debug)]
pub struct FetchResult {
    pub status: u16,
    /// Final address after redirects.
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// `document.cookie` access provided by the embedder: (read cookies for a document address,
/// store a cookie string for a document address).
pub type CookieAccess = (Rc<dyn Fn(&str) -> String>, Rc<dyn Fn(&str, &str)>);

#[derive(Clone, Debug)]
pub struct PageConfig {
    /// Report a dark colour scheme to `prefers-color-scheme`.
    pub dark: bool,
    pub load_images: bool,
    /// Whether scripts will run (controls `<noscript>`).
    pub scripting: bool,
}

impl Default for PageConfig {
    fn default() -> Self {
        PageConfig { dark: false, load_images: true, scripting: false }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PageAction {
    None,
    /// Something changed on screen.
    Redraw,
    Navigate { url: String, new_tab: bool },
    /// Form submission. `body` is `Some` for POST; `content_type` is `None` for the default
    /// encoding (`application/x-www-form-urlencoded`) and set for `multipart/form-data`.
    Submit { url: String, body: Option<Vec<u8>>, content_type: Option<String> },
    /// The user activated a file control. The embedder shows its file dialog and answers with
    /// `Page::files_chosen` (not at all if the dialog is cancelled).
    PickFiles { control: NodeId, multiple: bool, accept: String },
}

/// A file the user chose for a file control. The embedder reads it; the page only ever sees
/// files that were handed to it this way.
#[derive(Debug)]
pub struct PickedFile {
    pub name: String,
    /// MIME type guessed by the embedder ("" if unknown).
    pub mime: String,
    pub last_modified_ms: f64,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PointerKind {
    Default,
    Pointer,
    Text,
}

#[derive(Clone, Debug, Default)]
pub struct Hover {
    pub link: Option<String>,
    pub pointer: Option<PointerKind>,
    pub changed: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PageMemory {
    pub dom: usize,
    pub styles: usize,
    pub layout: usize,
    pub display_list: usize,
    pub nodes: usize,
    pub boxes: usize,
    pub unique_styles: usize,
    /// JavaScript heap of this page (0 if no script has run).
    pub script: usize,
}

pub(crate) enum SlotState {
    Loaded { sheet: Rc<Stylesheet>, media: MediaQueryList },
    Pending { id: u32, media: MediaQueryList },
    Failed,
}

pub(crate) struct Slot {
    /// The `<style>` / `<link>` element this sheet (or an import of it) belongs to.
    owner: NodeId,
    /// Hash of what the owner contributes; a different key means the sheet is rebuilt.
    key: u64,
    state: SlotState,
}

/// A `<script type="module">` waiting to run.
pub(crate) struct ModuleEntry {
    /// Address the module is registered under (a made-up one for inline modules).
    pub(crate) key: String,
    /// Source of an inline module that has not been registered yet.
    pub(crate) inline: Option<String>,
}

/// A classic script waiting to run.
pub(crate) struct PendingScript {
    node: NodeId,
    /// Address shown in error messages.
    url: String,
    /// `None` while an external script is still loading.
    source: Option<String>,
    /// Request id of an external script.
    req: Option<u32>,
    failed: bool,
    /// Position among the scripts the parser found (1, 2, ...); 0 for scripts inserted later.
    seq: u32,
    /// `defer`: runs once every script that blocks the parser has run.
    deferred: bool,
}

pub struct Page {
    pub url: String,
    pub title: String,
    pub(crate) base_url: String,
    pub(crate) join: UrlJoin,
    pub(crate) cfg: PageConfig,
    pub(crate) doc: Document,
    pub(crate) stylist: Stylist,
    pub(crate) styles: StyleStore,
    pub(crate) tree: LayoutTree,
    pub(crate) list: DrawList,
    pub(crate) canvas: Rgba,
    pub(crate) slots: Vec<Slot>,
    /// request id -> URL of that stylesheet (its own base for relative URLs)
    pub(crate) sheet_urls: HashMap<u32, String>,
    pub(crate) requests: Vec<ResourceRequest>,
    pub(crate) next_req: u32,
    pub(crate) requested_images: HashSet<String>,
    pub(crate) image_nodes: HashMap<String, Vec<NodeId>>,
    pub(crate) images: HashMap<NodeId, PaintImage>,
    pub(crate) image_sizes: HashMap<NodeId, (f32, f32)>,
    pub(crate) bg_images: HashMap<String, PaintImage>,
    pub(crate) svg_skipped: HashSet<NodeId>,
    /// What each inline `<svg>` looked like when it was last rasterised: its text colour
    /// (`currentColor`) and box size. A difference means it has to be drawn again.
    pub(crate) svg_state: HashMap<NodeId, (Rgba, f32, f32)>,
    /// Inline `<svg>` elements whose content was changed by script since they were rasterised.
    pub(crate) svg_dirty: HashSet<NodeId>,
    /// Images that were replaced and can be released by the embedder.
    pub(crate) stale_images: Vec<crate::draw::ImageId>,
    /// This page's key in the font system (0 for a page that is only a stand-in).
    pub(crate) font_scope: u32,
    /// `@font-face` rules of the loaded stylesheets.
    pub(crate) font_rules: Vec<WebFontRule>,
    /// Font files that have been asked for (by address), and the request ids still open.
    pub(crate) fonts_requested: HashSet<String>,
    pub(crate) font_reqs: HashMap<u32, usize>,
    pub(crate) form_values: HashMap<NodeId, String>,
    pub(crate) select_values: HashMap<NodeId, String>,
    pub(crate) focus: NodeId,
    pub(crate) hover: Vec<NodeId>,
    pub(crate) target: NodeId,
    pub(crate) caret_on: bool,
    pub(crate) viewport: (f32, f32),
    pub(crate) scroll: (f32, f32),
    pub(crate) sheets_dirty: bool,
    pub(crate) dirty_style: bool,
    pub(crate) dirty_layout: bool,
    pub(crate) dirty_paint: bool,
    pub(crate) laid_out: bool,
    /// Milliseconds the last style, layout and paint passes took (diagnostics).
    pub(crate) pass_ms: [f32; 3],
    /// Layout or the scroll position changed since scripts' observers were last run.
    pub(crate) geom_changed: bool,
    /// Transitions, CSS animations and script animations.
    pub(crate) timeline: crate::anim::Timeline,
    /// The values those animations currently lay over the styles.
    pub(crate) anim_over: crate::style::Overrides,
    // -- scripting --
    pub(crate) script: Option<Box<ScriptEngine>>,
    /// Scripts that run in document order.
    pub(crate) scripts: VecDeque<PendingScript>,
    /// Scripts that run whenever they arrive (`async`, or inserted by another script).
    pub(crate) loose_scripts: Vec<PendingScript>,
    /// Counts the scripts the parser found, for their document order.
    pub(crate) script_seq: u32,
    /// `<canvas>` elements that have a drawing context.
    pub(crate) canvases: HashMap<NodeId, canvas::CanvasSlot>,
    /// The embedder's GPU device for WebGL contexts.
    pub(crate) gl_device: Option<Rc<RefCell<dyn invisium_draw::gl::GlDevice>>>,
    /// Identifies this page object among all pages (see `Page::id`).
    pub(crate) id: u32,
    /// The documents shown in this page's `<iframe>` elements.
    pub(crate) frames: HashMap<NodeId, frames::Frame>,
    pub(crate) frame_requests: Vec<frames::FrameRequest>,
    /// What each `<iframe>` was last asked to show (to notice a new `src`).
    pub(crate) frame_srcs: HashMap<NodeId, String>,
    /// How many documents this one is framed in, and their addresses.
    pub(crate) frame_depth: u8,
    pub(crate) ancestors: Vec<String>,
    /// Messages this document's scripts sent to other documents, not yet delivered.
    pub(crate) outbox: Vec<frames::FrameMsg>,
    /// The frame that has the keyboard, and the one under the pointer (`NO_NODE` = none).
    pub(crate) focus_frame: NodeId,
    pub(crate) hover_frame: NodeId,
    /// This page's glyphs followed by its frames' (what the spliced display list indexes).
    pub(crate) glyph_buf: Vec<Glyph>,
    /// The document came over a direct connection to a local address: the embedder copies
    /// `Response::served_local` here for frames and passes it back on the frame's requests.
    pub served_local: bool,
    pub(crate) seen_scripts: HashSet<NodeId>,
    /// 0 = loading, 1 = `DOMContentLoaded` fired, 2 = `load` fired.
    pub(crate) lifecycle: u8,
    pub(crate) console: Vec<ConsoleMessage>,
    /// A navigation or submission requested by script, for the embedder to pick up.
    pub(crate) pending_action: Option<PageAction>,
    /// Every file the user has picked on this page, by id (script `File` objects refer to these
    /// ids, so a file stays readable after its control is cleared).
    pub(crate) picked: Vec<Rc<PickedFile>>,
    /// Current selection of each file control: indices into `picked`.
    pub(crate) file_values: HashMap<NodeId, Vec<u32>>,
    pub(crate) clock_ms: f64,
    /// Value of the focused text control when it received focus (for `change`).
    pub(crate) focus_value: Option<String>,
    pub(crate) script_redraw: bool,
    pub(crate) fetches: Vec<ScriptFetch>,
    pub(crate) fetch_aborts: Vec<u32>,
    pub(crate) live_fetches: HashSet<u32>,
    pub(crate) cookie_access: Option<CookieAccess>,
    /// The embedder's font system, shared so that layout can be brought up to date while a
    /// script is running (see `force_layout`).
    pub(crate) fonts: Option<Rc<RefCell<FontSystem>>>,
    /// How often a script forced layout (diagnostics).
    pub(crate) forced_layouts: u32,
    /// Native stack the script engine may use (0 = its conservative default).
    pub(crate) script_stack: usize,
    // -- script modules --
    /// `<script type="module">` elements in document order, waiting for their graphs.
    pub(crate) module_queue: VecDeque<ModuleEntry>,
    /// request id -> address of a module source being fetched
    pub(crate) module_reqs: HashMap<u32, String>,
    pub(crate) module_fetching: HashSet<String>,
    /// Module sources that arrived (or failed: `None`) and are not registered yet.
    pub(crate) module_arrived: Vec<(String, Option<String>)>,
    pub(crate) module_failed: HashSet<String>,
    /// Dynamic `import()` requests waiting for their graphs: (request id, address).
    pub(crate) imports: Vec<(u32, String)>,
    pub(crate) module_seq: u32,
}

/// Decode a document's bytes to text. `charset` is the HTTP `Content-Type` charset, if any;
/// otherwise a `<meta charset>` near the top is honoured. UTF-8 and Windows-1252/Latin-1 are
/// supported; other legacy encodings fall back to lossy UTF-8.
pub fn decode_text(bytes: &[u8], charset: Option<&str>) -> String {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    let sniffed: Option<String> = if charset.is_none() {
        let head = &bytes[..bytes.len().min(2048)];
        let lower = String::from_utf8_lossy(head).to_ascii_lowercase();
        lower.find("charset=").map(|p| {
            lower[p + 8..].trim_start_matches(['"', '\'', ' ']).chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect()
        })
    } else {
        None
    };
    let cs = charset.map(|c| c.trim().to_ascii_lowercase()).or(sniffed).unwrap_or_default();
    let text = decode_bytes(bytes, &cs, true);
    if text.contains('\r') {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        text
    }
}

/// Bytes to text without touching line endings (script-visible bodies must arrive as sent).
/// `label` is a lower-case charset label; a UTF-8 byte order mark is dropped. With `guess`,
/// an unlabelled body that is not valid UTF-8 is read as windows-1252.
pub(crate) fn decode_bytes(bytes: &[u8], label: &str, guess: bool) -> String {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    let latin = matches!(label, "iso-8859-1" | "latin1" | "windows-1252" | "cp1252" | "us-ascii" | "ascii" | "iso8859-1" | "iso-8859-15");
    if latin || (guess && label.is_empty() && std::str::from_utf8(bytes).is_err()) {
        const C1: [char; 32] = [
            '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}', '\u{90}', '‘', '’', '“',
            '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}', 'ž', 'Ÿ',
        ];
        bytes.iter().map(|&b| if (0x80..0xA0).contains(&b) { C1[(b - 0x80) as usize] } else { b as char }).collect()
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// A `multipart/form-data` body for the form entries; returns it with its boundary.
fn multipart_body(pairs: &[(String, String)], files: &[(usize, Option<Rc<PickedFile>>)]) -> (Vec<u8>, String) {
    // the boundary only has to be absent from the body; a clock-derived one is enough here
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let mut boundary = format!("----InvisiumFormBoundary{seed:032x}");
    let in_body = |b: &str| {
        let needle = b.as_bytes();
        pairs.iter().any(|(k, v)| k.contains(b) || v.contains(b))
            || files.iter().any(|(_, f)| f.as_ref().map_or(false, |f| f.bytes.windows(needle.len()).any(|w| w == needle)))
    };
    while in_body(&boundary) {
        boundary.push('x');
    }
    let quote = |s: &str| s.replace('"', "%22").replace('\r', "%0D").replace('\n', "%0A");
    let mut out = Vec::new();
    for (i, (name, value)) in pairs.iter().enumerate() {
        out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        match files.iter().find(|(at, _)| *at == i) {
            Some((_, file)) => {
                let (fname, mime, bytes): (&str, &str, &[u8]) = match file {
                    Some(f) => (&f.name, if f.mime.is_empty() { "application/octet-stream" } else { &f.mime }, &f.bytes),
                    None => ("", "application/octet-stream", &[]),
                };
                out.extend_from_slice(
                    format!("Content-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\nContent-Type: {mime}\r\n\r\n", quote(name), quote(fname)).as_bytes(),
                );
                out.extend_from_slice(bytes);
            }
            None => {
                out.extend_from_slice(format!("Content-Disposition: form-data; name=\"{}\"\r\n\r\n", quote(name)).as_bytes());
                out.extend_from_slice(value.as_bytes());
            }
        }
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (out, boundary)
}

/// The `@font-face` rules of a stylesheet (also those inside `@media` blocks), each with the
/// first source this engine can decode.
fn collect_font_rules(rules: &[Rule], out: &mut Vec<WebFontRule>) {
    for r in rules {
        match r {
            Rule::FontFace(ff) => {
                let usable = |format: &Option<String>| match format.as_deref() {
                    None => true,
                    Some(f) => matches!(f, "woff2" | "woff2-variations" | "truetype" | "opentype" | "truetype-variations" | "opentype-variations"),
                };
                let source = ff.sources.iter().find(|(url, format)| {
                    let path = url.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase();
                    usable(format) && !(format.is_none() && (path.ends_with(".woff") || path.ends_with(".eot") || path.ends_with(".svg")))
                });
                if let Some((url, _)) = source {
                    if out.len() < 512 && !ff.family.is_empty() {
                        out.push(WebFontRule {
                            family: Atom::new(&ff.family.to_ascii_lowercase()),
                            url: url.clone(),
                            weight: ff.weight,
                            italic: ff.italic,
                            ranges: ff.ranges.clone(),
                        });
                    }
                }
            }
            Rule::Media { rules, .. } => collect_font_rules(rules, out),
            _ => {}
        }
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        self.drop_canvases();
        // the fonts this page loaded go with it
        if self.font_scope != 0 && !self.fonts_requested.is_empty() {
            if let Some(fonts) = &self.fonts {
                if let Ok(mut f) = fonts.try_borrow_mut() {
                    f.drop_scope(self.font_scope);
                }
            }
        }
    }
}

fn form_encode(s: &str, out: &mut String) {
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
}

/// Pick a URL from a `srcset` attribute: the 1x candidate, else the first.
fn srcset_pick(srcset: &str) -> Option<&str> {
    let mut first = None;
    for cand in srcset.split(',') {
        let mut parts = cand.split_whitespace();
        let Some(url) = parts.next() else { continue };
        let desc = parts.next().unwrap_or("1x");
        if first.is_none() {
            first = Some(url);
        }
        if desc == "1x" {
            return Some(url);
        }
    }
    first
}

impl Page {
    /// Parse `html` and discover its subresources. Call `take_requests` afterwards.
    pub fn new(url: &str, html_src: &str, join: UrlJoin, viewport: (f32, f32), cfg: PageConfig) -> Page {
        let doc = html::parse(html_src, cfg.scripting);
        let mut p = Page::blank(url, join, viewport, cfg, doc);
        p.discover();
        p
    }

    fn blank(url: &str, join: UrlJoin, viewport: (f32, f32), cfg: PageConfig, doc: Document) -> Page {
        Page {
            url: url.to_string(),
            title: String::new(),
            base_url: url.to_string(),
            join,
            cfg,
            doc,
            stylist: Stylist::new(),
            styles: StyleStore::empty(),
            tree: LayoutTree::empty(),
            list: DrawList::new(),
            canvas: Rgba::WHITE,
            slots: Vec::new(),
            sheet_urls: HashMap::new(),
            requests: Vec::new(),
            next_req: 1,
            requested_images: HashSet::new(),
            image_nodes: HashMap::new(),
            images: HashMap::new(),
            image_sizes: HashMap::new(),
            bg_images: HashMap::new(),
            svg_skipped: HashSet::new(),
            svg_state: HashMap::new(),
            svg_dirty: HashSet::new(),
            stale_images: Vec::new(),
            font_scope: NEXT_FONT_SCOPE.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            font_rules: Vec::new(),
            fonts_requested: HashSet::new(),
            font_reqs: HashMap::new(),
            form_values: HashMap::new(),
            select_values: HashMap::new(),
            focus: NO_NODE,
            hover: Vec::new(),
            target: NO_NODE,
            caret_on: true,
            viewport,
            scroll: (0.0, 0.0),
            sheets_dirty: true,
            dirty_style: true,
            dirty_layout: true,
            dirty_paint: true,
            laid_out: false,
            pass_ms: [0.0; 3],
            geom_changed: false,
            timeline: crate::anim::Timeline::default(),
            anim_over: HashMap::new(),
            script: None,
            scripts: VecDeque::new(),
            loose_scripts: Vec::new(),
            script_seq: 0,
            canvases: HashMap::new(),
            gl_device: None,
            id: frames::next_page_id(),
            frames: HashMap::new(),
            frame_requests: Vec::new(),
            frame_srcs: HashMap::new(),
            frame_depth: 0,
            ancestors: Vec::new(),
            outbox: Vec::new(),
            focus_frame: NO_NODE,
            hover_frame: NO_NODE,
            glyph_buf: Vec::new(),
            served_local: false,
            seen_scripts: HashSet::new(),
            lifecycle: 0,
            console: Vec::new(),
            pending_action: None,
            picked: Vec::new(),
            file_values: HashMap::new(),
            clock_ms: 0.0,
            focus_value: None,
            script_redraw: false,
            fetches: Vec::new(),
            fetch_aborts: Vec::new(),
            live_fetches: HashSet::new(),
            cookie_access: None,
            fonts: None,
            forced_layouts: 0,
            script_stack: 0,
            module_queue: VecDeque::new(),
            module_reqs: HashMap::new(),
            module_fetching: HashSet::new(),
            module_arrived: Vec::new(),
            module_failed: HashSet::new(),
            imports: Vec::new(),
            module_seq: 0,
        }
    }

    /// A page with nothing in it: what stays behind while the real page is with the script
    /// host. Cheap to make (no parsing, no allocation worth mentioning).
    pub(crate) fn hollow(join: UrlJoin) -> Page {
        Page::blank("", join, (0.0, 0.0), PageConfig::default(), Document { nodes: Vec::new(), quirks: false })
    }

    /// Share the font system with the page so that scripts get up-to-date geometry: when a
    /// script changes the document and then measures something, layout is redone on the spot.
    /// The embedder keeps using the same `FontSystem` for `update`; it must not hold a borrow
    /// of it while calling page methods that can run script.
    pub fn set_fonts(&mut self, fonts: Rc<RefCell<FontSystem>>) {
        self.fonts = Some(fonts);
    }

    /// Tell the script engine how much native stack it may use. Chains of DOM callbacks (a
    /// custom element's `connectedCallback` inserting children whose callbacks insert more)
    /// nest natively; the engine's default budget suits a 2 MiB thread. Call this before the
    /// page runs its first script, with a size the calling thread really has to spare.
    pub fn set_script_stack(&mut self, bytes: usize) {
        self.script_stack = bytes;
    }

    /// How many times scripts forced layout on this page.
    pub fn forced_layouts(&self) -> u32 {
        self.forced_layouts
    }

    /// Bring style and layout up to date in the middle of a script, because the script asked
    /// for geometry after changing the document. Returns true if layout was redone.
    pub(crate) fn force_layout(&mut self, fx: &mut crate::script::Effects) -> bool {
        let Some(fonts) = self.fonts.clone() else { return false };
        if std::mem::take(&mut fx.sheets_changed) {
            self.sync_sheets();
        }
        self.dirty_style |= std::mem::take(&mut fx.dirty_style);
        self.dirty_layout |= std::mem::take(&mut fx.dirty_layout);
        if !(self.dirty_style || self.dirty_layout) {
            return false;
        }
        let Ok(mut fonts) = fonts.try_borrow_mut() else { return false };
        self.update_style_layout(&mut fonts);
        self.forced_layouts += 1;
        true
    }

    pub(crate) fn resolve(&self, reference: &str) -> Option<String> {
        (self.join)(&self.base_url, reference.trim())
    }

    fn request(&mut self, url: String, kind: ResKind) -> u32 {
        let id = self.next_req;
        self.next_req += 1;
        self.requests.push(ResourceRequest { id, url, kind });
        id
    }

    fn request_image(&mut self, url: String, node: NodeId) {
        if !self.cfg.load_images {
            return;
        }
        if node != NO_NODE {
            let nodes = self.image_nodes.entry(url.clone()).or_default();
            if !nodes.contains(&node) {
                nodes.push(node);
            }
            // already downloaded for another element (or an earlier `src`): use it right away
            if let Some(img) = self.bg_images.get(&url).copied() {
                self.images.insert(node, img);
                self.image_sizes.insert(node, (img.width, img.height));
                self.dirty_layout = true;
            }
        }
        if self.requested_images.insert(url.clone()) {
            self.request(url, ResKind::Image);
        }
    }

    fn add_inline_sheet(&mut self, css: &str, sheet_url: &str, media: MediaQueryList, at: Option<usize>, owner: NodeId, key: u64) {
        let join = self.join.clone();
        let base = sheet_url.to_string();
        let sheet = parse_stylesheet(css, Origin::Author, &|u| join(&base, u));
        // @import rules load before the sheet that imports them
        let mut imports: Vec<(String, MediaQueryList)> = Vec::new();
        for r in &sheet.rules {
            if let Rule::Import { url, media } = r {
                imports.push((url.clone(), media.clone()));
            }
        }
        let mut pos = at.unwrap_or(self.slots.len());
        for (url, m) in imports {
            if self.sheet_urls.len() > 64 {
                break; // import bombs
            }
            let id = self.request(url.clone(), ResKind::Stylesheet);
            self.sheet_urls.insert(id, url);
            self.slots.insert(pos, Slot { owner, key, state: SlotState::Pending { id, media: m } });
            pos += 1;
        }
        let slot = Slot { owner, key, state: SlotState::Loaded { sheet: Rc::new(sheet), media } };
        if pos >= self.slots.len() {
            self.slots.push(slot);
        } else {
            self.slots.insert(pos, slot);
        }
        self.sheets_dirty = true;
        self.dirty_style = true;
    }

    fn discover(&mut self) {
        let nodes: Vec<NodeId> = self.doc.descendants(DOCUMENT_NODE).collect();
        // <base href> first: it changes how everything else resolves
        for &n in &nodes {
            if self.doc.tag(n) == a::BASE {
                if let Some(href) = self.doc.attr(n, a::HREF) {
                    if let Some(abs) = (self.join)(&self.url, href) {
                        self.base_url = abs;
                    }
                }
                break;
            }
        }
        if let Some(frag) = self.url.split_once('#').map(|x| x.1.to_string()) {
            self.target = self.doc.element_by_id(&frag);
        }
        self.sync_sheets();
        self.retitle();
        for &n in &nodes {
            self.discover_node(n, true);
        }
    }

    /// Recompute the page title from the first `<title>` outside SVG.
    fn retitle(&mut self) {
        let t = self.doc.descendants(DOCUMENT_NODE).find(|&n| self.doc.tag(n) == a::TITLE && self.doc.closest(n, a::SVG) == NO_NODE);
        self.title = t.map(|n| self.doc.text_content(n).split_whitespace().collect::<Vec<_>>().join(" ")).unwrap_or_default();
    }

    /// The stylesheet elements of the document, in order: (element, content key).
    fn sheet_owners(&self) -> Vec<(NodeId, u64)> {
        use std::hash::{Hash, Hasher};
        let mut out = Vec::new();
        for n in self.doc.descendants(DOCUMENT_NODE) {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            match self.doc.tag(n) {
                a::STYLE if self.doc.closest(n, a::SVG) == NO_NODE => {
                    self.doc.text_content(n).hash(&mut h);
                }
                a::LINK => {
                    let rel = self.doc.attr(n, a::REL).unwrap_or("").to_ascii_lowercase();
                    let is_sheet = rel.split_ascii_whitespace().any(|r| r == "stylesheet") && !rel.contains("alternate");
                    if !is_sheet || self.doc.attr(n, a::DISABLED).is_some() {
                        continue;
                    }
                    let Some(abs) = self.doc.attr(n, a::HREF).filter(|h| !h.trim().is_empty()).and_then(|h| self.resolve(h)) else { continue };
                    abs.hash(&mut h);
                }
                _ => continue,
            }
            self.doc.attr(n, a::MEDIA).unwrap_or("").hash(&mut h);
            out.push((n, h.finish()));
        }
        out
    }

    /// Bring the stylesheet list in line with the document's `<style>` and `<link>` elements:
    /// drop sheets whose element is gone or changed, load new ones, keep document order.
    fn sync_sheets(&mut self) {
        let owners = self.sheet_owners();
        let before = self.slots.len();
        self.slots.retain(|s| owners.iter().any(|o| o.0 == s.owner && o.1 == s.key));
        let mut changed = self.slots.len() != before;
        for &(n, key) in &owners {
            if self.slots.iter().any(|s| s.owner == n) {
                continue;
            }
            changed = true;
            let media = self.doc.attr(n, a::MEDIA).map(|m| crate::css::parser::parse_media_query_list(&tokenize_value(m))).unwrap_or_default();
            if self.doc.tag(n) == a::STYLE {
                let css = self.doc.text_content(n);
                let base = self.base_url.clone();
                self.add_inline_sheet(&css, &base, media, None, n, key);
            } else if let Some(abs) = self.doc.attr(n, a::HREF).and_then(|h| self.resolve(h)) {
                let id = self.request(abs.clone(), ResKind::Stylesheet);
                self.sheet_urls.insert(id, abs);
                self.slots.push(Slot { owner: n, key, state: SlotState::Pending { id, media } });
            }
        }
        if changed {
            let order: HashMap<NodeId, usize> = owners.iter().enumerate().map(|(i, o)| (o.0, i)).collect();
            self.slots.sort_by_key(|s| order.get(&s.owner).copied().unwrap_or(usize::MAX));
            self.sheets_dirty = true;
            self.dirty_style = true;
        }
    }

    /// Resources and scripts of one element. `parser` is true for the initial document scan
    /// (scripts then run in document order) and false for nodes inserted by script.
    fn discover_node(&mut self, n: NodeId, parser: bool) {
        {
            match self.doc.tag(n) {
                a::SCRIPT if self.cfg.scripting => self.discover_script(n, parser),
                a::IFRAME => self.discover_frame(n),
                a::IMG => {
                    let src = self
                        .doc
                        .attr(n, a::SRC)
                        .filter(|s| !s.trim().is_empty())
                        .or_else(|| self.doc.attr(n, a::SRCSET).and_then(srcset_pick))
                        .or_else(|| self.doc.attr(n, a::DATA_SRC));
                    if let Some(abs) = src.and_then(|s| self.resolve(s)) {
                        self.request_image(abs, n);
                    }
                }
                a::INPUT => {
                    if self.doc.attr(n, a::TYPE).map_or(false, |t| t.eq_ignore_ascii_case("image")) {
                        if let Some(abs) = self.doc.attr(n, a::SRC).and_then(|s| self.resolve(s)) {
                            self.request_image(abs, n);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Requests issued since the last call. The embedder fetches them (subject to its policy)
    /// and reports back with `stylesheet_loaded` / `image_ready` / `resource_failed`.
    pub fn take_requests(&mut self) -> Vec<ResourceRequest> {
        std::mem::take(&mut self.requests)
    }

    /// Stylesheets still loading (the embedder may hold the first paint for these briefly).
    pub fn pending_stylesheets(&self) -> usize {
        self.slots.iter().filter(|s| matches!(s.state, SlotState::Pending { .. })).count()
    }

    pub fn stylesheet_loaded(&mut self, id: u32, css: &str) {
        let Some(pos) = self.slots.iter().position(|s| matches!(s.state, SlotState::Pending { id: i, .. } if i == id)) else { return };
        let slot = self.slots.remove(pos);
        let media = match slot.state {
            SlotState::Pending { media, .. } => media,
            _ => MediaQueryList::default(),
        };
        let url = self.sheet_urls.get(&id).cloned().unwrap_or_else(|| self.base_url.clone());
        self.add_inline_sheet(css, &url, media, Some(pos), slot.owner, slot.key);
        if self.script.is_some() && self.doc.tag(slot.owner) == a::LINK {
            self.resource_event(slot.owner, true);
        }
    }

    pub fn resource_failed(&mut self, id: u32) {
        if self.font_reqs.remove(&id).is_some() {
            return;
        }
        if let Some(pos) = self.slots.iter().position(|s| matches!(s.state, SlotState::Pending { id: i, .. } if i == id)) {
            self.slots[pos].state = SlotState::Failed;
            // a stylesheet that failed no longer blocks anything
            self.dirty_style = true;
            let owner = self.slots[pos].owner;
            if self.script.is_some() && self.doc.tag(owner) == a::LINK {
                self.resource_event(owner, false);
            }
        }
        self.script_failed(id);
    }

    /// An image was fetched, decoded and uploaded. `width`/`height` are its intrinsic size.
    pub fn image_ready(&mut self, url: &str, image: PaintImage) {
        if let Some(nodes) = self.image_nodes.get(url) {
            for &n in nodes {
                self.images.insert(n, image);
                self.image_sizes.insert(n, (image.width, image.height));
            }
            self.dirty_layout = true;
        }
        self.bg_images.insert(url.to_string(), image);
        self.dirty_paint = true;
    }

    /// Attach a rasterised image to an element directly (inline `<svg>`).
    pub fn set_node_image(&mut self, node: NodeId, image: PaintImage) {
        if let Some(old) = self.images.insert(node, image) {
            if old.id != image.id {
                self.stale_images.push(old.id);
            }
        }
        if self.doc.is_valid(node) && self.doc.tag(node) == a::SVG {
            self.svg_state.insert(node, (self.color_of(node), image.width, image.height));
            self.svg_dirty.remove(&node);
        }
        self.dirty_paint = true;
    }

    /// Images that were replaced since the last call (an inline `<svg>` drawn again); the
    /// embedder releases them.
    pub fn take_stale_images(&mut self) -> Vec<crate::draw::ImageId> {
        let mut out = std::mem::take(&mut self.stale_images);
        for f in self.frames.values_mut() {
            out.extend(f.page.take_stale_images());
        }
        out
    }

    /// Script changed something inside an inline `<svg>`: it has to be rasterised again.
    pub(crate) fn svg_changed(&mut self, inside: NodeId) {
        let mut n = inside;
        for _ in 0..64 {
            if n == NO_NODE || !self.doc.is_valid(n) {
                return;
            }
            if self.doc.tag(n) == a::SVG {
                self.svg_skipped.remove(&n);
                if self.images.contains_key(&n) && self.svg_dirty.insert(n) {
                    self.dirty_paint = true;
                }
                // keep going: an <svg> can be nested in another one
            }
            n = self.doc.parent(n);
        }
    }

    /// Every image handle this page holds (for releasing GPU memory when the page is dropped).
    pub fn image_ids(&self) -> Vec<crate::draw::ImageId> {
        // a WebGL canvas's image belongs to the GPU device, not to the embedder
        let gl = |n: &NodeId| matches!(self.canvases.get(n).map(|s| &s.ctx), Some(canvas::CanvasCtx::Gl(_)));
        let mut ids: Vec<_> = self.images.iter().filter(|(n, _)| !gl(n)).map(|(_, i)| i.id).chain(self.bg_images.values().map(|i| i.id)).collect();
        // a frame's placeholder is no image; the frames' own images count
        ids.retain(|id| *id < frames::FRAME_IMAGE_BASE);
        for f in self.frames.values() {
            ids.extend(f.page.image_ids());
        }
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    pub fn set_viewport(&mut self, w: f32, h: f32) {
        if (w - self.viewport.0).abs() > 0.01 || (h - self.viewport.1).abs() > 0.01 {
            self.viewport = (w, h);
            self.dirty_style = true;
            if self.script.is_some() {
                self.fire(NO_NODE, "resize", EventInit::default());
            }
        }
    }

    pub fn set_dark(&mut self, dark: bool) {
        if self.cfg.dark != dark {
            self.cfg.dark = dark;
            self.dirty_style = true;
        }
    }

    fn set_caret_own(&mut self, on: bool) {
        if self.caret_on != on && self.focus != NO_NODE {
            self.dirty_paint = true;
        }
        self.caret_on = on;
    }

    pub fn needs_update(&self) -> bool {
        self.dirty_style || self.dirty_layout || self.dirty_paint || self.frames_need_update()
    }

    /// Bring style, layout and the display list up to date. Returns true if the picture changed.
    pub fn update(&mut self, fonts: &mut FontSystem) -> bool {
        if !self.needs_update() {
            return false;
        }
        fonts.set_scope(self.font_scope);
        self.update_style_layout(fonts);
        if !self.frames.is_empty() {
            // a frame is laid out at the size this page's layout gives its element
            if self.update_frames(fonts) {
                self.dirty_paint = true;
            }
            fonts.set_scope(self.font_scope);
        }
        if self.dirty_paint {
            let t0 = std::time::Instant::now();
            let out = paint(
                &mut self.tree,
                PaintInput {
                    doc: &self.doc,
                    styles: &self.styles,
                    fonts,
                    images: &self.images,
                    bg_images: &self.bg_images,
                    form_values: &self.form_values,
                    focus: self.focus,
                    caret_on: self.caret_on,
                    viewport_w: self.viewport.0,
                    viewport_h: self.viewport.1,
                },
            );
            self.list = out.list;
            self.canvas = out.canvas;
            self.dirty_paint = false;
            self.glyph_buf.clear();
            if !self.frames.is_empty() {
                self.splice_frames();
            }
            self.pass_ms[2] = t0.elapsed().as_secs_f32() * 1000.0;
        }
        true
    }

    /// How long the last style, layout and paint passes took, in milliseconds.
    pub fn pass_times(&self) -> [f32; 3] {
        self.pass_ms
    }

    /// Mark styles stale so that the next `update` redoes everything (diagnostics).
    pub fn invalidate(&mut self) {
        self.dirty_style = true;
    }

    /// The style and layout part of `update` (no painting).
    pub(crate) fn update_style_layout(&mut self, fonts: &mut FontSystem) {
        fonts.set_scope(self.font_scope);
        if self.dirty_style {
            let t0 = std::time::Instant::now();
            if self.sheets_dirty {
                self.stylist.sheets.clear();
                self.font_rules.clear();
                for s in &self.slots {
                    if let SlotState::Loaded { sheet, media } = &s.state {
                        self.stylist.sheets.push(SheetEntry { sheet: sheet.clone(), media: media.clone() });
                        collect_font_rules(&sheet.rules, &mut self.font_rules);
                    }
                }
                self.stylist.invalidate();
                self.sheets_dirty = false;
            }
            let join = self.join.clone();
            let base = self.base_url.clone();
            let resolve = move |u: &str| join(&base, u);
            let env = MediaEnv { width: self.viewport.0, height: self.viewport.1, dark: self.cfg.dark };
            let input = StyleInput {
                doc: &self.doc,
                env,
                hover: &self.hover,
                active: &[],
                focus: self.focus,
                target: self.target,
                resolve: &resolve,
                scripting: self.cfg.scripting,
                animated: if self.anim_over.is_empty() { None } else { Some(&self.anim_over) },
            };
            let fresh = self.stylist.compute(&input);
            let old = std::mem::replace(&mut self.styles, fresh);
            self.dirty_style = false;
            // Values that changed may start transitions, which begin at the old value: lay
            // them over the fresh styles before anything is laid out or drawn.
            if self.sync_animations(&old) {
                self.refresh_overrides(self.clock_ms);
                if self.dirty_style {
                    let input = StyleInput {
                        doc: &self.doc,
                        env,
                        hover: &self.hover,
                        active: &[],
                        focus: self.focus,
                        target: self.target,
                        resolve: &resolve,
                        scripting: self.cfg.scripting,
                        animated: if self.anim_over.is_empty() { None } else { Some(&self.anim_over) },
                    };
                    self.styles = self.stylist.compute(&input);
                    self.dirty_style = false;
                }
            }
            drop(old);
            self.request_fonts();
            self.dirty_layout = true;
            self.discover_backgrounds();
            self.pass_ms[0] = t0.elapsed().as_secs_f32() * 1000.0;
        }
        if self.dirty_layout {
            let t0 = std::time::Instant::now();
            self.tree = layout_document(LayoutInput {
                doc: &self.doc,
                styles: &mut self.styles,
                fonts,
                viewport_w: self.viewport.0,
                viewport_h: self.viewport.1,
                image_sizes: &self.image_sizes,
            });
            self.dirty_layout = false;
            self.dirty_paint = true;
            if !self.laid_out {
                self.laid_out = true;
                if self.target != NO_NODE {
                    let target = self.target;
                    self.scroll_to_node(target);
                }
            }
            self.clamp_scroll();
            self.tree.place_sticky(&self.styles, self.scroll, self.viewport);
            self.geom_changed = true;
            self.pass_ms[1] = t0.elapsed().as_secs_f32() * 1000.0;
        }
    }

    /// Ask for the font files of `@font-face` families that some element actually uses and
    /// whose `unicode-range` covers text of this document.
    fn request_fonts(&mut self) {
        if self.font_rules.is_empty() {
            return;
        }
        let pending: Vec<usize> = (0..self.font_rules.len()).filter(|&i| !self.fonts_requested.contains(&self.font_rules[i].url)).collect();
        if pending.is_empty() {
            return;
        }
        let mut used: HashSet<Atom> = HashSet::new();
        for st in &self.styles.styles {
            used.extend(st.font_family.iter().copied());
        }
        // characters outside plain ASCII that the document's text uses (looked at only if a
        // pending rule is for such a range)
        let mut exotic: Option<HashSet<u32>> = None;
        for i in pending {
            let rule = &self.font_rules[i];
            if !used.contains(&rule.family) {
                continue;
            }
            let latin = rule.ranges.is_empty() || rule.ranges.iter().any(|&(a, b)| a <= 0x7e && b >= 0x20);
            if !latin {
                let chars = exotic.get_or_insert_with(|| {
                    let mut set = HashSet::new();
                    let mut budget = 400_000usize;
                    for id in 0..self.doc.nodes.len() as NodeId {
                        if let Some(t) = self.doc.text(id) {
                            for c in t.chars().take(budget) {
                                if c as u32 > 0x7e {
                                    set.insert(c as u32);
                                }
                            }
                            budget = budget.saturating_sub(t.len());
                            if budget == 0 {
                                break;
                            }
                        }
                    }
                    set
                });
                if !chars.iter().any(|c| rule.ranges.iter().any(|&(a, b)| *c >= a && *c <= b)) {
                    continue;
                }
            }
            let url = rule.url.clone();
            self.fonts_requested.insert(url.clone());
            let id = self.request(url, ResKind::Font);
            self.font_reqs.insert(id, i);
        }
    }

    /// A font file asked for with `ResKind::Font` arrived. Returns true if the font is usable
    /// (text set in its family is then laid out again).
    pub fn font_loaded(&mut self, id: u32, bytes: &[u8], fonts: &mut FontSystem) -> bool {
        let Some(i) = self.font_reqs.remove(&id) else { return false };
        let Some(rule) = self.font_rules.get(i) else { return false };
        let ok = fonts.add_web_font(self.font_scope, rule.family.as_str(), rule.weight, rule.italic, &rule.ranges, bytes);
        if ok {
            self.dirty_layout = true;
        }
        ok
    }

    fn discover_backgrounds(&mut self) {
        if !self.cfg.load_images {
            return;
        }
        let mut urls: Vec<String> = Vec::new();
        for st in &self.styles.styles {
            if let Some(layers) = &st.background {
                for l in layers.iter() {
                    if let Image::Url(u) = &l.image {
                        if !self.requested_images.contains(&**u) {
                            urls.push(u.to_string());
                        }
                    }
                }
            }
        }
        urls.sort();
        urls.dedup();
        for u in urls {
            self.request_image(u, NO_NODE);
        }
    }

    /// Drop everything that can be rebuilt (styles, layout, display list). The DOM stays, so the
    /// tab comes back with one `update` call. Used for background tabs.
    pub fn hibernate(&mut self) {
        self.styles = StyleStore::empty();
        self.tree = LayoutTree::empty();
        self.list = DrawList::new();
        self.dirty_style = true;
    }

    // ───────────── what the renderer needs ─────────────

    pub fn display_list(&self) -> &DrawList {
        &self.list
    }
    pub fn glyphs(&self) -> &[Glyph] {
        // with frames, the display list indexes this page's glyphs followed by theirs
        if self.glyph_buf.is_empty() {
            &self.tree.glyphs
        } else {
            &self.glyph_buf
        }
    }
    pub fn canvas_color(&self) -> Rgba {
        self.canvas
    }
    pub fn scroll(&self) -> (f32, f32) {
        self.scroll
    }
    pub fn content_size(&self) -> (f32, f32) {
        (self.tree.width, self.tree.height)
    }
    pub fn viewport(&self) -> (f32, f32) {
        self.viewport
    }
    pub fn document(&self) -> &Document {
        &self.doc
    }
    pub fn layout(&self) -> &LayoutTree {
        &self.tree
    }
    pub fn styles(&self) -> &StyleStore {
        &self.styles
    }

    pub fn memory(&self) -> PageMemory {
        PageMemory {
            dom: self.doc.memory_bytes(),
            styles: self.styles.memory_bytes(),
            layout: self.tree.memory_bytes(),
            display_list: self.list.memory_bytes(),
            nodes: self.doc.nodes.len(),
            boxes: self.tree.boxes.len(),
            unique_styles: self.styles.styles.len(),
            script: self.script.as_ref().map_or(0, |e| e.heap_bytes()),
        }
    }

    // ───────────── scrolling ─────────────

    fn max_scroll(&self) -> (f32, f32) {
        ((self.tree.width - self.viewport.0).max(0.0), (self.tree.height - self.viewport.1).max(0.0))
    }

    fn clamp_scroll(&mut self) {
        let (mx, my) = self.max_scroll();
        self.scroll.0 = self.scroll.0.clamp(0.0, mx);
        self.scroll.1 = self.scroll.1.clamp(0.0, my);
    }

    /// Returns true if the scroll position changed.
    pub fn scroll_by(&mut self, dx: f32, dy: f32) -> bool {
        let (x, y) = (self.scroll.0 + dx, self.scroll.1 + dy);
        self.scroll_to(x, y)
    }

    pub fn scroll_to(&mut self, x: f32, y: f32) -> bool {
        let changed = self.set_scroll(x, y);
        if changed && self.script.is_some() {
            self.fire(DOCUMENT_NODE, "scroll", EventInit::default());
        }
        changed
    }

    /// Move the scroll position without telling script (used from inside layout).
    fn set_scroll(&mut self, x: f32, y: f32) -> bool {
        let before = self.scroll;
        self.scroll = (x, y);
        self.clamp_scroll();
        let changed = before != self.scroll;
        if changed {
            self.after_scroll();
        }
        changed
    }

    /// The scroll position moved: boxes that stick to the viewport move with it.
    pub(crate) fn after_scroll(&mut self) {
        self.geom_changed = true;
        if !self.tree.sticky.is_empty() && !self.dirty_layout && !self.dirty_style {
            if self.tree.place_sticky(&self.styles, self.scroll, self.viewport) {
                self.dirty_paint = true;
            }
        }
    }

    fn scroll_to_node(&mut self, node: NodeId) -> bool {
        let b = self.tree.box_for_node(node);
        if b == NO_BOX {
            return false;
        }
        let (_, y) = self.tree.abs_pos(b);
        self.set_scroll(self.scroll.0, y)
    }

    // ───────────── input ─────────────

    fn hit(&self, x: f32, y: f32) -> BoxId {
        if !self.laid_out {
            return NO_BOX;
        }
        self.tree.hit_test(&self.styles, x + self.scroll.0, y + self.scroll.1, self.scroll.0, self.scroll.1).unwrap_or(NO_BOX)
    }

    fn link_at(&self, node: NodeId) -> Option<(NodeId, String)> {
        let anchor = self.doc.closest(node, a::A);
        if anchor == NO_NODE {
            return None;
        }
        let href = self.doc.attr(anchor, a::HREF)?;
        let abs = self.resolve(href)?;
        Some((anchor, abs))
    }

    /// Pointer moved to viewport coordinates `(x, y)`.
    fn mouse_move_own(&mut self, x: f32, y: f32) -> Hover {
        let b = self.hit(x, y);
        let mut out = Hover::default();
        let mut chain: Vec<NodeId> = Vec::new();
        if b != NO_BOX {
            let bx = &self.tree.boxes[b as usize];
            let node = bx.node;
            let st = self.styles.style(bx.style);
            out.link = self.link_at(node).map(|l| l.1);
            let is_text = matches!(bx.kind, crate::layout::BoxKind::Text);
            out.pointer = Some(match st.cursor {
                Cursor::Pointer => PointerKind::Pointer,
                Cursor::Text => PointerKind::Text,
                Cursor::Auto if out.link.is_some() => PointerKind::Pointer,
                Cursor::Auto if is_text => PointerKind::Text,
                _ => PointerKind::Default,
            });
            let mut n = if self.doc.is_element(node) { node } else { self.doc.parent(node) };
            while n != NO_NODE && n != DOCUMENT_NODE {
                chain.push(n);
                n = self.doc.parent(n);
            }
        }
        if chain != self.hover {
            let old = self.hover.first().copied().unwrap_or(NO_NODE);
            let new = chain.first().copied().unwrap_or(NO_NODE);
            self.hover = chain;
            if self.stylist.has_dynamic_rules() {
                self.dirty_style = true;
                out.changed = true;
            }
            if self.script.is_some() && old != new {
                let init = EventInit { x, y, ..EventInit::default() };
                if old != NO_NODE {
                    self.fire(old, "mouseout", init);
                }
                if new != NO_NODE {
                    self.fire(new, "mouseover", init);
                }
                if self.needs_update() {
                    out.changed = true;
                }
            }
        }
        out
    }

    pub fn mouse_leave(&mut self) -> bool {
        if self.hover.is_empty() {
            return false;
        }
        self.hover.clear();
        if self.stylist.has_dynamic_rules() {
            self.dirty_style = true;
            return true;
        }
        false
    }

    fn control_at(&self, node: NodeId) -> NodeId {
        let mut n = node;
        while n != NO_NODE {
            let t = self.doc.tag(n);
            if matches!(t, a::INPUT | a::TEXTAREA | a::SELECT | a::BUTTON | a::SUMMARY) {
                return n;
            }
            if t == a::LABEL {
                // a label activates the control it names or contains
                if let Some(target) = self.doc.attr(n, a::FOR) {
                    let c = self.doc.element_by_id(target);
                    if c != NO_NODE {
                        return c;
                    }
                }
                return self.doc.descendants(n).find(|&d| matches!(self.doc.tag(d), a::INPUT | a::TEXTAREA | a::SELECT)).unwrap_or(NO_NODE);
            }
            n = self.doc.parent(n);
        }
        NO_NODE
    }

    fn is_text_control(&self, node: NodeId) -> bool {
        match self.doc.tag(node) {
            a::TEXTAREA => true,
            a::INPUT => {
                let ty = self.doc.attr(node, a::TYPE).unwrap_or("text").to_ascii_lowercase();
                !matches!(ty.as_str(), "checkbox" | "radio" | "submit" | "button" | "reset" | "image" | "file" | "hidden" | "range" | "color")
            }
            _ => false,
        }
    }

    fn has_text_focus_own(&self) -> bool {
        self.focus != NO_NODE && self.is_text_control(self.focus)
    }

    fn set_focus(&mut self, node: NodeId) {
        if self.focus != node {
            let prev = self.focus;
            self.focus = node;
            self.caret_on = true;
            self.dirty_paint = true;
            if self.stylist.has_dynamic_rules() {
                self.dirty_style = true;
            }
            if self.script.is_some() {
                // `change` fires when an edited control loses focus
                let edited = prev != NO_NODE && self.focus_value.as_ref().map_or(false, |v| *v != self.current_value(prev));
                if edited {
                    self.fire(prev, "change", EventInit::default());
                }
                self.focus_value = if node != NO_NODE && self.is_text_control(node) { Some(self.current_value(node)) } else { None };
                self.script(|e| e.focus_changed(prev, node));
            }
        }
    }

    /// A primary-button click at viewport coordinates. `new_tab` = ctrl/middle click.
    fn click_own(&mut self, x: f32, y: f32, new_tab: bool) -> PageAction {
        let b = self.hit(x, y);
        if b == NO_BOX {
            self.set_focus(NO_NODE);
            return PageAction::None;
        }
        let node = self.tree.boxes[b as usize].node;
        if self.script.is_some() {
            let target = self.element_of(node);
            let init = EventInit { x, y, ..EventInit::default() };
            self.fire(target, "mousedown", init);
            self.fire(target, "mouseup", init);
            let proceed = self.fire(target, "click", init);
            if let Some(action) = self.pending_action.take() {
                return action;
            }
            if !proceed {
                return if self.needs_update() { PageAction::Redraw } else { PageAction::None };
            }
        }
        let control = self.control_at(node);
        if control != NO_NODE && self.doc.attr(control, a::DISABLED).is_none() {
            let action = self.activate(control);
            let toggled = action == PageAction::Redraw && matches!(self.doc.tag(control), a::INPUT | a::SELECT) && !self.is_text_control(control);
            if toggled && self.script.is_some() {
                self.fire(control, "input", EventInit::default());
                self.fire(control, "change", EventInit::default());
            }
            return self.pending_action.take().unwrap_or(action);
        }
        self.set_focus(NO_NODE);
        if let Some((anchor, url)) = self.link_at(node) {
            // same-document fragment links scroll instead of navigating
            let strip = |u: &str| u.split('#').next().unwrap_or("").to_string();
            if !new_tab && strip(&url) == strip(&self.url) {
                if let Some(frag) = url.split_once('#').map(|p| p.1) {
                    let t = if frag.is_empty() { self.doc.document_element() } else { self.doc.element_by_id(frag) };
                    if t != NO_NODE {
                        self.target = t;
                        self.dirty_style = self.stylist.has_dynamic_rules();
                        self.scroll_to_node(t);
                        return PageAction::Redraw;
                    }
                }
            }
            let blank = self.doc.attr(anchor, a::TARGET).map_or(false, |t| t.eq_ignore_ascii_case("_blank"));
            return PageAction::Navigate { url, new_tab: new_tab || blank };
        }
        if self.needs_update() {
            PageAction::Redraw
        } else {
            PageAction::None
        }
    }

    fn activate(&mut self, control: NodeId) -> PageAction {
        let tag = self.doc.tag(control);
        let ty = self.doc.attr(control, a::TYPE).unwrap_or("").to_ascii_lowercase();
        match tag {
            a::SUMMARY => {
                let details = self.doc.closest(control, a::DETAILS);
                if details != NO_NODE {
                    let open = self.doc.attr(details, a::OPEN).is_some();
                    if let Some(el) = self.doc.element_mut(details) {
                        if open {
                            el.remove_attr(a::OPEN);
                        } else {
                            el.set_attr(a::OPEN, "");
                        }
                    }
                    self.dirty_style = true;
                    return PageAction::Redraw;
                }
                PageAction::None
            }
            a::INPUT if ty == "checkbox" => {
                let checked = self.doc.attr(control, a::CHECKED).is_some();
                if let Some(el) = self.doc.element_mut(control) {
                    if checked {
                        el.remove_attr(a::CHECKED);
                    } else {
                        el.set_attr(a::CHECKED, "");
                    }
                }
                self.dirty_style = true;
                PageAction::Redraw
            }
            a::INPUT if ty == "radio" => {
                let name = self.doc.attr(control, a::NAME).unwrap_or("").to_string();
                let form = self.doc.closest(control, a::FORM);
                let scope = if form != NO_NODE { form } else { DOCUMENT_NODE };
                let group: Vec<NodeId> = self
                    .doc
                    .descendants(scope)
                    .filter(|&n| {
                        self.doc.tag(n) == a::INPUT
                            && self.doc.attr(n, a::TYPE).map_or(false, |t| t.eq_ignore_ascii_case("radio"))
                            && self.doc.attr(n, a::NAME).unwrap_or("") == name
                    })
                    .collect();
                for n in group {
                    if let Some(el) = self.doc.element_mut(n) {
                        el.remove_attr(a::CHECKED);
                    }
                }
                if let Some(el) = self.doc.element_mut(control) {
                    el.set_attr(a::CHECKED, "");
                }
                self.dirty_style = true;
                PageAction::Redraw
            }
            a::INPUT if ty == "file" => self.pick_action(control),
            a::INPUT if matches!(ty.as_str(), "submit" | "image") => self.submit(control, control),
            a::BUTTON if ty.is_empty() || ty == "submit" => self.submit(control, control),
            a::SELECT => {
                // without a popup yet, a click steps to the next option
                let opts: Vec<NodeId> = self.doc.descendants(control).filter(|&n| self.doc.tag(n) == a::OPTION).collect();
                if opts.is_empty() {
                    return PageAction::None;
                }
                let label = |p: &Page, o: NodeId| p.doc.text_content(o).trim().to_string();
                let cur = self.form_values.get(&control).cloned();
                let idx = match &cur {
                    Some(c) => opts.iter().position(|&o| &label(self, o) == c).unwrap_or(0),
                    None => opts.iter().position(|&o| self.doc.attr(o, a::SELECTED).is_some()).unwrap_or(0),
                };
                let next = opts[(idx + 1) % opts.len()];
                let text = label(self, next);
                let value = self.doc.attr(next, a::VALUE).map(|v| v.to_string()).unwrap_or_else(|| text.clone());
                self.form_values.insert(control, text);
                self.select_values.insert(control, value);
                self.set_focus(control);
                self.dirty_paint = true;
                PageAction::Redraw
            }
            _ if self.is_text_control(control) => {
                self.set_focus(control);
                PageAction::Redraw
            }
            _ => PageAction::None,
        }
    }

    /// Committed text typed while a text control has focus.
    fn text_input_own(&mut self, text: &str) -> PageAction {
        if !self.has_text_focus_own() {
            return PageAction::None;
        }
        let node = self.focus;
        let single = self.doc.tag(node) == a::INPUT;
        let max = self.doc.attr(node, a::MAXLENGTH).and_then(|m| m.trim().parse::<usize>().ok()).unwrap_or(1 << 20);
        let initial = self.current_value(node);
        let v = self.form_values.entry(node).or_insert(initial);
        for ch in text.chars() {
            if ch.is_control() && !(ch == '\n' && !single) {
                continue;
            }
            if v.chars().count() < max {
                v.push(ch);
            }
        }
        self.caret_on = true;
        self.dirty_paint = true;
        if self.script.is_some() {
            self.fire(node, "input", EventInit { data: Some(text), ..EventInit::default() });
            if let Some(action) = self.pending_action.take() {
                return action;
            }
        }
        PageAction::Redraw
    }

    fn current_value(&self, node: NodeId) -> String {
        if let Some(v) = self.form_values.get(&node) {
            return v.clone();
        }
        match self.doc.tag(node) {
            a::TEXTAREA => self.doc.text_content(node),
            _ => self.doc.attr(node, a::VALUE).unwrap_or("").to_string(),
        }
    }

    fn backspace_own(&mut self) -> PageAction {
        if !self.has_text_focus_own() {
            return PageAction::None;
        }
        let node = self.focus;
        if !self.fire(node, "keydown", EventInit { key: Some("Backspace"), ..EventInit::default() }) {
            return PageAction::Redraw;
        }
        let mut v = self.current_value(node);
        v.pop();
        self.form_values.insert(node, v);
        self.caret_on = true;
        self.dirty_paint = true;
        if self.script.is_some() {
            self.fire(node, "input", EventInit::default());
            self.fire(node, "keyup", EventInit { key: Some("Backspace"), ..EventInit::default() });
        }
        PageAction::Redraw
    }

    /// Enter pressed: submits the form of a focused single-line input, adds a line to a textarea.
    fn enter_own(&mut self) -> PageAction {
        if !self.has_text_focus_own() {
            return PageAction::None;
        }
        if self.doc.tag(self.focus) == a::TEXTAREA {
            return self.text_input("\n");
        }
        let focus = self.focus;
        if self.script.is_some() {
            let key = EventInit { key: Some("Enter"), ..EventInit::default() };
            let proceed = self.fire(focus, "keydown", key);
            self.fire(focus, "keyup", key);
            if let Some(action) = self.pending_action.take() {
                return action;
            }
            if !proceed {
                return PageAction::Redraw;
            }
        }
        self.submit(focus, NO_NODE)
    }

    fn blur_own(&mut self) {
        self.set_focus(NO_NODE);
    }

    /// Submission started by the user: script gets a `submit` event and may cancel it.
    fn submit(&mut self, from: NodeId, submitter: NodeId) -> PageAction {
        let form = self.doc.closest(from, a::FORM);
        if form == NO_NODE {
            return PageAction::None;
        }
        if self.script.is_some() {
            let proceed = self.fire(form, "submit", EventInit { submitter, ..EventInit::default() });
            if let Some(action) = self.pending_action.take() {
                return action;
            }
            if !proceed {
                return if self.needs_update() { PageAction::Redraw } else { PageAction::None };
            }
        }
        self.submit_now(form, submitter)
    }

    /// Build the submission of `form` without asking script.
    fn submit_now(&mut self, form: NodeId, submitter: NodeId) -> PageAction {
        if form == NO_NODE || self.doc.tag(form) != a::FORM {
            return PageAction::None;
        }
        let mut pairs: Vec<(String, String)> = Vec::new();
        // entries of `pairs` that are files: (index, the file or `None` for "nothing chosen")
        let mut files: Vec<(usize, Option<Rc<PickedFile>>)> = Vec::new();
        let controls: Vec<NodeId> = self.doc.descendants(form).collect();
        for n in controls {
            let tag = self.doc.tag(n);
            if !matches!(tag, a::INPUT | a::TEXTAREA | a::SELECT | a::BUTTON) || self.doc.attr(n, a::DISABLED).is_some() {
                continue;
            }
            let Some(name) = self.doc.attr(n, a::NAME).filter(|s| !s.is_empty()).map(|s| s.to_string()) else { continue };
            match tag {
                a::INPUT => {
                    let ty = self.doc.attr(n, a::TYPE).unwrap_or("text").to_ascii_lowercase();
                    match ty.as_str() {
                        "checkbox" | "radio" => {
                            if self.doc.attr(n, a::CHECKED).is_some() {
                                pairs.push((name, self.doc.attr(n, a::VALUE).unwrap_or("on").to_string()));
                            }
                        }
                        "submit" | "image" | "button" | "reset" => {
                            if n == submitter && ty != "image" {
                                pairs.push((name, self.doc.attr(n, a::VALUE).unwrap_or("").to_string()));
                            }
                        }
                        "file" => {
                            let chosen = self.file_values.get(&n).cloned().unwrap_or_default();
                            if chosen.is_empty() {
                                files.push((pairs.len(), None));
                                pairs.push((name.clone(), String::new()));
                            }
                            for id in chosen {
                                let f = self.picked[id as usize].clone();
                                files.push((pairs.len(), Some(f.clone())));
                                pairs.push((name.clone(), f.name.clone()));
                            }
                        }
                        _ => pairs.push((name, self.current_value(n))),
                    }
                }
                a::TEXTAREA => pairs.push((name, self.current_value(n))),
                a::SELECT => {
                    let v = self.select_values.get(&n).cloned().unwrap_or_else(|| {
                        let opts: Vec<NodeId> = self.doc.descendants(n).filter(|&o| self.doc.tag(o) == a::OPTION).collect();
                        let chosen = opts.iter().copied().find(|&o| self.doc.attr(o, a::SELECTED).is_some()).or(opts.first().copied());
                        chosen
                            .map(|o| self.doc.attr(o, a::VALUE).map(|v| v.to_string()).unwrap_or_else(|| self.doc.text_content(o).trim().to_string()))
                            .unwrap_or_default()
                    });
                    pairs.push((name, v));
                }
                _ => {
                    if n == submitter {
                        pairs.push((name, self.doc.attr(n, a::VALUE).unwrap_or("").to_string()));
                    }
                }
            }
        }
        let mut encoded = String::new();
        for (i, (k, v)) in pairs.iter().enumerate() {
            if i > 0 {
                encoded.push('&');
            }
            form_encode(k, &mut encoded);
            encoded.push('=');
            form_encode(v, &mut encoded);
        }
        let action = self.doc.attr(form, a::ACTION).unwrap_or("").to_string();
        let target = if action.trim().is_empty() { self.url.split('#').next().unwrap_or("").to_string() } else { action };
        let Some(abs) = self.resolve(&target) else { return PageAction::None };
        let post = self.doc.attr(form, a::METHOD).map_or(false, |m| m.eq_ignore_ascii_case("post"));
        let multipart = post && self.doc.attr(form, a::ENCTYPE).map_or(false, |e| e.trim().eq_ignore_ascii_case("multipart/form-data"));
        if multipart {
            let (body, boundary) = multipart_body(&pairs, &files);
            PageAction::Submit { url: abs, body: Some(body), content_type: Some(format!("multipart/form-data; boundary={boundary}")) }
        } else if post {
            PageAction::Submit { url: abs, body: Some(encoded.into_bytes()), content_type: None }
        } else {
            let base = abs.split('#').next().unwrap_or("").split('?').next().unwrap_or("").to_string();
            PageAction::Submit { url: format!("{base}?{encoded}"), body: None, content_type: None }
        }
    }

    fn pick_action(&self, control: NodeId) -> PageAction {
        PageAction::PickFiles {
            control,
            multiple: self.doc.attr(control, a::MULTIPLE).is_some(),
            accept: self.doc.attr(control, a::ACCEPT).unwrap_or("").to_string(),
        }
    }

    /// The answer to `PageAction::PickFiles`: these files become the control's selection
    /// (replacing an earlier one) and scripts hear `input` and `change`.
    pub fn files_chosen(&mut self, control: NodeId, files: Vec<PickedFile>) {
        let is_file_control = self.doc.is_valid(control)
            && self.doc.tag(control) == a::INPUT
            && self.doc.attr(control, a::TYPE).map_or(false, |t| t.eq_ignore_ascii_case("file"));
        if !is_file_control || files.is_empty() {
            return;
        }
        let multiple = self.doc.attr(control, a::MULTIPLE).is_some();
        let mut ids = Vec::new();
        for f in files.into_iter().take(if multiple { usize::MAX } else { 1 }) {
            ids.push(self.picked.len() as u32);
            self.picked.push(Rc::new(f));
        }
        self.file_values.insert(control, ids.clone());
        // what the control shows next to its button
        let shown = ids.iter().map(|&i| self.picked[i as usize].name.as_str()).collect::<Vec<_>>().join(", ");
        self.form_values.insert(control, shown);
        self.dirty_paint = true;
        if self.script.is_some() {
            let infos: Vec<(u32, String, String, f64, f64)> = ids
                .iter()
                .map(|&id| {
                    let f = &self.picked[id as usize];
                    (id, f.name.clone(), f.mime.clone(), f.last_modified_ms, f.bytes.len() as f64)
                })
                .collect();
            self.script(|e| e.files_changed(control, &infos));
        }
    }

    /// Names of the files chosen for a file control (what the control shows).
    pub fn chosen_file_names(&self, control: NodeId) -> Vec<&str> {
        self.file_values.get(&control).map_or(Vec::new(), |ids| ids.iter().map(|&i| self.picked[i as usize].name.as_str()).collect())
    }

    /// Inline `<svg>` elements that still need rasterising, with their laid-out size. The
    /// embedder answers each with `set_node_image` or `skip_svg`.
    pub fn svg_elements(&self) -> Vec<(NodeId, f32, f32)> {
        let mut out = Vec::new();
        for b in &self.tree.boxes {
            if b.kind == crate::layout::BoxKind::Replaced
                && self.doc.tag(b.node) == a::SVG
                && b.w > 0.0
                && b.h > 0.0
                && !self.svg_skipped.contains(&b.node)
            {
                // not drawn yet, changed by script, or drawn for another colour or size
                let stale = match (self.images.contains_key(&b.node), self.svg_state.get(&b.node)) {
                    (true, Some(&(color, w, h))) => {
                        self.svg_dirty.contains(&b.node) || color != self.color_of(b.node) || (w - b.w).abs() > 0.5 || (h - b.h).abs() > 0.5
                    }
                    (true, None) => self.svg_dirty.contains(&b.node),
                    (false, _) => true,
                };
                if stale {
                    out.push((b.node, b.w, b.h));
                }
            }
        }
        out
    }

    /// This inline SVG could not be rasterised; stop offering it.
    pub fn skip_svg(&mut self, node: NodeId) {
        self.svg_skipped.insert(node);
    }

    /// Serialise an element subtree back to markup (used to hand inline SVG to the rasteriser).
    pub fn outer_html(&self, node: NodeId) -> String {
        fn esc(s: &str, out: &mut String) {
            for ch in s.chars() {
                match ch {
                    '&' => out.push_str("&amp;"),
                    '<' => out.push_str("&lt;"),
                    '>' => out.push_str("&gt;"),
                    '"' => out.push_str("&quot;"),
                    c => out.push(c),
                }
            }
        }
        fn walk(doc: &Document, n: NodeId, out: &mut String, depth: usize) {
            if depth > 256 {
                return;
            }
            if let Some(t) = doc.text(n) {
                esc(t, out);
                return;
            }
            let Some(el) = doc.element(n) else { return };
            out.push('<');
            out.push_str(el.tag.as_str());
            for at in &el.attrs {
                out.push(' ');
                out.push_str(at.name.as_str());
                out.push_str("=\"");
                esc(&at.value, out);
                out.push('"');
            }
            out.push('>');
            for c in doc.children(n) {
                walk(doc, c, out, depth + 1);
            }
            out.push_str("</");
            out.push_str(el.tag.as_str());
            out.push('>');
        }
        let mut s = String::new();
        walk(&self.doc, node, &mut s, 0);
        s
    }

    /// Text colour in effect for an element (resolves `currentColor` for SVG).
    pub fn color_of(&self, node: NodeId) -> Rgba {
        self.styles.get(node).map_or(Rgba::BLACK, |s| Rgba::new(s.color.r, s.color.g, s.color.b, s.color.a))
    }

    /// `<link rel="icon">` URL, if the page declares one.
    pub fn icon_url(&self) -> Option<String> {
        let icon = Atom::new("icon");
        let _ = icon;
        for n in self.doc.descendants(DOCUMENT_NODE) {
            if self.doc.tag(n) == a::LINK {
                let rel = self.doc.attr(n, a::REL).unwrap_or("").to_ascii_lowercase();
                if rel.split_ascii_whitespace().any(|r| r == "icon") {
                    return self.doc.attr(n, a::HREF).and_then(|h| self.resolve(h));
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn join() -> UrlJoin {
        // minimal resolver for tests: absolute URLs pass through, others are appended to the origin
        Rc::new(|base: &str, r: &str| {
            if r.contains("://") {
                Some(r.to_string())
            } else if let Some(frag) = r.strip_prefix('#') {
                Some(format!("{}#{}", base.split('#').next().unwrap_or(""), frag))
            } else {
                let origin: String = base.splitn(4, '/').take(3).collect::<Vec<_>>().join("/");
                Some(format!("{}/{}", origin, r.trim_start_matches('/')))
            }
        })
    }

    fn page(html: &str) -> (Page, FontSystem) {
        let mut p = Page::new("https://example.test/dir/page", html, join(), (800.0, 600.0), PageConfig::default());
        let mut fonts = FontSystem::new();
        p.update(&mut fonts);
        (p, fonts)
    }

    #[test]
    fn discovers_resources_in_order() {
        let mut p = Page::new(
            "https://example.test/",
            "<title> My  Page </title><link rel=stylesheet href=/a.css><style>@import 'b.css'; p{color:red}</style><img src=x.png><img src=x.png><img srcset='y.png 2x, z.png 1x'>",
            join(),
            (800.0, 600.0),
            PageConfig::default(),
        );
        assert_eq!(p.title, "My Page");
        let reqs = p.take_requests();
        let urls: Vec<_> = reqs.iter().map(|r| (r.kind, r.url.as_str())).collect();
        assert_eq!(
            urls,
            vec![
                (ResKind::Stylesheet, "https://example.test/a.css"),
                (ResKind::Stylesheet, "https://example.test/b.css"),
                (ResKind::Image, "https://example.test/x.png"),
                (ResKind::Image, "https://example.test/z.png"),
            ]
        );
        assert_eq!(p.pending_stylesheets(), 2);
        p.stylesheet_loaded(reqs[0].id, "body{margin:0}");
        p.resource_failed(reqs[1].id);
        assert_eq!(p.pending_stylesheets(), 0);
    }

    #[test]
    fn external_css_applies_and_cascades_in_document_order() {
        let mut p = Page::new(
            "https://example.test/",
            "<link rel=stylesheet href=a.css><style>#x{height:10px}</style><div id=x></div>",
            join(),
            (800.0, 600.0),
            PageConfig::default(),
        );
        let mut fonts = FontSystem::new();
        let reqs = p.take_requests();
        p.update(&mut fonts);
        let h = |p: &Page| p.layout().boxes[p.layout().box_for_node(p.document().element_by_id("x")) as usize].h;
        assert_eq!(h(&p), 10.0);
        // the linked sheet comes first in the document, so the later <style> still wins
        p.stylesheet_loaded(reqs[0].id, "#x{height:50px;width:20px}");
        p.update(&mut fonts);
        assert_eq!(h(&p), 10.0);
        let b = &p.layout().boxes[p.layout().box_for_node(p.document().element_by_id("x")) as usize];
        assert_eq!(b.w, 20.0);
    }

    #[test]
    fn links_and_hover() {
        let (mut p, mut fonts) = page("<style>body{margin:0} a{display:block;height:40px} a:hover{height:60px}</style><a id=l href='/next'>go</a><a href='#bottom'>down</a><div style='height:2000px'></div><p id=bottom>end</p>");
        let h = p.mouse_move(10.0, 10.0);
        assert_eq!(h.link.as_deref(), Some("https://example.test/next"));
        assert_eq!(h.pointer, Some(PointerKind::Pointer));
        assert!(h.changed);
        p.update(&mut fonts);
        let b = &p.layout().boxes[p.layout().box_for_node(p.document().element_by_id("l")) as usize];
        assert_eq!(b.h, 60.0, ":hover restyle");
        assert_eq!(p.click(10.0, 10.0, false), PageAction::Navigate { url: "https://example.test/next".into(), new_tab: false });
        // fragment link scrolls instead of navigating
        assert_eq!(p.click(10.0, 70.0, false), PageAction::Redraw);
        assert!(p.scroll().1 > 1000.0);
    }

    #[test]
    fn forms_submit() {
        let (mut p, _fonts) = page(
            "<style>body{margin:0} input{display:block;height:20px;width:200px;margin:0;border:0;padding:0}</style>\
             <form action='/search'><input name=q><input type=hidden name=h value='a b'><input type=checkbox name=c checked><input type=submit name=go value=Go></form>",
        );
        assert_eq!(p.click(5.0, 5.0, false), PageAction::Redraw);
        assert!(p.has_text_focus());
        p.text_input("rust & c++");
        p.backspace();
        assert_eq!(p.enter(), PageAction::Submit { url: "https://example.test/search?q=rust+%26+c%2B&h=a+b&c=on".into(), body: None, content_type: None });
    }

    #[test]
    fn decodes_legacy_text() {
        assert_eq!(decode_text(b"\xEF\xBB\xBFhi\r\nthere", None), "hi\nthere");
        assert_eq!(decode_text(b"caf\xe9 \x93x\x94", Some("ISO-8859-1")), "café “x”");
        assert_eq!(decode_text(b"<meta charset=windows-1252>\x80", None), "<meta charset=windows-1252>€");
        assert_eq!(decode_text("ünï".as_bytes(), None), "ünï");
    }
}
