//! Scripts running against real pages: the DOM they see and what the page does afterwards.

use crate::dom::NodeId;
use crate::page::{Page, PageAction, PageConfig, ResKind, UrlJoin};
use crate::script::ConsoleLevel;
use crate::text::FontSystem;
use std::cell::RefCell;
use std::rc::Rc;

fn join() -> UrlJoin {
    Rc::new(|base: &str, r: &str| {
        if r.contains("://") {
            Some(r.to_string())
        } else if base == "about:blank" {
            None
        } else if let Some(frag) = r.strip_prefix('#') {
            Some(format!("{}#{}", base.split('#').next().unwrap_or(""), frag))
        } else if r.is_empty() {
            Some(base.to_string())
        } else {
            let origin: String = base.splitn(4, '/').take(3).collect::<Vec<_>>().join("/");
            Some(format!("{}/{}", origin, r.trim_start_matches('/')))
        }
    })
}

struct T {
    p: Page,
    fonts: Rc<RefCell<FontSystem>>,
    log: Vec<String>,
}

impl T {
    fn new(html: &str) -> T {
        let cfg = PageConfig { scripting: true, ..PageConfig::default() };
        let mut p = Page::new("https://example.test/dir/page?q=1", html, join(), (800.0, 600.0), cfg);
        let fonts = Rc::new(RefCell::new(FontSystem::new()));
        p.set_fonts(fonts.clone());
        let mut t = T { p, fonts, log: Vec::new() };
        t.p.update(&mut t.fonts.borrow_mut());
        t.step(0.0);
        t
    }

    /// Run script work due at `now`, then bring layout up to date.
    fn step(&mut self, now: f64) {
        for _ in 0..8 {
            if !self.p.pump(now) {
                break;
            }
        }
        self.p.update(&mut self.fonts.borrow_mut());
        for m in self.p.take_console() {
            let tag = match m.level {
                ConsoleLevel::Error => "error",
                ConsoleLevel::Warn => "warn",
                _ => "log",
            };
            self.log.push(format!("{tag}: {}", m.text));
        }
    }

    fn errors(&self) -> Vec<&String> {
        self.log.iter().filter(|l| l.starts_with("error")).collect()
    }

    fn logs(&self) -> Vec<&str> {
        self.log.iter().filter_map(|l| l.strip_prefix("log: ")).collect()
    }

    fn id(&self, id: &str) -> NodeId {
        self.p.document().element_by_id(id)
    }

    fn height(&self, id: &str) -> f32 {
        let b = self.p.layout().box_for_node(self.id(id));
        self.p.layout().boxes[b as usize].h
    }
}

#[test]
fn bootstrap_runs_and_scripts_change_the_document() {
    let t = T::new("<title>before</title><body><script>document.title = 'after ' + document.body.tagName;</script>");
    assert_eq!(t.errors(), Vec::<&String>::new(), "console: {:?}", t.log);
    assert_eq!(t.p.title, "after BODY");
}

#[test]
fn created_elements_are_laid_out() {
    let t = T::new(
        "<style>body{margin:0}</style><body><div id=a></div><script>
        var d = document.createElement('div');
        d.id = 'b';
        d.style.height = '50px';
        d.className = 'x y';
        document.body.appendChild(d);
        document.getElementById('a').style.cssText = 'height: 20px; width: 30px';
        console.log(d.classList.contains('y'), d.parentNode === document.body, document.querySelectorAll('div').length);
        </script>",
    );
    assert_eq!(t.errors(), Vec::<&String>::new(), "console: {:?}", t.log);
    assert_eq!(t.height("b"), 50.0);
    assert_eq!(t.height("a"), 20.0);
    assert_eq!(t.logs(), vec!["true true 2"]);
}

#[test]
fn inner_html_selectors_and_attributes() {
    let t = T::new(
        "<body><ul id=list></ul><script>
        var ul = document.getElementById('list');
        ul.innerHTML = '<li class=item data-n=1>one</li><li class=\"item sel\" data-n=2>two &amp; more</li>';
        var items = ul.querySelectorAll('li.item');
        var sel = ul.querySelector('.sel');
        console.log(items.length, sel.textContent, sel.getAttribute('data-n'), sel.matches('ul > li:last-child'));
        console.log(ul.innerHTML);
        console.log(sel.closest('ul') === ul, ul.children[0].nextElementSibling === sel, ul.firstChild.nodeType);
        sel.remove();
        console.log(ul.childNodes.length, ul.outerHTML);
        </script>",
    );
    assert_eq!(t.errors(), Vec::<&String>::new(), "console: {:?}", t.log);
    assert_eq!(
        t.logs(),
        vec![
            "2 two & more 2 true",
            "<li class=\"item\" data-n=\"1\">one</li><li class=\"item sel\" data-n=\"2\">two &amp; more</li>",
            "true true 1",
            "1 <ul id=\"list\"><li class=\"item\" data-n=\"1\">one</li></ul>",
        ]
    );
}

#[test]
fn click_listeners_run_and_can_cancel_navigation() {
    let mut t = T::new(
        "<style>body{margin:0} a{display:block;height:40px}</style>
        <a id=stay href='/stay'>stay</a><a id=go href='/go'>go</a>
        <script>
        document.getElementById('stay').addEventListener('click', function (e) {
            e.preventDefault();
            document.title = 'clicked ' + e.type + ' ' + (e.target === this) + ' ' + e.isTrusted;
        });
        var seen = [];
        document.addEventListener('click', function (e) { seen.push(e.target.id + ':' + e.eventPhase); }, true);
        document.body.onclick = function (e) { seen.push('body:' + e.eventPhase); console.log(seen.join(',')); };
        </script>",
    );
    assert_eq!(t.errors(), Vec::<&String>::new(), "console: {:?}", t.log);
    let a = t.p.click(10.0, 10.0, false);
    t.step(1.0);
    assert_ne!(a, PageAction::Navigate { url: "https://example.test/stay".into(), new_tab: false });
    assert_eq!(t.p.title, "clicked click true true");
    assert_eq!(t.p.click(10.0, 50.0, false), PageAction::Navigate { url: "https://example.test/go".into(), new_tab: false });
    t.step(2.0);
    assert_eq!(t.logs(), vec!["stay:1,body:3", "stay:1,body:3,go:1,body:3"]);
}

