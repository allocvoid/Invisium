//! String interning. Tag names, attribute names, ids and class names are stored once and
//! referred to by a 4-byte handle, so a DOM with 50k elements does not carry 50k copies of "div".

use std::collections::HashMap;
use std::fmt;
use std::sync::{OnceLock, RwLock};

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Atom(pub u32);

struct Interner {
    map: HashMap<&'static str, u32>,
    strs: Vec<&'static str>,
}

fn interner() -> &'static RwLock<Interner> {
    static I: OnceLock<RwLock<Interner>> = OnceLock::new();
    I.get_or_init(|| {
        let mut map = HashMap::with_capacity(a::STATIC_ATOMS.len() * 2);
        let mut strs = Vec::with_capacity(a::STATIC_ATOMS.len() * 2);
        for (i, s) in a::STATIC_ATOMS.iter().enumerate() {
            map.insert(*s, i as u32);
            strs.push(*s);
        }
        RwLock::new(Interner { map, strs })
    })
}

impl Atom {
    pub const NONE: Atom = Atom(u32::MAX);

    pub fn new(s: &str) -> Atom {
        {
            let i = interner().read().unwrap();
            if let Some(&id) = i.map.get(s) {
                return Atom(id);
            }
        }
        let mut i = interner().write().unwrap();
        if let Some(&id) = i.map.get(s) {
            return Atom(id);
        }
        let leaked: &'static str = Box::leak(s.to_owned().into_boxed_str());
        let id = i.strs.len() as u32;
        i.strs.push(leaked);
        i.map.insert(leaked, id);
        Atom(id)
    }

    /// Intern the ASCII-lowercased form of `s`.
    pub fn lower(s: &str) -> Atom {
        if s.bytes().any(|b| b.is_ascii_uppercase()) {
            Atom::new(&s.to_ascii_lowercase())
        } else {
            Atom::new(s)
        }
    }

    pub fn as_str(self) -> &'static str {
        if self == Atom::NONE {
            return "";
        }
        interner().read().unwrap().strs[self.0 as usize]
    }

    pub fn is_none(self) -> bool {
        self == Atom::NONE
    }

    /// Number of distinct strings interned so far (for the memory page).
    pub fn count() -> usize {
        interner().read().unwrap().strs.len()
    }
}

impl fmt::Debug for Atom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl fmt::Display for Atom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

macro_rules! static_atoms {
    ($( $name:ident = $s:expr ),* $(,)?) => {
        #[allow(non_camel_case_types, clippy::upper_case_acronyms)]
        #[repr(u32)]
        enum Idx { $($name),* }
        $( pub const $name: Atom = Atom(Idx::$name as u32); )*
        pub(super) static STATIC_ATOMS: &[&str] = &[$($s),*];
    };
}

