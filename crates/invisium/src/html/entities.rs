//! Named character references. The full HTML table has 2,231 names; this is the subset that
//! actually appears on the web (all of HTML 4 plus the common HTML5 additions).

use std::collections::HashMap;
use std::sync::OnceLock;

static ENTITIES: &[(&str, &str)] = &[
    ("amp", "&"), ("lt", "<"), ("gt", ">"), ("quot", "\""), ("apos", "'"), ("nbsp", "\u{a0}"),
    ("iexcl", "¡"), ("cent", "¢"), ("pound", "£"), ("curren", "¤"), ("yen", "¥"), ("brvbar", "¦"),
    ("sect", "§"), ("uml", "¨"), ("copy", "©"), ("ordf", "ª"), ("laquo", "«"), ("not", "¬"),
    ("shy", "\u{ad}"), ("reg", "®"), ("macr", "¯"), ("deg", "°"), ("plusmn", "±"), ("sup2", "²"),
    ("sup3", "³"), ("acute", "´"), ("micro", "µ"), ("para", "¶"), ("middot", "·"), ("cedil", "¸"),
    ("sup1", "¹"), ("ordm", "º"), ("raquo", "»"), ("frac14", "¼"), ("frac12", "½"), ("frac34", "¾"),
    ("iquest", "¿"), ("Agrave", "À"), ("Aacute", "Á"), ("Acirc", "Â"), ("Atilde", "Ã"), ("Auml", "Ä"),
    ("Aring", "Å"), ("AElig", "Æ"), ("Ccedil", "Ç"), ("Egrave", "È"), ("Eacute", "É"), ("Ecirc", "Ê"),
    ("Euml", "Ë"), ("Igrave", "Ì"), ("Iacute", "Í"), ("Icirc", "Î"), ("Iuml", "Ï"), ("ETH", "Ð"),
    ("Ntilde", "Ñ"), ("Ograve", "Ò"), ("Oacute", "Ó"), ("Ocirc", "Ô"), ("Otilde", "Õ"), ("Ouml", "Ö"),
    ("times", "×"), ("Oslash", "Ø"), ("Ugrave", "Ù"), ("Uacute", "Ú"), ("Ucirc", "Û"), ("Uuml", "Ü"),
    ("Yacute", "Ý"), ("THORN", "Þ"), ("szlig", "ß"), ("agrave", "à"), ("aacute", "á"), ("acirc", "â"),
    ("atilde", "ã"), ("auml", "ä"), ("aring", "å"), ("aelig", "æ"), ("ccedil", "ç"), ("egrave", "è"),
    ("eacute", "é"), ("ecirc", "ê"), ("euml", "ë"), ("igrave", "ì"), ("iacute", "í"), ("icirc", "î"),
    ("iuml", "ï"), ("eth", "ð"), ("ntilde", "ñ"), ("ograve", "ò"), ("oacute", "ó"), ("ocirc", "ô"),
    ("otilde", "õ"), ("ouml", "ö"), ("divide", "÷"), ("oslash", "ø"), ("ugrave", "ù"), ("uacute", "ú"),
    ("ucirc", "û"), ("uuml", "ü"), ("yacute", "ý"), ("thorn", "þ"), ("yuml", "ÿ"),
    ("OElig", "Œ"), ("oelig", "œ"), ("Scaron", "Š"), ("scaron", "š"), ("Yuml", "Ÿ"), ("fnof", "ƒ"),
    ("circ", "ˆ"), ("tilde", "˜"),
    ("Alpha", "Α"), ("Beta", "Β"), ("Gamma", "Γ"), ("Delta", "Δ"), ("Epsilon", "Ε"), ("Zeta", "Ζ"),
    ("Eta", "Η"), ("Theta", "Θ"), ("Iota", "Ι"), ("Kappa", "Κ"), ("Lambda", "Λ"), ("Mu", "Μ"),
    ("Nu", "Ν"), ("Xi", "Ξ"), ("Omicron", "Ο"), ("Pi", "Π"), ("Rho", "Ρ"), ("Sigma", "Σ"),
    ("Tau", "Τ"), ("Upsilon", "Υ"), ("Phi", "Φ"), ("Chi", "Χ"), ("Psi", "Ψ"), ("Omega", "Ω"),
    ("alpha", "α"), ("beta", "β"), ("gamma", "γ"), ("delta", "δ"), ("epsilon", "ε"), ("zeta", "ζ"),
    ("eta", "η"), ("theta", "θ"), ("iota", "ι"), ("kappa", "κ"), ("lambda", "λ"), ("mu", "μ"),
    ("nu", "ν"), ("xi", "ξ"), ("omicron", "ο"), ("pi", "π"), ("rho", "ρ"), ("sigmaf", "ς"),
    ("sigma", "σ"), ("tau", "τ"), ("upsilon", "υ"), ("phi", "φ"), ("chi", "χ"), ("psi", "ψ"),
    ("omega", "ω"), ("thetasym", "ϑ"), ("upsih", "ϒ"), ("piv", "ϖ"),
    ("ensp", "\u{2002}"), ("emsp", "\u{2003}"), ("thinsp", "\u{2009}"), ("zwnj", "\u{200c}"),
    ("zwj", "\u{200d}"), ("lrm", "\u{200e}"), ("rlm", "\u{200f}"), ("ndash", "–"), ("mdash", "—"),
    ("lsquo", "‘"), ("rsquo", "’"), ("sbquo", "‚"), ("ldquo", "“"), ("rdquo", "”"), ("bdquo", "„"),
    ("dagger", "†"), ("Dagger", "‡"), ("bull", "•"), ("hellip", "…"), ("permil", "‰"), ("prime", "′"),
    ("Prime", "″"), ("lsaquo", "‹"), ("rsaquo", "›"), ("oline", "‾"), ("frasl", "⁄"), ("euro", "€"),
    ("image", "ℑ"), ("weierp", "℘"), ("real", "ℜ"), ("trade", "™"), ("alefsym", "ℵ"),
    ("larr", "←"), ("uarr", "↑"), ("rarr", "→"), ("darr", "↓"), ("harr", "↔"), ("crarr", "↵"),
    ("lArr", "⇐"), ("uArr", "⇑"), ("rArr", "⇒"), ("dArr", "⇓"), ("hArr", "⇔"),
    ("forall", "∀"), ("part", "∂"), ("exist", "∃"), ("empty", "∅"), ("nabla", "∇"), ("isin", "∈"),
    ("notin", "∉"), ("ni", "∋"), ("prod", "∏"), ("sum", "∑"), ("minus", "−"), ("lowast", "∗"),
    ("radic", "√"), ("prop", "∝"), ("infin", "∞"), ("ang", "∠"), ("and", "∧"), ("or", "∨"),
    ("cap", "∩"), ("cup", "∪"), ("int", "∫"), ("there4", "∴"), ("sim", "∼"), ("cong", "≅"),
    ("asymp", "≈"), ("ne", "≠"), ("equiv", "≡"), ("le", "≤"), ("ge", "≥"), ("sub", "⊂"),
    ("sup", "⊃"), ("nsub", "⊄"), ("sube", "⊆"), ("supe", "⊇"), ("oplus", "⊕"), ("otimes", "⊗"),
    ("perp", "⊥"), ("sdot", "⋅"), ("lceil", "⌈"), ("rceil", "⌉"), ("lfloor", "⌊"), ("rfloor", "⌋"),
    ("lang", "〈"), ("rang", "〉"), ("loz", "◊"), ("spades", "♠"), ("clubs", "♣"), ("hearts", "♥"),
    ("diams", "♦"),
    // common HTML5 additions
    ("AMP", "&"), ("LT", "<"), ("GT", ">"), ("QUOT", "\""), ("COPY", "©"), ("REG", "®"),
    ("hyphen", "‐"), ("dash", "‐"), ("check", "✓"), ("checkmark", "✓"), ("cross", "✗"),
    ("star", "☆"), ("starf", "★"), ("bigstar", "★"), ("phone", "☎"), ("female", "♀"), ("male", "♂"),
    ("num", "#"), ("excl", "!"), ("percnt", "%"), ("lpar", "("), ("rpar", ")"), ("ast", "*"),
    ("plus", "+"), ("comma", ","), ("period", "."), ("sol", "/"), ("colon", ":"), ("semi", ";"),
    ("equals", "="), ("quest", "?"), ("commat", "@"), ("lsqb", "["), ("lbrack", "["), ("bsol", "\\"),
    ("rsqb", "]"), ("rbrack", "]"), ("Hat", "^"), ("lowbar", "_"), ("grave", "`"), ("lcub", "{"),
    ("lbrace", "{"), ("verbar", "|"), ("vert", "|"), ("rcub", "}"), ("rbrace", "}"), ("dollar", "$"),
    ("Tab", "\t"), ("NewLine", "\n"), ("nbsp", "\u{a0}"), ("NoBreak", "\u{2060}"),
    ("ZeroWidthSpace", "\u{200b}"), ("hairsp", "\u{200a}"), ("numsp", "\u{2007}"), ("puncsp", "\u{2008}"),
    ("emsp13", "\u{2004}"), ("emsp14", "\u{2005}"), ("mldr", "…"), ("nldr", "‥"), ("centerdot", "·"),
    ("bullet", "•"), ("caret", "⁁"), ("hybull", "⁃"), ("horbar", "―"), ("Vert", "‖"),
    ("triangleright", "▹"), ("triangleleft", "◃"), ("rtrif", "▸"), ("ltrif", "◂"), ("utrif", "▴"),
    ("dtrif", "▾"), ("rtri", "▹"), ("ltri", "◃"), ("utri", "▵"), ("dtri", "▿"), ("squ", "□"),
    ("squf", "▪"), ("blk14", "░"), ("blk12", "▒"), ("blk34", "▓"), ("block", "█"), ("cir", "○"),
    ("xcirc", "◯"), ("larrhk", "↩"), ("rarrhk", "↪"), ("nearr", "↗"), ("searr", "↘"), ("swarr", "↙"),
    ("nwarr", "↖"), ("varr", "↕"), ("lsh", "↰"), ("rsh", "↱"), ("map", "↦"), ("rarrw", "↝"),
    ("xlarr", "⟵"), ("xrarr", "⟶"), ("xharr", "⟷"), ("Larr", "↞"), ("Rarr", "↠"),
    ("leftarrow", "←"), ("rightarrow", "→"), ("uparrow", "↑"), ("downarrow", "↓"),
    ("LeftArrow", "←"), ("RightArrow", "→"), ("UpArrow", "↑"), ("DownArrow", "↓"),
    ("approx", "≈"), ("ap", "≈"), ("leq", "≤"), ("geq", "≥"), ("neq", "≠"), ("pm", "±"), ("mp", "∓"),
    ("half", "½"), ("frac13", "⅓"), ("frac23", "⅔"), ("frac15", "⅕"), ("frac18", "⅛"),
    ("ell", "ℓ"), ("hbar", "ℏ"), ("planck", "ℏ"), ("angst", "Å"), ("ohm", "Ω"), ("mho", "℧"),
    ("naturals", "ℕ"), ("integers", "ℤ"), ("rationals", "ℚ"), ("reals", "ℝ"), ("complexes", "ℂ"),
    ("setminus", "∖"), ("compfn", "∘"), ("Sqrt", "√"), ("vellip", "⋮"), ("ctdot", "⋯"), ("dtdot", "⋱"),
    ("lt", "<"), ("ll", "≪"), ("gg", "≫"), ("sbquo", "‚"), ("lsquor", "‚"), ("ldquor", "„"),
    ("rdquor", "”"), ("rsquor", "’"), ("OpenCurlyQuote", "‘"), ("CloseCurlyQuote", "’"),
    ("OpenCurlyDoubleQuote", "“"), ("CloseCurlyDoubleQuote", "”"), ("fflig", "ﬀ"), ("filig", "ﬁ"),
    ("fllig", "ﬂ"), ("inodot", "ı"), ("imath", "ı"), ("Scedil", "Ş"), ("scedil", "ş"),
    ("Ccaron", "Č"), ("ccaron", "č"), ("Zcaron", "Ž"), ("zcaron", "ž"), ("Lstrok", "Ł"),
    ("lstrok", "ł"), ("Nacute", "Ń"), ("nacute", "ń"), ("Sacute", "Ś"), ("sacute", "ś"),
    ("Zacute", "Ź"), ("zacute", "ź"), ("Zdot", "Ż"), ("zdot", "ż"), ("Cacute", "Ć"), ("cacute", "ć"),
    ("Eogon", "Ę"), ("eogon", "ę"), ("Aogon", "Ą"), ("aogon", "ą"), ("Rcaron", "Ř"), ("rcaron", "ř"),
    ("Ecaron", "Ě"), ("ecaron", "ě"), ("Uring", "Ů"), ("uring", "ů"), ("Gbreve", "Ğ"), ("gbreve", "ğ"),
    ("Idot", "İ"), ("odblac", "ő"), ("Odblac", "Ő"), ("udblac", "ű"), ("Udblac", "Ű"),
    ("amacr", "ā"), ("emacr", "ē"), ("imacr", "ī"), ("omacr", "ō"), ("umacr", "ū"),
    ("copysr", "℗"), ("sung", "♪"), ("flat", "♭"), ("natural", "♮"), ("sharp", "♯"),
    ("spadesuit", "♠"), ("clubsuit", "♣"), ("heartsuit", "♥"), ("diamondsuit", "♦"),
    ("larrb", "⇤"), ("rarrb", "⇥"), ("cularr", "↶"), ("curarr", "↷"), ("olarr", "↺"), ("orarr", "↻"),
    ("lharu", "↼"), ("rharu", "⇀"), ("rlarr", "⇄"), ("lrarr", "⇆"), ("udarr", "⇅"),
    ("bull", "•"), ("middot", "·"), ("boxh", "─"), ("boxv", "│"), ("boxdr", "┌"), ("boxdl", "┐"),
    ("boxur", "└"), ("boxul", "┘"), ("boxvr", "├"), ("boxvl", "┤"), ("boxhd", "┬"), ("boxhu", "┴"),
    ("boxvh", "┼"),
];