#[test]
fn timers_fire_in_order_at_their_deadlines() {
    let mut t = T::new(
        "<body><script>
        setTimeout(function () { console.log('b'); }, 100);
        setTimeout(function (x) { console.log('a' + x); }, 50, 1);
        var n = 0;
        var iv = setInterval(function () { n++; console.log('tick' + n); if (n === 2) clearInterval(iv); }, 200);
        var gone = setTimeout(function () { console.log('never'); }, 10);
        clearTimeout(gone);
        </script>",
    );
    assert_eq!(t.errors(), Vec::<&String>::new(), "console: {:?}", t.log);
    assert!(t.logs().is_empty());
    assert_eq!(t.p.next_timer_ms(), Some(50.0));
    t.step(60.0);
    assert_eq!(t.logs(), vec!["a1"]);
    t.step(250.0);
    assert_eq!(t.logs(), vec!["a1", "b", "tick1"]);
    t.step(460.0);
    t.step(700.0);
    assert_eq!(t.logs(), vec!["a1", "b", "tick1", "tick2"]);
    assert_eq!(t.p.next_timer_ms(), None);
}

#[test]
fn external_scripts_run_in_document_order() {
    let cfg = PageConfig { scripting: true, ..PageConfig::default() };
    let html = "<body><script src='/a.js'></script><script>console.log('inline sees ' + typeof fromA);</script><script src='/b.js'></script>";
    let mut p = Page::new("https://example.test/", html, join(), (800.0, 600.0), cfg);
    let reqs: Vec<_> = p.take_requests().into_iter().filter(|r| r.kind == ResKind::Script).collect();
    assert_eq!(reqs.iter().map(|r| r.url.as_str()).collect::<Vec<_>>(), vec!["https://example.test/a.js", "https://example.test/b.js"]);
    assert_eq!(p.pending_scripts(), 3);
    // the later script arrives first: nothing may run yet
    p.script_loaded(reqs[1].id, "console.log('b ran');");
    p.pump(0.0);
    assert!(p.take_console().is_empty());
    p.script_loaded(reqs[0].id, "var fromA = 1; console.log('a ran');");
    p.pump(1.0);
    let log: Vec<String> = p.take_console().into_iter().map(|m| m.text).collect();
    assert_eq!(log, vec!["a ran", "inline sees number", "b ran"]);
    assert_eq!(p.pending_scripts(), 0);
}

#[test]
fn a_failing_script_does_not_stop_the_next_one() {
    let t = T::new(
        "<body><script>var ok = 1; nope.missing();</script><script>console.log('still here ' + ok);</script>
        <script>this is not javascript</script><script>console.log('and here');</script>",
    );
    assert_eq!(t.logs(), vec!["still here 1", "and here"]);
    assert_eq!(t.errors().len(), 2, "console: {:?}", t.log);
    assert!(t.errors()[0].contains("ReferenceError"), "console: {:?}", t.log);
    assert!(t.errors()[1].contains("SyntaxError"), "console: {:?}", t.log);
}

#[test]
fn lifecycle_events_fire_once_in_order() {
    let t = T::new(
        "<body><script>
        console.log(document.readyState);
        document.addEventListener('DOMContentLoaded', function () { console.log('dcl ' + document.readyState); });
        window.addEventListener('load', function () { console.log('load ' + document.readyState); });
        window.onload = function () { console.log('onload'); };
        </script>",
    );
    assert_eq!(t.errors(), Vec::<&String>::new(), "console: {:?}", t.log);
    assert_eq!(t.logs(), vec!["loading", "dcl interactive", "load complete", "onload"]);
}

#[test]
fn script_inserted_styles_and_scripts_take_effect() {
    let mut t = T::new(
        "<style>body{margin:0}</style><body><div id=box></div><script>
        var st = document.createElement('style');
        st.textContent = '#box{height:70px}';
        document.head.appendChild(st);
        var s = document.createElement('script');
        s.src = '/late.js';
        s.onload = function () { console.log('late loaded'); };
        document.head.appendChild(s);
        var inline = document.createElement('script');
        inline.text = 'console.log(\"inline inserted\")';
        document.body.appendChild(inline);
        </script>",
    );
    assert_eq!(t.errors(), Vec::<&String>::new(), "console: {:?}", t.log);
    assert_eq!(t.height("box"), 70.0);
    let reqs: Vec<_> = t.p.take_requests().into_iter().filter(|r| r.kind == ResKind::Script).collect();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].url, "https://example.test/late.js");
    t.p.script_loaded(reqs[0].id, "document.getElementById('box').style.height = '90px'; console.log('late ran');");
    t.step(5.0);
    assert_eq!(t.logs(), vec!["inline inserted", "late ran", "late loaded"]);
    assert_eq!(t.height("box"), 90.0);
}

#[test]
fn location_url_and_storage() {
    let t = T::new(
        "<body><script>
        console.log(location.href, location.pathname, location.search, location.origin);
        var u = new URL('/x/y?a=1&b=two%20words#frag', 'https://host.test:8443/base/dir/');
        console.log(u.host, u.port, u.pathname, u.hash, u.searchParams.get('b'));
        u.searchParams.set('c', 'd e');
        console.log(u.search);
        var threw = false;
        try { new URL('relative/only'); } catch (e) { threw = e instanceof TypeError; }
        localStorage.setItem('k', 5);
        console.log(threw, localStorage.getItem('k'), localStorage.length, localStorage.getItem('nope'));
        history.pushState({n: 1}, '', '/pushed?z=1');
        console.log(location.pathname, history.state.n);
        </script>",
    );
    assert_eq!(t.errors(), Vec::<&String>::new(), "console: {:?}", t.log);
    assert_eq!(
        t.logs(),
        vec![
            "https://example.test/dir/page?q=1 /dir/page ?q=1 https://example.test",
            "host.test:8443 8443 /x/y #frag two words",
            "?a=1&b=two+words&c=d+e",
            "true 5 1 null",
            "/pushed 1",
        ]
    );
    assert_eq!(t.p.url, "https://example.test/pushed?z=1");
}

#[test]
fn form_values_events_and_submit() {
    let mut t = T::new(
        "<style>body{margin:0} input{display:block;height:30px;width:200px}</style>
        <form id=f action='/search'><input id=q name=q value='abc'><input id=c type=checkbox name=c></form>
        <script>
        var q = document.getElementById('q'), f = document.getElementById('f');
        console.log(q.value, q.type, q.form === f, f.elements.length, document.getElementById('c').checked);
        q.value = 'xyz';
        q.addEventListener('input', function (e) { console.log('input ' + q.value); });
        f.addEventListener('submit', function (e) { e.preventDefault(); console.log('submit blocked ' + q.value); });
        </script>",
    );
    assert_eq!(t.errors(), Vec::<&String>::new(), "console: {:?}", t.log);
    t.p.click(10.0, 10.0, false);
    assert!(t.p.has_text_focus());
    t.p.text_input("!");
    let a = t.p.enter();
    t.step(1.0);
    assert!(!matches!(a, PageAction::Submit { .. }), "submit should have been cancelled: {a:?}");
    assert_eq!(t.logs(), vec!["abc text true 2 false", "input xyz!", "submit blocked xyz!"]);
}