/// Well-known names with fixed handles, usable in `match`.
#[allow(non_upper_case_globals)]
pub mod a {
    use super::Atom;
    static_atoms! {
        EMPTY = "",
        // --- elements ---
        HTML = "html", HEAD = "head", BODY = "body", TITLE = "title", META = "meta", LINK = "link",
        STYLE = "style", SCRIPT = "script", NOSCRIPT = "noscript", BASE = "base", TEMPLATE = "template",
        DIV = "div", SPAN = "span", P = "p", A = "a", IMG = "img", BR = "br", HR = "hr", WBR = "wbr",
        H1 = "h1", H2 = "h2", H3 = "h3", H4 = "h4", H5 = "h5", H6 = "h6",
        UL = "ul", OL = "ol", LI = "li", DL = "dl", DT = "dt", DD = "dd", MENU = "menu", DIR = "dir",
        TABLE = "table", THEAD = "thead", TBODY = "tbody", TFOOT = "tfoot", TR = "tr", TD = "td", TH = "th",
        CAPTION = "caption", COLGROUP = "colgroup", COL = "col",
        FORM = "form", INPUT = "input", BUTTON = "button", SELECT = "select", OPTION = "option",
        OPTGROUP = "optgroup", TEXTAREA = "textarea", LABEL = "label", FIELDSET = "fieldset", LEGEND = "legend",
        B = "b", I = "i", U = "u", S = "s", EM = "em", STRONG = "strong", SMALL = "small", BIG = "big",
        CODE = "code", TT = "tt", FONT = "font", STRIKE = "strike", NOBR = "nobr", SUB = "sub", SUP = "sup",
        PRE = "pre", LISTING = "listing", XMP = "xmp", PLAINTEXT = "plaintext", BLOCKQUOTE = "blockquote",
        CENTER = "center", ADDRESS = "address", ARTICLE = "article", ASIDE = "aside", SECTION = "section",
        NAV = "nav", HEADER = "header", FOOTER = "footer", MAIN = "main", FIGURE = "figure",
        FIGCAPTION = "figcaption", DETAILS = "details", SUMMARY = "summary", DIALOG = "dialog", HGROUP = "hgroup",
        IFRAME = "iframe", FRAME = "frame", FRAMESET = "frameset", NOFRAMES = "noframes", NOEMBED = "noembed",
        OBJECT = "object", EMBED = "embed", APPLET = "applet", MARQUEE = "marquee", PARAM = "param",
        VIDEO = "video", AUDIO = "audio", SOURCE = "source", TRACK = "track", CANVAS = "canvas",
        PICTURE = "picture", SVG = "svg", MATH = "math", AREA = "area", MAP = "map",
        RB = "rb", RP = "rp", RT = "rt", RTC = "rtc", RUBY = "ruby", ABBR = "abbr", CITE = "cite",
        Q = "q", KBD = "kbd", SAMP = "samp", VAR = "var", MARK = "mark", DEL = "del", INS = "ins", TIME = "time",
        PATH = "path", G = "g", RECT = "rect", CIRCLE = "circle", ELLIPSE = "ellipse", LINE = "line",
        POLYGON = "polygon", POLYLINE = "polyline", USE = "use", DEFS = "defs", SYMBOL = "symbol",
        // --- attributes ---
        ID = "id", CLASS = "class", HREF = "href", SRC = "src", SRCSET = "srcset", ALT = "alt", REL = "rel",
        TYPE = "type", NAME = "name", VALUE = "value", CONTENT = "content", CHARSET = "charset",
        HTTP_EQUIV = "http-equiv", MEDIA = "media", WIDTH = "width", HEIGHT = "height", ALIGN = "align",
        VALIGN = "valign", BGCOLOR = "bgcolor", COLOR = "color", FACE = "face", SIZE = "size",
        BORDER = "border", CELLPADDING = "cellpadding", CELLSPACING = "cellspacing", COLSPAN = "colspan",
        ROWSPAN = "rowspan", NOWRAP = "nowrap", HIDDEN = "hidden", OPEN = "open", CHECKED = "checked",
        DISABLED = "disabled", SELECTED = "selected", PLACEHOLDER = "placeholder", ACTION = "action",
        METHOD = "method", TARGET = "target", START = "start", REVERSED = "reversed", TEXT = "text",
        VLINK = "vlink", ALINK = "alink", BACKGROUND = "background", LANG = "lang", DATA_SRC = "data-src",
        LOADING = "loading", SIZES = "sizes", AS = "as", FOR = "for", READONLY = "readonly",
        MULTIPLE = "multiple", ROWS = "rows", COLS = "cols", MAXLENGTH = "maxlength", ROLE = "role",
        VIEWBOX = "viewbox", D = "d", FILL = "fill", STROKE = "stroke", STROKE_WIDTH = "stroke-width",
        X = "x", Y = "y", CX = "cx", CY = "cy", R = "r", RX = "rx", RY = "ry", X1 = "x1", Y1 = "y1",
        X2 = "x2", Y2 = "y2", POINTS = "points", TRANSFORM = "transform", HSPACE = "hspace", VSPACE = "vspace",
        ENCTYPE = "enctype", AUTOFOCUS = "autofocus", TABINDEX = "tabindex", DATA = "data", POSTER = "poster",
        FILL_RULE = "fill-rule", STROKE_LINECAP = "stroke-linecap", STROKE_LINEJOIN = "stroke-linejoin",
        OPACITY = "opacity", XLINK_HREF = "xlink:href",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_atoms_resolve() {
        assert_eq!(a::DIV.as_str(), "div");
        assert_eq!(Atom::new("div"), a::DIV);
        assert_eq!(Atom::lower("DIV"), a::DIV);
        assert_eq!(a::XLINK_HREF.as_str(), "xlink:href");
        let x = Atom::new("some-class-name");
        assert_eq!(x.as_str(), "some-class-name");
        assert_eq!(Atom::new("some-class-name"), x);
    }
}
