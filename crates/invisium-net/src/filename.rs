//! Suggested download file names: taken from `Content-Disposition` or the URL and made safe to
//! create on any desktop file system.

use crate::types::Headers;
use crate::url::{percent_decode, Url};

const MAX_BYTES: usize = 150;

/// Whether `Content-Disposition` says `attachment`.
pub(crate) fn is_attachment(headers: &Headers) -> bool {
    headers.get("content-disposition").is_some_and(|v| {
        v.split(';').next().is_some_and(|t| t.trim().eq_ignore_ascii_case("attachment"))
    })
}

/// Splits `Content-Disposition` parameters, honouring quoted strings.
fn params(v: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut parts = Vec::new();
    let (mut cur, mut quoted, mut escape) = (String::new(), false, false);
    for c in v.chars() {
        if escape {
            cur.push(c);
            escape = false;
            continue;
        }
        match c {
            '\\' if quoted => {
                cur.push(c);
                escape = true;
            }
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            ';' if !quoted => parts.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    parts.push(cur);
    for p in parts.into_iter().skip(1) {
        let Some((k, val)) = p.split_once('=') else { continue };
        let val = val.trim();
        let val = match val.strip_prefix('"').and_then(|x| x.strip_suffix('"')) {
            Some(inner) => {
                // `\"` and `\\` are escapes; any other backslash is kept (servers send
                // Windows paths unescaped), and the path is cut off later anyway.
                let mut s = String::new();
                let mut esc = false;
                for c in inner.chars() {
                    if esc {
                        if c != '"' && c != '\\' {
                            s.push('\\');
                        }
                        s.push(c);
                        esc = false;
                    } else if c == '\\' {
                        esc = true;
                    } else {
                        s.push(c);
                    }
                }
                s
            }
            None => val.to_string(),
        };
        out.push((k.trim().to_ascii_lowercase(), val));
    }
    out
}

/// `filename*=UTF-8''...` (RFC 5987); other charsets are decoded as Latin-1.
fn ext_value(v: &str) -> Option<String> {
    let mut it = v.splitn(3, '\'');
    let charset = it.next()?.to_ascii_lowercase();
    let _lang = it.next()?;
    let bytes = percent_decode(it.next()?);
    match charset.as_str() {
        "utf-8" => String::from_utf8(bytes).ok(),
        "iso-8859-1" => Some(bytes.iter().map(|&b| b as char).collect()),
        _ => None,
    }
}

fn from_disposition(headers: &Headers) -> Option<String> {
    let ps = params(headers.get("content-disposition")?);
    let star = ps.iter().find(|(k, _)| k == "filename*").and_then(|(_, v)| ext_value(v));
    star.or_else(|| ps.into_iter().find(|(k, _)| k == "filename").map(|(_, v)| v))
}

fn from_url(url: &Url) -> Option<String> {
    let last = url.path().rsplit('/').next()?;
    (!last.is_empty()).then(|| String::from_utf8_lossy(&percent_decode(last)).into_owned())
}

/// The safe suggested file name for a response (see the NET contract), never empty.
pub(crate) fn suggested(headers: &Headers, url: &Url) -> String {
    let candidates = [from_disposition(headers), from_url(url)];
    for c in candidates.into_iter().flatten() {
        let s = sanitize(&c);
        if !s.is_empty() {
            return s;
        }
    }
    "download".to_string()
}

const RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9",
    "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9", "conin$", "conout$",
];

fn trim_name(s: &str) -> &str {
    s.trim_matches(|c: char| c == '.' || c == ' ' || c.is_whitespace())
}

/// Makes `raw` a safe single file name; returns "" when nothing usable is left.
pub(crate) fn sanitize(raw: &str) -> String {
    // No directory parts: keep what follows the last separator of either kind.
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let replaced: String = base
        .chars()
        .map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { '_' } else { c })
        .collect();
    let mut name = trim_name(&replaced).to_string();
    if name.len() > MAX_BYTES {
        name = trim_name(&shorten(&name)).to_string();
    }
    // Checked on the final, shortened name, so shortening cannot produce a reserved one.
    if is_reserved(&name) {
        name.insert(0, '_');
        if name.len() > MAX_BYTES {
            let mut cut = MAX_BYTES;
            while !name.is_char_boundary(cut) {
                cut -= 1;
            }
            name.truncate(cut);
            name = trim_name(&name).to_string(); // the leading '_' stays
        }
    }
    name
}

/// A Windows device name, with or without extension: CON, PRN, AUX, NUL, CONIN$, CONOUT$,
/// COM1-9 and LPT1-9, also with the superscript digits ¹ ² ³.
fn is_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").trim_end().to_lowercase();
    if RESERVED.contains(&stem.as_str()) {
        return true;
    }
    ["com", "lpt"].iter().any(|p| stem.strip_prefix(p).is_some_and(|d| matches!(d, "¹" | "²" | "³")))
}