#[test]
fn fetch_and_xhr_go_through_the_embedder() {
    use crate::page::FetchResult;
    let mut t = T::new(
        "<body><script>
        fetch('/api/data?x=1', { method: 'POST', headers: { 'X-Custom': 'a' }, body: JSON.stringify({ q: 1 }) })
            .then(function (r) { console.log('fetch', r.status, r.ok, r.headers.get('content-type'), r.url); return r.json(); })
            .then(function (j) { console.log('json', j.answer); })
            .catch(function (e) { console.log('fetch failed', e.message); });
        var x = new XMLHttpRequest();
        x.open('GET', '/xhr');
        x.onreadystatechange = function () { if (x.readyState === 4) console.log('xhr', x.status, x.responseText, x.getResponseHeader('X-A')); };
        x.onerror = function () { console.log('xhr error ' + x.status); };
        x.send();
        fetch('https://other.test/blocked').then(function () { console.log('unexpected'); }, function (e) { console.log('rejected', e instanceof TypeError, e.message); });
        var threw = false;
        try { new XMLHttpRequest().open('GET', '/sync', false); } catch (e) { threw = true; }
        fetch('/x', { method: 'GET', body: 'no' }).catch(function (e) { console.log('get with body', threw, e instanceof TypeError); });
        </script>",
    );
    assert_eq!(t.errors(), Vec::<&String>::new(), "console: {:?}", t.log);
    let reqs = t.p.take_fetches();
    assert_eq!(reqs.len(), 3, "requests: {reqs:?}");
    assert_eq!((reqs[0].method.as_str(), reqs[0].url.as_str()), ("POST", "https://example.test/api/data?x=1"));
    assert_eq!(reqs[0].body.as_deref(), Some(&b"{\"q\":1}"[..]));
    assert!(reqs[0].headers.contains(&("x-custom".to_string(), "a".to_string())), "{:?}", reqs[0].headers);
    assert!(reqs[0].headers.iter().any(|(k, v)| k == "content-type" && v.starts_with("text/plain")));
    assert_eq!((reqs[0].credentials, reqs[0].same_origin), (1, false));
    assert_eq!((reqs[1].method.as_str(), reqs[1].url.as_str(), reqs[1].body.is_none()), ("GET", "https://example.test/xhr", true));
    assert_eq!(reqs[2].url, "https://other.test/blocked");
    assert_eq!(t.p.pending_fetches(), 3);
    let ok = FetchResult { status: 200, url: reqs[0].url.clone(), headers: vec![("Content-Type".into(), "application/json".into())], body: br#"{"answer":42}"#.to_vec() };
    t.p.fetch_done(reqs[0].id, Some(ok));
    t.p.fetch_done(reqs[1].id, None);
    t.p.fetch_done(reqs[2].id, None);
    t.p.fetch_done(reqs[2].id, None); // a second answer is ignored
    t.step(1.0);
    assert_eq!(t.errors(), Vec::<&String>::new(), "console: {:?}", t.log);
    assert_eq!(
        t.logs(),
        vec![
            "get with body true true",
            "fetch 200 true application/json https://example.test/api/data?x=1",
            "json 42",
            "xhr 0  null",
            "xhr error 0",
            "rejected true Failed to fetch",
        ]
    );
    assert_eq!(t.p.pending_fetches(), 0);
}

#[test]
fn document_cookie_uses_the_embedders_jar() {
    use std::cell::RefCell;
    let jar: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(vec!["seen=1".to_string()]));
    let cfg = PageConfig { scripting: true, ..PageConfig::default() };
    let html = "<body><script>console.log(document.cookie); document.cookie = 'a=b; path=/'; console.log(document.cookie);</script>";
    let mut p = Page::new("https://example.test/", html, join(), (800.0, 600.0), cfg);
    let (r, w) = (jar.clone(), jar.clone());
    p.set_cookie_access((
        Rc::new(move |url: &str| format!("{} @{}", r.borrow().join("; "), url)),
        Rc::new(move |_url: &str, cookie: &str| w.borrow_mut().push(cookie.split(';').next().unwrap_or("").to_string())),
    ));
    p.pump(0.0);
    let log: Vec<String> = p.take_console().into_iter().map(|m| m.text).collect();
    assert_eq!(log, vec!["seen=1 @https://example.test/", "seen=1; a=b @https://example.test/"]);
}

#[test]
fn other_documents_never_touch_the_page() {
    // jQuery probes `createHTMLDocument("").body.innerHTML = ...`; that must not hit the page
    let t = T::new(
        "<style>body{margin:0}</style><body><div id=keep style='height:40px'>page content</div><script>
        var other = document.implementation.createHTMLDocument('');
        other.body.innerHTML = '<form></form><form></form>';
        var base = other.createElement('base');
        other.head.appendChild(base);
        var parsed = new DOMParser().parseFromString('<p class=x>hi <b>there</b></p>', 'text/html');
        console.log(other !== document, other.body.childNodes.length, other.nodeType, other.head.lastChild === base);
        console.log(parsed.querySelector('p.x b').textContent, parsed.body.children.length, document.body.children.length);
        console.log(document.getElementById('keep').textContent, document.querySelectorAll('form').length);
        </script>",
    );
    assert_eq!(t.errors(), Vec::<&String>::new(), "console: {:?}", t.log);
    assert_eq!(t.logs(), vec!["true 2 9 true", "there 1 2", "page content 0"]);
    assert_eq!(t.height("keep"), 40.0);
}

#[test]
fn text_encoding_randomness_and_message_channels() {
    let mut t = T::new(
        "<body><script>
        var bytes = new TextEncoder().encode('h\u{e9}\u{20ac}\u{1F600}');
        console.log(Array.from(bytes).join(','), new TextDecoder().decode(bytes) === 'h\u{e9}\u{20ac}\u{1F600}');
        var a = crypto.getRandomValues(new Uint8Array(32)), b = crypto.getRandomValues(new Uint8Array(32));
        var same = true, zero = true;
        for (var i = 0; i < 32; i++) { if (a[i] !== b[i]) same = false; if (a[i] !== 0) zero = false; }
        console.log(same, zero, crypto.randomUUID().length, String.fromCodePoint(0x1F600).length);
        var ch = new MessageChannel();
        ch.port2.onmessage = function (e) { console.log('got', e.data.n); };
        ch.port1.postMessage({ n: 7 });
        </script>",
    );
    t.step(1.0);
    assert_eq!(t.errors(), Vec::<&String>::new(), "console: {:?}", t.log);
    assert_eq!(t.logs(), vec!["104,195,169,226,130,172,240,159,152,128 true", "false false 36 2", "got 7"]);
}