/// The legacy names that are recognised even without a trailing semicolon.
static NO_SEMICOLON: &[&str] = &[
    "amp", "lt", "gt", "quot", "nbsp", "copy", "reg", "AMP", "LT", "GT", "QUOT", "COPY", "REG", "deg",
    "laquo", "raquo", "middot", "para", "sect", "pound", "yen", "cent", "times", "divide", "plusmn",
    "frac12", "frac14", "frac34", "sup1", "sup2", "sup3", "shy", "iexcl", "iquest", "micro", "not",
    "acute", "cedil", "uml", "macr", "ordf", "ordm", "curren", "brvbar", "szlig", "eacute", "egrave",
    "aacute", "agrave", "auml", "ouml", "uuml", "Auml", "Ouml", "Uuml", "ntilde", "ccedil",
];

fn table() -> &'static HashMap<&'static str, &'static str> {
    static T: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    T.get_or_init(|| ENTITIES.iter().copied().collect())
}

pub fn lookup(name: &str) -> Option<&'static str> {
    table().get(name).copied()
}

fn numeric(code: u32) -> char {
    // Windows-1252 remapping for the C1 range, as browsers do.
    const C1: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}',
        '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}', 'ž', 'Ÿ',
    ];
    match code {
        0 => '\u{fffd}',
        0x80..=0x9f => C1[(code - 0x80) as usize],
        _ => char::from_u32(code).unwrap_or('\u{fffd}'),
    }
}