/// Cuts a name to at most `MAX_BYTES`, keeping a short extension.
fn shorten(name: &str) -> String {
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 && name.len() - i <= 16 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    let mut cut = MAX_BYTES - ext.len();
    while !stem.is_char_boundary(cut.min(stem.len())) {
        cut -= 1;
    }
    format!("{}{}", &stem[..cut.min(stem.len())], ext)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(disposition: Option<&str>, url: &str) -> String {
        let mut h = Headers::new();
        if let Some(d) = disposition {
            h.push("Content-Disposition", d);
        }
        suggested(&h, &Url::parse(url).unwrap())
    }

    #[test]
    fn file_names() {
        let u = "https://example.com/files/report.pdf";
        let cases: &[(Option<&str>, &str, &str)] = &[
            (None, u, "report.pdf"),
            (Some("attachment; filename=plain.txt"), u, "plain.txt"),
            (Some("attachment; filename=\"quoted name.txt\""), u, "quoted name.txt"),
            (Some("attachment; filename=\"semi;colon.txt\""), u, "semi;colon.txt"),
            (Some("attachment; filename=\"esc\\\"aped.txt\""), u, "esc_aped.txt"),
            (Some("attachment; filename*=UTF-8''%E2%82%AC%20rates.txt; filename=\"fallback.txt\""), u, "€ rates.txt"),
            (Some("attachment; filename=\"fallback.txt\"; filename*=UTF-8''star.txt"), u, "star.txt"),
            (Some("attachment; filename*=iso-8859-1'en'%E9t%E9.txt"), u, "été.txt"),
            (Some("attachment; filename=\"../../x\""), u, "x"),
            (Some("attachment; filename=\"C:\\x\""), u, "x"),
            (Some("attachment; filename=C:\\x"), u, "x"),
            (Some("attachment; filename=\"C:\\\\dir\\\\y.txt\""), u, "y.txt"),
            (Some("attachment; filename=\"..\\..\\windows\\system32\\evil.dll\""), u, "evil.dll"),
            (Some("attachment; filename=\"CON\""), u, "_CON"),
            (Some("attachment; filename=\"nul.txt\""), u, "_nul.txt"),
            (Some("attachment; filename=\"lpt9.tar.gz\""), u, "_lpt9.tar.gz"),
            (Some("attachment; filename=\"com1 .txt\""), u, "_com1 .txt"),
            (Some("attachment; filename=\"CONIN$\""), u, "_CONIN$"),
            (Some("attachment; filename=\"conout$.log\""), u, "_conout$.log"),
            (Some("attachment; filename*=UTF-8''COM%C2%B9.txt"), u, "_COM¹.txt"),
            (Some("attachment; filename*=UTF-8''lpt%C2%B2"), u, "_lpt²"),
            (Some("attachment; filename*=UTF-8''LPT%C2%B3.tar.gz"), u, "_LPT³.tar.gz"),
            (Some("attachment; filename*=UTF-8''com%E2%81%B4.txt"), u, "com⁴.txt"),
            (Some("attachment; filename=\"conin.txt\""), u, "conin.txt"),
            (Some("attachment; filename=\"a\u{1}b\u{7f}c.txt\""), u, "a_b_c.txt"),
            (Some("attachment; filename=\"what?<is>|this*.txt\""), u, "what__is__this_.txt"),
            (Some("attachment; filename=\"  ..hidden.. \""), u, "hidden"),
            (Some("attachment; filename=\"\""), u, "report.pdf"),
            (Some("attachment; filename=\"...\""), "https://example.com/", "download"),
            (None, "https://example.com/dir/", "download"),
            (None, "https://example.com/", "download"),
            (None, "https://example.com/a%20b%2Fc.txt", "c.txt"),
            (None, "https://example.com/con", "_con"),
            (Some("inline"), "https://example.com/x.png?q=1", "x.png"),
        ];
        for (d, url, want) in cases {
            assert_eq!(name(*d, url), *want, "{d:?} {url}");
        }
    }

    #[test]
    fn long_names_keep_their_extension() {
        let long = format!("attachment; filename=\"{}.pdf\"", "a".repeat(400));
        let n = name(Some(&long), "https://example.com/");
        assert_eq!(n.len(), MAX_BYTES);
        assert!(n.ends_with(".pdf"));
        let wide = format!("attachment; filename*=UTF-8''{}.txt", "%E2%82%AC".repeat(100));
        let n = name(Some(&wide), "https://example.com/");
        assert!(n.len() <= MAX_BYTES && n.ends_with(".txt") && n.starts_with('€'));
    }

    #[test]
    fn shortening_never_yields_a_reserved_name_or_trailing_dots() {
        // "CON." followed by a long extension: cut to 150 bytes, it is still a device name.
        let n = sanitize(&format!("CON.{}", "x".repeat(300)));
        assert!(n.starts_with("_CON.") && n.len() <= MAX_BYTES, "{n}");
        // A cut that would end in dots or spaces.
        let n = sanitize(&format!("{}. . .{}", "a".repeat(146), "b".repeat(200)));
        assert!(!n.ends_with('.') && !n.ends_with(' ') && n.len() <= MAX_BYTES, "{n}");
        // Every result of the long and odd inputs is non-empty, short, and not reserved.
        for raw in [format!("nul{}", ".".repeat(400)), format!("{} ", "é".repeat(200)), "aux ...".to_string()] {
            let n = sanitize(&raw);
            assert!(!n.is_empty() && n.len() <= MAX_BYTES && !is_reserved(&n), "{raw:?} -> {n}");
            assert!(!n.ends_with(['.', ' ']), "{n}");
        }
    }

    #[test]
    fn attachment_flag() {
        let mut h = Headers::new();
        assert!(!is_attachment(&h));
        h.push("Content-Disposition", "Attachment; filename=x");
        assert!(is_attachment(&h));
    }
}