#[test]
fn the_global_object_is_a_window() {
    let mut t = T::new(
        "<body><script>
        var hits = [];
        addEventListener('ping', function (e) { hits.push('bare ' + (this === window)); });
        window.addEventListener('ping', function () { hits.push('qualified'); });
        dispatchEvent(new Event('ping'));
        console.log(hits.join(','), window instanceof Window, window instanceof EventTarget, typeof CDATASection, typeof ProcessingInstruction);
        // a library patching the prototype must affect the window itself
        var seen = 0, orig = Window.prototype.addEventListener;
        Window.prototype.addEventListener = function () { seen++; return orig.apply(this, arguments); };
        window.addEventListener('x', function () {});
        console.log(seen);
        </script>",
    );
    t.step(1.0);
    assert_eq!(t.errors(), Vec::<&String>::new(), "console: {:?}", t.log);
    assert_eq!(t.logs(), vec!["bare true,qualified true true function function", "1"]);
}

#[test]
fn binary_bodies_travel_both_ways() {
    use crate::page::FetchResult;
    let mut t = T::new(
        "<body><script>
        var hex = function (b) { return Array.prototype.map.call(new Uint8Array(b), function (x) { return (x < 16 ? '0' : '') + x.toString(16); }).join(''); };
        // request bodies: a typed-array window, a Blob with a type, text
        fetch('/a', { method: 'POST', body: new Uint8Array([9, 0, 255, 128, 7]).subarray(1, 4) })
            .then(function (r) { return r.arrayBuffer(); })
            .then(function (b) { console.log('a', b instanceof ArrayBuffer, hex(b)); });
        fetch('/b', { method: 'POST', body: new Blob(['h\\u00e9', new Uint8Array([0, 1])], { type: 'Application/X-Thing' }) })
            .then(function (r) { var c = r.clone(); return Promise.all([r.blob(), c.text()]); })
            .then(function (v) { console.log('b', v[0].size, v[0].type, JSON.stringify(v[1])); return v[0].slice(1, 3).arrayBuffer(); })
            .then(function (b) { console.log('slice', hex(b)); });
        var x = new XMLHttpRequest();
        x.open('POST', '/c'); x.responseType = 'arraybuffer';
        x.onload = function () { console.log('c', x.response instanceof ArrayBuffer, hex(x.response), x.responseText === ''); };
        x.send('caf\\u00e9');
        var y = new XMLHttpRequest();
        y.open('GET', '/d');
        y.onload = function () { console.log('d', JSON.stringify(y.responseText)); };
        y.send();
        // Blob, File and FileReader on their own
        var f = new File(['ab', new Blob(['cd'])], 'n.txt', { type: 'text/plain', lastModified: 5 });
        console.log('file', f.size, f.name, f.type, f.lastModified, f instanceof Blob, Object.prototype.toString.call(f));
        var fr = new FileReader();
        fr.onload = function () { console.log('reader', fr.result, fr.readyState); };
        fr.readAsDataURL(f);
        new Response('x\\r\\ny').text().then(function (s) { console.log('resp', JSON.stringify(s)); });
        </script>",
    );
    assert_eq!(t.errors(), Vec::<&String>::new(), "console: {:?}", t.log);
    let reqs = t.p.take_fetches();
    assert_eq!(reqs.len(), 4, "requests: {reqs:?}");
    assert_eq!(reqs[0].body.as_deref(), Some(&[0u8, 255, 128][..]));
    assert!(!reqs[0].headers.iter().any(|(k, _)| k == "content-type"), "{:?}", reqs[0].headers);
    assert_eq!(reqs[1].body.as_deref(), Some(&[b'h', 0xC3, 0xA9, 0, 1][..]));
    assert!(reqs[1].headers.contains(&("content-type".to_string(), "application/x-thing".to_string())), "{:?}", reqs[1].headers);
    assert_eq!(reqs[2].body.as_deref(), Some("café".as_bytes()));
    let reply = |i: usize, ct: &str, body: &[u8]| FetchResult {
        status: 200,
        url: reqs[i].url.clone(),
        headers: vec![("Content-Type".into(), ct.into())],
        body: body.to_vec(),
    };
    t.p.fetch_done(reqs[0].id, Some(reply(0, "application/octet-stream", &[0, 200, 13, 10, 255])));
    t.p.fetch_done(reqs[1].id, Some(reply(1, "image/png", &[b'o', b'k', 13, 10])));
    t.p.fetch_done(reqs[2].id, Some(reply(2, "application/octet-stream", &[1, 2, 250])));
    // a Latin-1 label is honoured by XMLHttpRequest, and line endings arrive as sent
    t.p.fetch_done(reqs[3].id, Some(reply(3, "text/plain; charset=ISO-8859-1", &[b'a', 0xE9, 13, 10, b'b'])));
    t.step(1.0);
    t.step(10.0);
    assert_eq!(t.errors(), Vec::<&String>::new(), "console: {:?}", t.log);
    let mut logs: Vec<String> = t.logs().iter().map(|s| s.to_string()).collect();
    logs.sort();
    assert_eq!(
        logs,
        vec![
            "a true 00c80d0aff",
            "b 4 image/png \"ok\\r\\n\"",
            "c true 0102fa true",
            "d \"a\u{e9}\\r\\nb\"",
            "file 4 n.txt text/plain 5 true [object File]",
            "reader data:text/plain;base64,YWJjZA== 2",
            "resp \"x\\r\\ny\"",
            "slice 6b0d",
        ]
    );
}