/// Decode character references in `s`. `in_attr` enables the attribute-value rule that leaves
/// `&name` alone when it is followed by `=` or an alphanumeric (so `?a=1&copy=2` survives).
pub fn decode(s: &str, in_attr: bool) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let mut start = 0;
    while i < bytes.len() {
        if bytes[i] != b'&' {
            i += 1;
            continue;
        }
        out.push_str(&s[start..i]);
        let rest = &bytes[i + 1..];
        let mut consumed = 0; // bytes consumed after '&'
        let mut replacement: Option<String> = None;
        if rest.first() == Some(&b'#') {
            let (radix, digits_start) = match rest.get(1) {
                Some(b'x') | Some(b'X') => (16, 2),
                _ => (10, 1),
            };
            let mut j = digits_start;
            let mut code: u32 = 0;
            while j < rest.len() && (rest[j] as char).is_digit(radix) {
                code = code.saturating_mul(radix).saturating_add((rest[j] as char).to_digit(radix).unwrap());
                j += 1;
            }
            if j > digits_start {
                if rest.get(j) == Some(&b';') {
                    j += 1;
                }
                consumed = j;
                replacement = Some(numeric(code).to_string());
            }
        } else {
            let mut j = 0;
            while j < rest.len() && j < 32 && rest[j].is_ascii_alphanumeric() {
                j += 1;
            }
            if j > 0 {
                let name = &s[i + 1..i + 1 + j];
                if rest.get(j) == Some(&b';') {
                    if let Some(v) = lookup(name) {
                        consumed = j + 1;
                        replacement = Some(v.to_string());
                    }
                }
                if replacement.is_none() {
                    // longest legacy prefix without semicolon
                    for len in (2..=j.min(6)).rev() {
                        let prefix = &name[..len];
                        if NO_SEMICOLON.contains(&prefix) {
                            let next = rest.get(len).copied();
                            let blocked =
                                in_attr && next.map_or(false, |c| c == b'=' || c.is_ascii_alphanumeric());
                            if !blocked {
                                consumed = len;
                                replacement = lookup(prefix).map(|v| v.to_string());
                            }
                            break;
                        }
                    }
                }
            }
        }
        match replacement {
            Some(r) => {
                out.push_str(&r);
                i += 1 + consumed;
            }
            None => {
                out.push('&');
                i += 1;
            }
        }
        start = i;
    }
    out.push_str(&s[start..]);
    out
}

#[cfg(test)]
mod tests {
    use super::decode;

    #[test]
    fn decodes() {
        assert_eq!(decode("a &amp; b &lt;c&gt; &#65;&#x42; &nbsp;x", false), "a & b <c> AB \u{a0}x");
        assert_eq!(decode("&copy 2024 &unknown; &", false), "© 2024 &unknown; &");
        assert_eq!(decode("?a=1&copy=2&amp;b=3", true), "?a=1&copy=2&b=3");
        assert_eq!(decode("&#150;&#8212;", false), "–—");
    }
}