#[test]
fn file_controls_pick_read_and_upload() {
    use crate::page::PickedFile;
    let mut t = T::new(
        "<body><div id=drop style='height:40px'>drop a file</div>
        <form id=f action='/up' method=post enctype='multipart/form-data'>
          <input type=hidden name=kind value='doc'>
          <input type=file id=file name=file accept='.txt,image/*' style='display:none'>
        </form>
        <script>
        var input = document.getElementById('file'), kept = null;
        // not in answer to the user: must not open anything
        input.click();
        console.log('start', input.files.length, input.files instanceof FileList, JSON.stringify(input.value));
        document.getElementById('drop').addEventListener('click', function () { input.click(); });
        input.addEventListener('change', function (e) {
          var f = this.files[0];
          kept = f;
          console.log('change', e.isTrusted, this.files.length, f.name, f.size, f.type, f.lastModified, f instanceof File, this.value);
          f.slice(-5).text().then(function (s) { console.log('tail', s, f.slice(2, 4).size); });
          // the whole form, as a script would send it
          var x = new XMLHttpRequest();
          x.open('POST', '/upload');
          x.upload.addEventListener('progress', function (p) { console.log('progress', p.lengthComputable, p.loaded === p.total, p.total > 12); });
          x.onload = function () { console.log('uploaded', x.status); };
          x.send(new FormData(document.getElementById('f')));
          // one chunk of it, the way large uploads are split
          var fd = new FormData();
          fd.append('chunk', f.slice(0, 5), 'part0');
          fd.append('name', f.name);
          fetch('/chunk', { method: 'POST', body: fd });
        });
        </script>",
    );
    assert_eq!(t.errors(), Vec::<&String>::new(), "console: {:?}", t.log);
    assert_eq!(t.p.take_action(), None, "a script may not open the file dialog by itself");
    // the user clicks the drop zone; its handler forwards the click to the hidden control
    let action = t.p.click(20.0, 20.0, false);
    let control = t.id("file");
    assert_eq!(action, PageAction::PickFiles { control, multiple: false, accept: ".txt,image/*".into() });
    t.p.files_chosen(control, vec![PickedFile { name: "notes.txt".into(), mime: "text/plain".into(), last_modified_ms: 1234.0, bytes: b"hello\r\nworld".to_vec() }]);
    t.step(1.0);
    assert_eq!(t.errors(), Vec::<&String>::new(), "console: {:?}", t.log);
    let reqs = t.p.take_fetches();
    assert_eq!(reqs.len(), 2, "requests: {reqs:?}");
    let text = |b: &Option<Vec<u8>>| String::from_utf8_lossy(b.as_deref().unwrap_or(&[])).into_owned();
    let boundary = |r: &crate::page::ScriptFetch| {
        let ct = &r.headers.iter().find(|(k, _)| k == "content-type").expect("content type").1;
        ct.strip_prefix("multipart/form-data; boundary=").expect("multipart").to_string()
    };
    let b0 = boundary(&reqs[0]);
    assert_eq!(
        text(&reqs[0].body),
        format!(
            "--{b0}\r\nContent-Disposition: form-data; name=\"kind\"\r\n\r\ndoc\r\n--{b0}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"notes.txt\"\r\nContent-Type: text/plain\r\n\r\nhello\r\nworld\r\n--{b0}--\r\n"
        )
    );
    let b1 = boundary(&reqs[1]);
    assert_eq!(
        text(&reqs[1].body),
        format!(
            "--{b1}\r\nContent-Disposition: form-data; name=\"chunk\"; filename=\"part0\"\r\nContent-Type: application/octet-stream\r\n\r\nhello\r\n--{b1}\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\nnotes.txt\r\n--{b1}--\r\n"
        )
    );
    t.p.fetch_done(reqs[0].id, Some(crate::page::FetchResult { status: 201, url: reqs[0].url.clone(), headers: vec![], body: vec![] }));
    t.step(2.0);
    // clearing the control: the selection goes, a File the script kept stays readable
    t.p.eval("input.value = ''; console.log('cleared', input.files.length, JSON.stringify(input.value)); kept.text().then(function (s) { console.log('kept', s.length); });");
    t.step(3.0);
    assert_eq!(t.errors(), Vec::<&String>::new(), "console: {:?}", t.log);
    assert_eq!(
        t.logs(),
        vec![
            "start 0 true \"\"",
            "change true 1 notes.txt 12 text/plain 1234 true C:\\fakepath\\notes.txt",
            "tail world 2",
            "progress true true true",
            "uploaded 201",
            "cleared 0 \"\"",
            "kept 12",
        ]
    );
    // a plain form submission carries the file too
    t.p.files_chosen(control, vec![PickedFile { name: "a\"b.bin".into(), mime: String::new(), last_modified_ms: 0.0, bytes: vec![0, 255, 13, 10] }]);
    t.p.eval("document.getElementById('f').submit();");
    match t.p.take_action() {
        Some(PageAction::Submit { url, body: Some(body), content_type: Some(ct) }) => {
            assert_eq!(url, "https://example.test/up");
            let b = ct.strip_prefix("multipart/form-data; boundary=").expect("multipart");
            let mut want = format!("--{b}\r\nContent-Disposition: form-data; name=\"kind\"\r\n\r\ndoc\r\n--{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a%22b.bin\"\r\nContent-Type: application/octet-stream\r\n\r\n").into_bytes();
            want.extend_from_slice(&[0, 255, 13, 10]);
            want.extend_from_slice(format!("\r\n--{b}--\r\n").as_bytes());
            assert_eq!(body, want);
        }
        other => panic!("expected a multipart submission, got {other:?}"),
    }
}

#[test]
fn current_script_is_still_set_in_the_scripts_microtasks() {
    let mut t = T::new(
        "<body><script id=one>
        Promise.resolve().then(function () { console.log('microtask', document.currentScript && document.currentScript.id); });
        (async function () { await null; console.log('after await', document.currentScript && document.currentScript.id); })();
        setTimeout(function () { console.log('timer', document.currentScript); }, 0);
        console.log('sync', document.currentScript.id);
        </script>",
    );
    t.step(1.0);
    assert_eq!(t.errors(), Vec::<&String>::new(), "console: {:?}", t.log);
    assert_eq!(t.logs(), vec!["sync one", "microtask one", "after await one", "timer null"]);
}

#[test]
fn streams_read_pipe_and_back_responses() {
    let mut t = T::new(
        "<body><script>
        var log = function () { console.log(Array.prototype.join.call(arguments, ' ')); };
        // a source that is pulled, read to the end
        var i = 0;
        var s = new ReadableStream({ pull: function (c) { i++; if (i > 3) c.close(); else c.enqueue('c' + i); } });
        var r = s.getReader(), got = [];
        (function next() {
          r.read().then(function (x) { if (x.done) { log('read', got.join(','), s.locked); return; } got.push(x.value); next(); });
        })();
        r.closed.then(function () { log('closed'); });
        // enqueue before anyone reads, async iteration, a second reader is refused
        var q = new ReadableStream({ start: function (c) { c.enqueue(1); c.enqueue(2); c.close(); } });
        (async function () {
          var seen = [];
          for await (var v of q) seen.push(v);
          var threw = false;
          try { s.getReader(); } catch (e) { threw = e instanceof TypeError; }
          log('iter', seen.join(','), q.locked, threw);
        })();
        // errors reach the reader
        new ReadableStream({ start: function (c) { c.error(new Error('boom')); } }).getReader().read().catch(function (e) { log('error', e.message); });
        // a transform in the middle, then collected by a Response
        var upper = new TransformStream({ transform: function (chunk, c) { c.enqueue(chunk.toUpperCase()); }, flush: function (c) { c.enqueue('!'); } });
        var src = new ReadableStream({ start: function (c) { c.enqueue('ab'); c.enqueue('cd'); c.close(); } });
        new Response(src.pipeThrough(upper)).text().then(function (text) { log('piped', text); });
        // a response body as a stream of bytes; the body counts as used afterwards
        var resp = new Response('h\\u00e9llo');
        var br = resp.body.getReader();
        br.read().then(function (x) {
          log('body', x.value instanceof Uint8Array, x.value.length, resp.bodyUsed);
          return Promise.all([br.read(), resp.text().then(function () { return 'read twice'; }, function (e) { return e instanceof TypeError; })]);
        }).then(function (v) { log('body end', v[0].done, v[1]); });
        // tee and Blob.stream
        var pair = new Blob(['xyz']).stream().tee();
        Promise.all([new Response(pair[0]).text(), new Response(pair[1]).arrayBuffer()]).then(function (v) { log('tee', v[0], v[1].byteLength); });
        // a writable sink sees chunks in order and then close
        var sunk = [];
        var w = new WritableStream({ write: function (c) { sunk.push(c); }, close: function () { sunk.push('end'); } });
        new ReadableStream({ start: function (c) { c.enqueue('p'); c.enqueue('q'); c.close(); } }).pipeTo(w).then(function () { log('sink', sunk.join(','), w.locked); });
        log('types', typeof ReadableStreamDefaultReader, typeof WritableStreamDefaultWriter, new CountQueuingStrategy({ highWaterMark: 4 }).highWaterMark);
        </script>",
    );
    t.step(1.0);
    t.step(10.0);
    assert_eq!(t.errors(), Vec::<&String>::new(), "console: {:?}", t.log);
    let mut logs: Vec<String> = t.logs().iter().map(|s| s.to_string()).collect();
    logs.sort();
    assert_eq!(
        logs,
        vec![
            "body end true true",
            "body true 6 true",
            "closed",
            "error boom",
            "iter 1,2 false true",
            "piped ABCD!",
            "read c1,c2,c3 true",
            "sink p,q,end false",
            "tee xyz 3",
            "types function function 4",
        ]
    );
}

#[test]
fn dom_objects_name_their_interface_and_storage_fails_politely() {
    let mut t = T::new(
        "<body><div id=d>x</div><script>
        var str = function (x) { return Object.prototype.toString.call(x); };
        var d = document.getElementById('d');
        console.log(str(d), str(document.createTextNode('t')), str(window), str(new Event('x')), str({}), str([]));
        // what a reactive library does: only plain objects get wrapped
        console.log(str(d) === '[object Object]', str(document.body), str(document.createElement('section')));
        // observers refuse things that are not elements at the call, not later in a timer
        var threw = [];
        try { new ResizeObserver(function () {}).observe(new Proxy({}, {})); } catch (e) { threw.push(e instanceof TypeError); }
        try { new IntersectionObserver(function () {}).observe(document.createTextNode('t')); } catch (e) { threw.push(e instanceof TypeError); }
        console.log(threw.join(','));
        new ResizeObserver(function (entries) { console.log('resize', entries[0].target === d, entries[0].contentRect.width > 0); }).observe(d);
        // IndexedDB: the interfaces exist, opening reports an error instead of throwing
        console.log(typeof IDBIndex, typeof IDBKeyRange.bound, indexedDB instanceof IDBFactory, IDBKeyRange.bound(1, 5).includes(3));
        var req = indexedDB.open('db', 1);
        req.onsuccess = function () { console.log('opened'); };
        req.onerror = function (e) { console.log('idb', e.target === req, req.error.name, req.readyState); };
        </script>",
    );
    t.step(1.0);
    t.p.frame(5.0);
    t.step(10.0);
    assert_eq!(t.errors(), Vec::<&String>::new(), "console: {:?}", t.log);
    assert_eq!(
        t.logs(),
        vec![
            "[object HTMLDivElement] [object Text] [object Window] [object Event] [object Object] [object Array]",
            "false [object HTMLBodyElement] [object HTMLElement]",
            "true,true",
            "function function true true",
            "idb true InvalidStateError done",
            // size observers report with the next frame, after the timers
            "resize true true",
        ]
    );
}

#[test]
fn custom_elements_upgrade_and_get_their_callbacks() {
    let mut t = T::new(
        "<style>body{margin:0} x-box{display:block}</style><body><x-box id=parsed size=3></x-box><script>
        var log = [];
        class XBox extends HTMLElement {
            static get observedAttributes() { return ['size']; }
            constructor() { super(); this.ready = true; log.push('ctor ' + (this.id || 'new')); }
            connectedCallback() { log.push('connected ' + this.id); this.style.height = (this.getAttribute('size') * 10) + 'px'; }
            disconnectedCallback() { log.push('disconnected ' + this.id); }
            attributeChangedCallback(name, old, value) { log.push(name + ':' + old + '>' + value); }
        }
        customElements.define('x-box', XBox);
        var parsed = document.getElementById('parsed');
        console.log(log.join(' | '));
        console.log(parsed instanceof XBox, parsed.ready, customElements.get('x-box') === XBox);
        log = [];
        var made = document.createElement('x-box');
        made.id = 'made';
        made.setAttribute('size', '5');
        document.body.appendChild(made);
        var viaNew = new XBox();
        viaNew.id = 'vianew';
        var holder = document.createElement('div');
        holder.innerHTML = '<x-box id=inner size=2></x-box>';
        document.body.appendChild(holder);
        made.remove();
        console.log(log.join(' | '));
        console.log(viaNew instanceof XBox, viaNew.localName, document.getElementById('inner') instanceof XBox);
        var illegal = false;
        try { new HTMLElement(); } catch (e) { illegal = e instanceof TypeError; }
        customElements.whenDefined('x-box').then(function (c) { console.log('whenDefined', c === XBox, illegal); });
        </script>",
    );
    t.step(1.0);
    assert_eq!(t.errors(), Vec::<&String>::new(), "console: {:?}", t.log);
    assert_eq!(
        t.logs(),
        vec![
            "ctor parsed | size:null>3 | connected parsed",
            "true true true",
            "ctor new | size:null>5 | connected made | ctor new | ctor inner | size:null>2 | connected inner | disconnected made",
            "true x-box true",
            "whenDefined true true",
        ]
    );
    // callbacks changed layout: 3 * 10px and 2 * 10px
    assert_eq!(t.height("parsed"), 30.0);
    assert_eq!(t.height("inner"), 20.0);
}

#[test]
fn geometry_read_after_a_change_is_up_to_date() {
    // a script that changes the document and measures in the same task must see the result
    let t = T::new(
        "<style>body{margin:0} #a{height:10px} .tall{height:120px !important} #b{width:50%}</style>
        <body><div id=a></div><div id=b>text</div><script>
        var a = document.getElementById('a'), b = document.getElementById('b');
        var before = a.offsetHeight;
        a.className = 'tall';
        var afterClass = a.offsetHeight, bTop = b.getBoundingClientRect().top;
        a.style.height = '30px';
        a.className = '';
        var afterStyle = a.getBoundingClientRect().height;
        var st = document.createElement('style');
        st.textContent = '#b{height:44px;display:inline-block}';
        document.head.appendChild(st);
        var cs = getComputedStyle(b);
        var added = document.createElement('div');
        added.style.cssText = 'height:7px;width:300px';
        document.body.appendChild(added);
        console.log(before, afterClass, bTop, afterStyle, b.offsetHeight, cs.display, b.offsetWidth, added.offsetTop, added.offsetWidth, document.documentElement.scrollHeight >= 81);
        </script>",
    );
    assert_eq!(t.errors(), Vec::<&String>::new(), "console: {:?}", t.log);
    assert_eq!(t.logs(), vec!["10 120 120 30 44 inline-block 400 74 300 true"]);
    assert!(t.p.forced_layouts() >= 3, "forced layouts: {}", t.p.forced_layouts());
}

#[test]
fn css_supports_answers_from_what_the_engine_parses() {
    let t = T::new(
        "<body><script>
        console.log(CSS.supports('(--a: 0)'), CSS.supports('--x', 'red'), CSS.supports('display', 'grid'), CSS.supports('(display: flex)'));
        console.log(CSS.supports('backdrop-filter', 'blur(2px)'), CSS.supports('not-a-property', '1'), CSS.supports('(color: red) and (gap: 1px)'), CSS.supports('color', ''));
        console.log(CSS.escape('a.b#c d'), CSS.escape('1st'), CSS.escape('-x_y'));
        </script>",
    );
    assert_eq!(t.errors(), Vec::<&String>::new(), "console: {:?}", t.log);
    assert_eq!(t.logs(), vec!["true true true true", "false false false false", "a\\.b\\#c\\ d \\31 st -x_y"]);
}

#[test]
fn module_scripts_load_their_graph_and_run_in_order() {
    let cfg = PageConfig { scripting: true, ..PageConfig::default() };
    let html = "<body><script type=module src='/app/main.js'></script>
        <script>console.log('classic first');</script>
        <script nomodule>console.log('nomodule must not run');</script>
        <script type=module>import { twice } from '/app/util.js'; console.log('inline module', twice(4), typeof document.currentScript, import.meta.url.length > 0);
            import('/app/lazy.js').then(function (m) { console.log('lazy', m.default); });
            import('/app/missing.js').catch(function (e) { console.log('missing rejected', e instanceof TypeError); });</script>";
    let mut p = Page::new("https://example.test/", html, join(), (800.0, 600.0), cfg);
    let mut served: Vec<String> = Vec::new();
    let mut log: Vec<String> = Vec::new();
    // play the network: answer every script request until the page is quiet
    for round in 0..12 {
        p.pump(round as f64);
        log.extend(p.take_console().into_iter().map(|m| m.text));
        let reqs: Vec<_> = p.take_requests().into_iter().filter(|r| r.kind == ResKind::Script).collect();
        if reqs.is_empty() && round > 2 {
            break;
        }
        for r in reqs {
            served.push(r.url.clone());
            match r.url.as_str() {
                "https://example.test/app/main.js" => p.script_loaded(r.id, "import { twice } from '/app/util.js'; import name from '/app/name.js'; console.log('main', name, twice(21));"),
                "https://example.test/app/util.js" => p.script_loaded(r.id, "console.log('util evaluated'); export function twice(x) { return x * 2; }"),
                "https://example.test/app/name.js" => p.script_loaded(r.id, "export default 'graph';"),
                "https://example.test/app/lazy.js" => p.script_loaded(r.id, "export default 'loaded on demand';"),
                _ => p.resource_failed(r.id),
            }
        }
    }
    assert_eq!(
        log,
        vec![
            "classic first",
            "util evaluated",
            "main graph 42",
            "inline module 8 object true",
            "lazy loaded on demand",
            "missing rejected true",
        ],
        "served: {served:?}"
    );
    // each module source was fetched exactly once
    served.sort();
    let mut unique = served.clone();
    unique.dedup();
    assert_eq!(served, unique);
    assert_eq!(p.pending_scripts(), 0);
}

impl T {
    /// One displayed frame at `now`: timers, frame callbacks, animations, then layout.
    fn frame(&mut self, now: f64) {
        self.step(now);
        self.p.frame(now);
        self.step(now);
    }

    fn opacity(&self, id: &str) -> f32 {
        self.p.styles().get(self.id(id)).map_or(f32::NAN, |s| s.opacity)
    }
}

#[test]
fn animation_frames_run_once_per_frame_in_order() {
    let mut t = T::new(
        "<script>
           var seen = [];
           requestAnimationFrame(function (ts) { seen.push('a' + (typeof ts)); requestAnimationFrame(function () { seen.push('c'); }); });
           var dropped = requestAnimationFrame(function () { seen.push('never'); });
           requestAnimationFrame(function () { seen.push('b'); });
           cancelAnimationFrame(dropped);
         </script>",
    );
    assert!(t.p.wants_frame());
    // timers alone do not run frame callbacks
    t.step(500.0);
    t.p.eval("console.log(seen.join(','))");
    t.frame(516.0);
    t.p.eval("console.log(seen.join(','))");
    // a callback queued during a frame waits for the next one
    assert!(t.p.wants_frame());
    t.frame(532.0);
    t.p.eval("console.log(seen.join(','))");
    t.step(532.0);
    assert!(!t.p.wants_frame());
    assert_eq!(t.logs(), ["", "anumber,b", "anumber,b,c"]);
    assert!(t.errors().is_empty(), "{:?}", t.log);
}

#[test]
fn element_animate_moves_values_and_finishes() {
    let mut t = T::new(
        "<div id=box style='width:50px;height:50px'></div>
         <script>
           var box = document.getElementById('box');
           var a = box.animate({ opacity: [0, 1], transform: ['translateX(0px)', 'translateX(100px)'] }, { duration: 200, easing: 'linear', fill: 'both' });
           a.finished.then(function () { console.log('finished ' + a.playState + ' ' + a.currentTime); });
           a.onfinish = function () { console.log('onfinish'); };
           console.log(a.playState, box.getAnimations().length, document.getAnimations().length);
           var gone = box.animate([{ opacity: 0.3 }, { opacity: 0.6 }], 100);
           gone.finished.catch(function (e) { console.log('cancelled ' + e.name); });
           gone.cancel();
           console.log(gone.playState, box.getAnimations().length);
         </script>",
    );
    t.frame(0.0);
    assert!(t.p.wants_frame());
    t.frame(100.0);
    assert!((t.opacity("box") - 0.5).abs() < 0.01, "{}", t.opacity("box"));
    let st = t.p.styles().get(t.id("box")).unwrap();
    let m = st.transform_matrix(50.0, 50.0).unwrap();
    assert!((m[4] - 50.0).abs() < 0.5, "{m:?}");
    t.p.eval("console.log(getComputedStyle(box).opacity, a.currentTime, a.playState)");
    t.frame(250.0);
    // `fill: both` keeps the last values after the end
    assert!((t.opacity("box") - 1.0).abs() < 0.001);
    t.frame(266.0);
    assert!(!t.p.wants_frame());
    // committing the values and cancelling leaves them as inline style
    t.p.eval("a.commitStyles(); a.cancel(); console.log(box.style.opacity, a.playState, box.getAnimations().length)");
    t.frame(280.0);
    assert!((t.opacity("box") - 1.0).abs() < 0.001);
    assert_eq!(
        t.logs(),
        ["running 1 1", "idle 1", "cancelled AbortError", "0.5 100 running", "onfinish", "finished finished 200", "1 idle 0"]
    );
    assert!(t.errors().is_empty(), "{:?}", t.log);
}

#[test]
fn css_transitions_and_keyframes_run_on_the_page_clock() {
    let mut t = T::new(
        "<style>
           #b { width: 100px; height: 20px; opacity: 1; background-color: rgb(0, 0, 0); transition: opacity 100ms linear, background-color 100ms linear 50ms }
           #b.on { opacity: 0.5; background-color: rgb(200, 0, 0) }
           @keyframes turn { from { rotate: 0deg } to { rotate: 360deg } }
           #s { width: 10px; height: 10px; animation: turn 1s linear infinite }
           @keyframes fade { from { opacity: var(--from, 0.2) } }
           #f { --from: 0.4; animation: fade 100ms linear both }
         </style>
         <div id=b></div><div id=s></div><div id=f></div>
         <script>
           var b = document.getElementById('b'), log = [];
           ['transitionstart', 'transitionend', 'animationstart', 'animationend'].forEach(function (k) {
             document.addEventListener(k, function (e) { log.push(k + ':' + (e.propertyName || e.animationName) + ':' + e.target.id); });
           });
         </script>",
    );
    t.frame(0.0);
    // keyframes start with the first style pass; var() in a keyframe reads the element's value
    assert!((t.opacity("f") - 0.4).abs() < 0.001, "{}", t.opacity("f"));
    t.frame(50.0);
    assert!((t.opacity("f") - 0.7).abs() < 0.01, "{}", t.opacity("f"));
    let rot = |t: &T| t.p.styles().get(t.id("s")).unwrap().rotate;
    assert!((rot(&t) - 18.0).abs() < 0.5, "{}", rot(&t));
    // a class change starts the transitions from the old values
    t.p.eval("b.className = 'on'");
    t.step(50.0);
    assert!((t.opacity("b") - 1.0).abs() < 0.001, "{}", t.opacity("b"));
    t.frame(100.0);
    assert!((t.opacity("b") - 0.75).abs() < 0.01, "{}", t.opacity("b"));
    let bg = |t: &T| t.p.styles().get(t.id("b")).unwrap().background_color;
    assert_eq!(bg(&t).r, 0); // still inside its delay
    t.frame(150.0);
    assert!((t.opacity("b") - 0.5).abs() < 0.001);
    assert_eq!(bg(&t).r, 100);
    // turning back half-way goes from where it is
    t.p.eval("b.className = ''");
    t.step(150.0);
    assert_eq!(bg(&t).r, 100);
    t.frame(175.0);
    assert!((t.opacity("b") - 0.625).abs() < 0.01, "{}", t.opacity("b"));
    t.frame(1000.0);
    assert!((t.opacity("b") - 1.0).abs() < 0.001);
    assert_eq!(bg(&t).r, 0);
    // the endless spinner keeps the page asking for frames
    assert!(t.p.wants_frame());
    t.p.eval("document.getElementById('s').style.animation = 'none'");
    t.frame(1016.0);
    assert!(!t.p.wants_frame());
    assert_eq!(rot(&t), 0.0);
    t.p.eval("console.log(log.join(' '))");
    t.step(1016.0);
    let log = t.logs().join(" ");
    for want in ["animationstart:turn:s", "animationstart:fade:f", "animationend:fade:f", "transitionstart:opacity:b", "transitionend:opacity:b", "transitionend:background-color:b"] {
        assert!(log.contains(want), "missing {want} in {log}");
    }
    assert!(t.errors().is_empty(), "{:?}", t.log);
}

#[test]
fn observers_follow_layout_and_scrolling() {
    let mut t = T::new(
        "<div id=top style='height:300px'></div><div id=far style='height:100px;margin-top:2000px'></div><div style='height:3000px'></div>
         <script>
           var out = [];
           var io = new IntersectionObserver(function (entries) {
             entries.forEach(function (e) { out.push(e.target.id + ':' + e.isIntersecting + ':' + Math.round(e.intersectionRatio * 100)); });
           }, { threshold: [0, 0.5] });
           io.observe(document.getElementById('top'));
           io.observe(document.getElementById('far'));
           var ro = new ResizeObserver(function (entries) { entries.forEach(function (e) { out.push('size:' + e.contentRect.height); }); });
           ro.observe(document.getElementById('top'));
           console.log(IntersectionObserverEntry.prototype.hasOwnProperty('intersectionRatio'));
         </script>",
    );
    t.frame(16.0);
    t.p.eval("console.log(out.join(' ')); out.length = 0");
    // nothing changed: the observers stay quiet and ask for no frames
    t.frame(32.0);
    t.frame(48.0);
    assert!(!t.p.wants_frame());
    // scrolling brings the far box in and takes the top one out
    t.p.scroll_to(0.0, 2000.0);
    assert!(t.p.wants_frame());
    t.frame(64.0);
    t.p.eval("console.log(out.join(' ')); out.length = 0");
    // a size change is reported once
    t.p.eval("document.getElementById('top').style.height = '120px'");
    t.frame(80.0);
    t.frame(96.0);
    t.p.eval("console.log(out.join(' '))");
    t.step(96.0);
    assert_eq!(t.logs(), ["true", "size:300 top:true:100 far:false:0", "top:false:0 far:true:100", "size:120"]);
    assert!(t.errors().is_empty(), "{:?}", t.log);
}
