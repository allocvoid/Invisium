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
fn async_and_defer_scripts_keep_the_order_a_parser_gives_them() {
    let cfg = PageConfig { scripting: true, ..PageConfig::default() };
    // The layout a bundler emits: early async chunks, a deferred script, an inline bootstrap,
    // and an async chunk after it that the bootstrap waits for.
    let html = "<head><script async src='/early.js'></script></head><body>\
        <script defer src='/defer.js'></script>\
        <script>console.log('inline, late chunk ran: ' + (typeof late !== 'undefined'));</script>\
        <script async src='/late.js'></script>\
        <script src='/block.js'></script>\
        <script async src='/last.js'></script>\
        <script>document.addEventListener('DOMContentLoaded', function () { console.log('dcl'); });</script>";
    let mut p = Page::new("https://example.test/", html, join(), (800.0, 600.0), cfg);
    let reqs: Vec<_> = p.take_requests().into_iter().filter(|r| r.kind == ResKind::Script).collect();
    let id = |name: &str| reqs.iter().find(|r| r.url.ends_with(name)).unwrap().id;
    let mut log: Vec<String> = Vec::new();
    // The later chunks arrive before anything else. `late.js` may run only after the inline
    // script before it; `last.js` has a blocking script before it that is still loading.
    p.script_loaded(id("late.js"), "var late = 1; console.log('late');");
    p.script_loaded(id("last.js"), "console.log('last');");
    p.pump(0.0);
    log.extend(p.take_console().into_iter().map(|m| m.text));
    assert_eq!(log, vec!["inline, late chunk ran: false", "late"]);
    // an early async script runs whenever it arrives; the deferred one does not hold anything
    p.script_loaded(id("early.js"), "console.log('early');");
    p.pump(1.0);
    p.script_loaded(id("block.js"), "console.log('block');");
    p.pump(2.0);
    log.extend(p.take_console().into_iter().map(|m| m.text));
    assert_eq!(log[2..], ["early", "block", "last"]);
    // deferred scripts run last, and DOMContentLoaded waits for them
    p.script_loaded(id("defer.js"), "console.log('defer');");
    p.pump(3.0);
    log.extend(p.take_console().into_iter().map(|m| m.text));
    assert_eq!(log[5..], ["defer", "dcl"]);
    assert_eq!(p.pending_scripts(), 0);
}

#[test]
fn url_needs_a_base_unless_it_is_absolute() {
    let t = T::new(
        "<body><script>
        function tryUrl(a, b) { try { return (b === undefined ? new URL(a) : new URL(a, b)).href; } catch (e) { return e.name; } }
        console.log(tryUrl(''), tryUrl('/x'), tryUrl('x y'), tryUrl('https://a.test/p?q#h'));
        console.log(tryUrl('/x', 'https://a.test/dir/'), tryUrl('', 'https://a.test/dir/'), tryUrl('/x', 'nope'));
        console.log(URL.canParse(''), URL.canParse('https://b.test/'), URL.parse('') === null, URL.parse('https://a.test/').host);
        </script>",
    );
    assert!(t.errors().is_empty(), "{:?}", t.errors());
    assert_eq!(
        t.logs(),
        vec![
            "TypeError TypeError TypeError https://a.test/p?q#h",
            "https://a.test/x https://a.test/dir/ TypeError",
            "false true true a.test",
        ]
    );
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

/// A GPU device that only records what it is asked to do.
#[derive(Default)]
struct RecordingGl {
    next: u32,
    cmds: Vec<invisium_draw::gl::Cmd>,
    destroyed: Vec<u32>,
}

impl invisium_draw::gl::GlDevice for RecordingGl {
    fn create(&mut self) -> Option<u32> {
        self.next += 1;
        Some(self.next)
    }
    fn destroy(&mut self, ctx: u32) {
        self.destroyed.push(ctx);
    }
    fn image(&self, ctx: u32) -> invisium_draw::ImageId {
        9000 + ctx
    }
    fn run(&mut self, _ctx: u32, cmds: &mut Vec<invisium_draw::gl::Cmd>) {
        self.cmds.append(cmds);
    }
    fn read_pixels(&mut self, _ctx: u32, _target: &invisium_draw::gl::Target, _color: u32, rect: invisium_draw::gl::Rect4) -> Option<(invisium_draw::gl::TexFormat, Vec<u8>)> {
        let n = (rect[2].max(0) * rect[3].max(0)) as usize;
        Some((invisium_draw::gl::TexFormat::Rgba8, [10u8, 20, 30, 255].repeat(n)))
    }
    fn read_buffer(&mut self, _ctx: u32, _buffer: u32, _offset: u32, len: u32) -> Option<Vec<u8>> {
        Some(vec![7; len as usize])
    }
    fn stats(&self) -> invisium_draw::gl::GlStats {
        invisium_draw::gl::GlStats::default()
    }
}

#[test]
fn a_canvas_gets_a_webgl_context_that_reaches_the_device() {
    use invisium_draw::gl::Cmd;
    let cfg = PageConfig { scripting: true, ..PageConfig::default() };
    let html = "<body><canvas id=c width=64 height=32></canvas><canvas id=plain></canvas><script>
        var c = document.getElementById('c');
        var gl = c.getContext('webgl2', { alpha: false });
        console.log(String(gl), gl === c.getContext('webgl2'), c.getContext('2d'), gl.canvas === c);
        console.log(gl.getParameter(gl.VERSION), gl.drawingBufferWidth, gl.drawingBufferHeight, gl.DEPTH_TEST, WebGL2RenderingContext.TEXTURE_2D, gl.TIMEOUT_IGNORED);
        gl.clearColor(1, 0, 0, 1);
        gl.clear(gl.COLOR_BUFFER_BIT);
        var px = new Uint8Array(8);
        gl.readPixels(0, 0, 2, 1, gl.RGBA, gl.UNSIGNED_BYTE, px);
        console.log(Array.prototype.join.call(px, ' '));
        var b = gl.createBuffer();
        gl.bindBuffer(gl.ARRAY_BUFFER, b);
        gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([1, 2, 3]), gl.STATIC_DRAW);
        console.log(String(b), b instanceof WebGLBuffer, gl.getParameter(gl.ARRAY_BUFFER_BINDING) === b, gl.getBufferParameter(gl.ARRAY_BUFFER, gl.BUFFER_SIZE), gl.getError());
        var ext = gl.getExtension('webgl_lose_context');
        console.log(typeof ext.loseContext, ext === gl.getExtension('WEBGL_lose_context'), gl.getExtension('WEBGL_debug_renderer_info'));
        console.log(document.getElementById('plain').getContext('nonsense'));
        </script>";
    let mut p = Page::new("https://example.test/", html, join(), (800.0, 600.0), cfg);
    let dev = Rc::new(RefCell::new(RecordingGl::default()));
    p.set_gl_device(dev.clone());
    let fonts = Rc::new(RefCell::new(FontSystem::new()));
    p.set_fonts(fonts.clone());
    p.update(&mut fonts.borrow_mut());
    p.pump(0.0);
    p.update(&mut fonts.borrow_mut());
    let log: Vec<String> = p.take_console().into_iter().map(|m| m.text).collect();
    assert_eq!(
        log,
        vec![
            "[object WebGL2RenderingContext] true null true",
            "WebGL 2.0 64 32 2929 3553 -1",
            "10 20 30 255 10 20 30 255",
            "[object WebGLBuffer] true true 12 0",
            "function true null",
            "null",
        ]
    );
    // the script's drawing reached the device and was presented at the end of the turn
    let cmds = std::mem::take(&mut dev.borrow_mut().cmds);
    assert!(matches!(cmds.first(), Some(Cmd::Configure { width: 64, height: 32, alpha: false, .. })), "{:?}", cmds.first());
    assert!(cmds.iter().any(|c| matches!(c, Cmd::Clear { .. })));
    assert!(cmds.iter().any(|c| matches!(c, Cmd::BufferCreate { bytes, .. } if bytes.len() == 12)));
    assert_eq!(cmds.iter().filter(|c| matches!(c, Cmd::Present)).count(), 1);
    // the element is painted with the device's image, which the embedder must not release
    assert_eq!(p.gl_image_ids(), vec![9001]);
    assert!(p.image_ids().is_empty());
    assert_eq!(p.canvas_stats().0, 1);
    // the context goes with the page
    drop(p);
    assert_eq!(dev.borrow().destroyed, vec![1]);
}

#[test]
fn a_canvas_draws_in_2d_and_hands_its_bitmap_to_the_embedder() {
    let mut t = T::new(
        "<body><canvas id=c width=40 height=20 style='color: rgb(0, 170, 102)'></canvas><script>
        var c = document.getElementById('c'), x = c.getContext('2d');
        console.log(String(x), x === c.getContext('2d'), c.getContext('webgl'), x.canvas === c);
        x.fillStyle = '#1e66d0';
        x.fillRect(0, 0, 20, 20);
        x.fillStyle = 'not a colour';
        console.log(x.fillStyle, x.font, x.globalAlpha, x.lineWidth);
        x.fillStyle = 'currentColor';
        x.fillRect(20, 0, 20, 20);
        var d = x.getImageData(5, 5, 1, 1), e = x.getImageData(30, 5, 1, 1).data;
        console.log(d instanceof ImageData, d.width, Object.prototype.toString.call(d.data), d.data.join(','), e.join(','));
        var g = x.createLinearGradient(0, 0, 40, 0);
        g.addColorStop(0, 'red');
        console.log(g instanceof CanvasGradient, String(g));
        x.font = 'italic 700 12px sans-serif';
        var m = x.measureText('abc');
        console.log(x.font, m.width > 5, m.fontBoundingBoxAscent > 5, typeof m.actualBoundingBoxLeft);
        try { x.arc(0, 0, -1, 0, 1); console.log('no error'); } catch (err) { console.log(err instanceof DOMException, err.name); }
        x.save(); x.setTransform({ a: 2, b: 0, c: 0, d: 2, e: 1, f: 1 }); console.log(x.getTransform().a, x.getTransform().e); x.restore();
        </script>",
    );
    assert!(t.errors().is_empty(), "{:?}", t.errors());
    assert_eq!(
        t.logs(),
        vec![
            "[object CanvasRenderingContext2D] true null true",
            "#1e66d0 10px sans-serif 1 1",
            "true 1 [object Uint8ClampedArray] 30,102,208,255 0,170,102,255",
            "true [object CanvasGradient]",
            "italic 700 12px sans-serif true true number",
            "true IndexSizeError",
            "2 1",
        ]
    );
    // the bitmap goes to the embedder once per change, as straight RGBA from the top row
    let up = t.p.take_canvas_uploads();
    assert_eq!(up.len(), 1);
    let (node, w, h, px) = &up[0];
    assert_eq!((*node, *w, *h, px.len()), (t.id("c"), 40, 20, 40 * 20 * 4));
    assert_eq!(&px[0..4], &[30, 102, 208, 255]);
    assert_eq!(&px[(39 * 4)..(40 * 4)], &[0, 170, 102, 255]);
    assert!(t.p.take_canvas_uploads().is_empty());
    // resizing clears it and asks for another upload
    t.p.eval("document.getElementById('c').width = 10; console.log(document.getElementById('c').getContext('2d').getImageData(1, 1, 1, 1).data.join(','))");
    t.step(1.0);
    assert_eq!(t.logs().last(), Some(&"0,0,0,0"));
    let up = t.p.take_canvas_uploads();
    assert_eq!((up.len(), up[0].1, up[0].2), (1, 10, 20));
}

#[test]
fn frames_load_paint_inside_their_element_and_take_input() {
    use invisium_draw::Item;
    let cfg = PageConfig { scripting: true, ..PageConfig::default() };
    let html = "<body style='margin:0'><p style='margin:0;height:40px'>top</p>\
        <iframe id=f src='/inner.html' style='border:0;width:300px;height:100px;display:block'></iframe>\
        <iframe id=self src='/'></iframe>\
        <iframe id=doc srcdoc='<p>inline</p><script>console.log(\"srcdoc ran\")</script>' sandbox></iframe>\
        <script>document.getElementById('f').addEventListener('load', function () { console.log('frame load event'); });</script>";
    let mut p = Page::new("https://example.test/", html, join(), (800.0, 600.0), cfg);
    let fonts = Rc::new(RefCell::new(FontSystem::new()));
    p.set_fonts(fonts.clone());
    p.update(&mut fonts.borrow_mut());
    // only the real frame is asked for: a document may not frame itself, srcdoc needs no fetch
    let reqs = p.take_frame_requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!((reqs[0].node, reqs[0].url.as_str()), (p.document().element_by_id("f"), "https://example.test/inner.html"));
    assert_eq!(p.frames_loading(), 0);
    let before = p.display_list().items.len();
    p.frame_loaded(
        reqs[0].node,
        "https://example.test/inner.html",
        "<body style='margin:0;background:#00f'><a id=l href='/next.html' style='display:block;height:50px'>inner link</a>\
         <script>console.log('inner ran at ' + location.href + ' framed ' + (window.top !== window));</script>",
        false,
    );
    for t in 0..4 {
        p.pump(t as f64);
        p.update(&mut fonts.borrow_mut());
    }
    let log: Vec<String> = p.take_console().into_iter().map(|m| m.text).collect();
    // the sandboxed inline frame runs no script; the real one does, and its element hears `load`
    assert!(log.iter().any(|l| l.starts_with("inner ran at https://example.test/inner.html")), "{log:?}");
    assert!(log.iter().any(|l| l == "frame load event"), "{log:?}");
    assert!(!log.iter().any(|l| l.contains("srcdoc ran")), "{log:?}");
    // the frame's picture sits inside this page's list: clipped, moved to the element, blue
    let list = p.display_list();
    assert!(list.items.len() > before);
    let at = list.items.iter().position(|i| matches!(i, Item::Rect { color, .. } if (color.r, color.g, color.b) == (0, 0, 255))).expect("frame background");
    assert!(matches!(&list.items[at - 1], Item::PushClip { rect, .. } if (rect.x, rect.y, rect.w, rect.h) == (0.0, 40.0, 300.0, 100.0)), "{:?}", list.items[at - 1]);
    assert!(matches!(&list.items[at + 1], Item::PushTransform { m } if (m[4], m[5]) == (0.0, 40.0)));
    // glyph spans of the frame's text point into the combined glyph buffer
    let glyphs = p.glyphs().len() as u32;
    for it in &list.items {
        if let Item::Glyphs { glyphs: span, .. } = it {
            assert!(span.0 <= span.1 && span.1 <= glyphs);
        }
    }
    assert!(list.items.iter().all(|i| !matches!(i, Item::Image { image, .. } if *image >= 0xF000_0000)));
    assert!(p.image_ids().is_empty());
    // hovering and clicking the link inside the frame: the frame navigates, not the page
    let hover = p.mouse_move(20.0, 60.0);
    assert_eq!(hover.link.as_deref(), Some("https://example.test/next.html"));
    assert_eq!(p.click(20.0, 60.0, false), PageAction::Redraw);
    let reqs = p.take_frame_requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].url, "https://example.test/next.html");
    // a click outside the frame is the page's own
    assert_eq!(p.click(20.0, 10.0, false), PageAction::None);
    // every page can be reached by id for routing
    let mut ids = Vec::new();
    p.for_each_page(&mut |pg| ids.push((pg.id(), pg.is_frame())));
    assert_eq!(ids.len(), 3);
    assert!(!ids[0].1 && ids[1].1 && ids[2].1);
    let inner = ids.iter().find(|i| i.1).unwrap().0;
    assert!(p.page_by_id(inner).is_some());
}

#[test]
fn documents_in_frames_talk_by_messages_only() {
    let cfg = PageConfig { scripting: true, ..PageConfig::default() };
    let html = "<body><iframe id=f src='https://widget.test/w.html'></iframe><script>
        var f = document.getElementById('f');
        console.log('top: top is self ' + (window.top === window) + ', parent is self ' + (window.parent === window) + ', document of frame ' + f.contentDocument + ', same window ' + (f.contentWindow === f.contentWindow));
        window.addEventListener('message', function (e) {
            console.log('top got ' + JSON.stringify(e.data) + ' from ' + e.origin + ' source is frame ' + (e.source === f.contentWindow));
            if (e.data.hello) e.source.postMessage({ reply: e.data.hello + 1 }, 'https://widget.test');
            if (e.data.hello) e.source.postMessage({ reply: 'wrong origin' }, 'https://other.test');
        });
        </script>";
    let mut p = Page::new("https://example.test/page", html, join(), (800.0, 600.0), cfg);
    let fonts = Rc::new(RefCell::new(FontSystem::new()));
    p.set_fonts(fonts.clone());
    p.update(&mut fonts.borrow_mut());
    let reqs = p.take_frame_requests();
    assert_eq!(reqs.len(), 1);
    p.frame_loaded(
        reqs[0].node,
        "https://widget.test/w.html",
        "<body><script>
        console.log('frame: top is self ' + (window.top === window) + ', parent is top ' + (window.parent === window.top) + ', frameElement ' + window.frameElement);
        window.addEventListener('message', function (e) {
            console.log('frame got ' + JSON.stringify(e.data) + ' from ' + e.origin + ' source is parent ' + (e.source === window.parent));
        });
        window.parent.postMessage({ hello: 41, list: [1, 'two', null] }, '*');
        try { window.parent.postMessage(function () {}, '*'); } catch (e) { console.log('frame cannot send a function: ' + e.name); }
        </script>",
        false,
    );
    for t in 0..8 {
        p.pump(t as f64);
        p.update(&mut fonts.borrow_mut());
    }
    let log: Vec<String> = p.take_console().into_iter().map(|m| m.text).collect();
    assert_eq!(
        log,
        vec![
            // (a page's own console lines come before those of the documents framed in it)
            "top: top is self true, parent is self true, document of frame null, same window true",
            "top got {\"hello\":41,\"list\":[1,\"two\",null]} from https://widget.test source is frame true",
            "frame: top is self false, parent is top true, frameElement null",
            "frame cannot send a function: DataCloneError",
            "frame got {\"reply\":42} from https://example.test source is parent true",
        ]
    );
}

#[test]
fn inline_elements_report_where_their_text_is() {
    let t = T::new(
        "<body style='margin:0'><p style='margin:0;font:20px/30px sans-serif'><span id=a>one</span> <span id=b>two <i id=c>three</i></span></p><span id=e></span><script>
        function r(id) { var q = document.getElementById(id).getBoundingClientRect(); return [Math.round(q.left), Math.round(q.top), Math.round(q.width), Math.round(q.height)]; }
        var a = r('a'), b = r('b'), c = r('c');
        console.log(a[0] === 0, a[2] > 10, b[0] > a[0] + a[2] - 1, c[0] > b[0], Math.abs(c[0] + c[2] - (b[0] + b[2])) <= 1, a[3] > 10, a[1] >= 0 && a[1] < 30);
        console.log(r('e').join(','));
        </script>",
    );
    assert!(t.errors().is_empty(), "{:?}", t.errors());
    // each word starts after the one before it; an inner element ends where its parent ends
    assert_eq!(t.logs()[0], "true true true true true true true");
}

#[test]
fn computed_style_reports_the_text_properties_pages_copy() {
    let t = T::new(
        "<body><p id=p style='font-size:20px;letter-spacing:-0.05em;text-transform:uppercase;line-height:1.5;word-spacing:2px'>x</p><p id=q>y</p><script>
        var a = getComputedStyle(document.getElementById('p')), b = getComputedStyle(document.getElementById('q'));
        console.log(a.letterSpacing, a.textTransform, a.lineHeight, a.wordSpacing, '|', b.letterSpacing, b.textTransform, b.lineHeight);
        </script>",
    );
    assert_eq!(t.logs(), vec!["-1px uppercase 30px 2px | normal none normal"]);
}

#[test]
fn a_script_can_point_a_frame_somewhere_else() {
    let mut t = T::new(
        "<body><iframe id=f></iframe><script>
        var f = document.getElementById('f');
        f.src = '/first.html';
        var g = document.createElement('iframe');
        g.src = 'https://other.test/second.html';
        document.body.appendChild(g);
        </script>",
    );
    let mut urls: Vec<String> = t.p.take_frame_requests().into_iter().map(|r| r.url).collect();
    urls.sort();
    assert_eq!(urls, vec!["https://example.test/first.html", "https://other.test/second.html"]);
    // the same address again asks for nothing; a new one does
    t.p.eval("document.getElementById('f').src = '/first.html'");
    t.step(1.0);
    assert!(t.p.take_frame_requests().is_empty());
    t.p.eval("document.getElementById('f').src = '/third.html'");
    t.step(2.0);
    assert_eq!(t.p.take_frame_requests().len(), 1);
}

#[test]
fn throwaway_webgl_canvases_do_not_use_up_the_contexts() {
    let cfg = PageConfig { scripting: true, ..PageConfig::default() };
    // pages probe for WebGL with canvases they never attach; the shown canvas must keep working
    let html = "<body><canvas id=shown></canvas><script>
        var shown = document.getElementById('shown').getContext('webgl2'), made = 0;
        for (var i = 0; i < 40; i++) { if (document.createElement('canvas').getContext('webgl2')) made++; }
        shown.clearColor(0, 1, 0, 1); shown.clear(shown.COLOR_BUFFER_BIT);
        console.log(made, shown.getError());
        </script>";
    let mut p = Page::new("https://example.test/", html, join(), (800.0, 600.0), cfg);
    let dev = Rc::new(RefCell::new(RecordingGl::default()));
    p.set_gl_device(dev.clone());
    let fonts = Rc::new(RefCell::new(FontSystem::new()));
    p.set_fonts(fonts.clone());
    p.update(&mut fonts.borrow_mut());
    p.pump(0.0);
    let log: Vec<String> = p.take_console().into_iter().map(|m| m.text).collect();
    assert_eq!(log, vec!["40 0"]);
    // never more than the limit alive, and the canvas in the document was not the one dropped
    assert_eq!(p.canvas_stats().0, 16);
    assert_eq!(dev.borrow().destroyed.len(), 25);
    assert!(!dev.borrow().destroyed.contains(&1));
    assert!(dev.borrow().cmds.iter().any(|c| matches!(c, invisium_draw::gl::Cmd::Clear { .. })));
}

#[test]
fn scripts_hear_the_pointer_move_cross_elements_press_and_release() {
    let mut t = T::new(
        "<style>body{margin:0} div{height:100px}</style>
        <div id=a><p id=in style='margin:0;height:40px'>text</p></div><div id=b></div><a id=l href='/go' style='display:block;height:40px'>go</a>
        <script>
        var log = [];
        function tell(e) {
            var who = e.currentTarget === window ? 'win' : e.currentTarget.id;
            log.push(e.type + '@' + who + (e.target.id ? '>' + e.target.id : '') + ' ' + e.clientX + ',' + e.clientY + ' b' + e.buttons);
        }
        ['pointermove', 'mousemove', 'pointerdown', 'mousedown', 'pointerup', 'mouseup', 'click'].forEach(function (k) { window.addEventListener(k, tell); });
        var a = document.getElementById('a'), b = document.getElementById('b');
        ['mouseenter', 'mouseleave', 'pointerenter', 'mouseover', 'mouseout'].forEach(function (k) { a.addEventListener(k, tell); });
        b.addEventListener('pointerdown', function (e) { b.setPointerCapture(e.pointerId); });
        b.addEventListener('pointermove', function (e) { log.push('captured ' + b.hasPointerCapture(1) + ' ' + e.movementX + ',' + e.movementY + ' off ' + e.offsetX + ',' + e.offsetY); });
        function flush() { console.log(log.join(' | ')); log = []; }
        </script>",
    );
    assert_eq!(t.errors(), Vec::<&String>::new(), "console: {:?}", t.log);
    let say = |t: &mut T, now: f64| {
        t.p.frame(now);
        t.p.eval("flush()");
        t.step(now);
        t.log.pop().unwrap_or_default()
    };
    // three reports from the mouse before a frame are one move for scripts, and the page asks
    // for that frame
    t.p.mouse_move(5.0, 5.0);
    t.p.mouse_move(8.0, 8.0);
    t.p.mouse_move(10.0, 10.0);
    assert!(t.p.wants_frame());
    assert_eq!(
        say(&mut t, 1.0),
        "log: pointerenter@a>a 10,10 b0 | mouseover@a>in 10,10 b0 | mouseenter@a>a 10,10 b0 | pointermove@win>in 10,10 b0 | mousemove@win>in 10,10 b0"
    );
    assert!(!t.p.wants_frame());
    // inside the same element nothing is crossed; out of it, the element hears it leave
    t.p.mouse_move(10.0, 60.0);
    assert_eq!(say(&mut t, 2.0), "log: mouseout@a>in 10,60 b0 | mouseover@a>a 10,60 b0 | pointermove@win>a 10,60 b0 | mousemove@win>a 10,60 b0");
    t.p.mouse_move(10.0, 150.0);
    assert_eq!(say(&mut t, 3.0), "log: mouseout@a>a 10,150 b0 | mouseleave@a>a 10,150 b0 | captured false 0,90 off 10,50 | pointermove@win>b 10,150 b0 | mousemove@win>b 10,150 b0");
    // a press, a drag with the pointer captured, and a release over another element: while
    // captured, the pointer counts as being over the element that holds it; the click is at
    // what holds both elements (here the body)
    t.p.press(10.0, 150.0, crate::page::PointerButton::Primary);
    t.p.mouse_move(30.0, 20.0);
    assert_ne!(t.p.release(30.0, 20.0, crate::page::PointerButton::Primary, false), PageAction::Navigate { url: "https://example.test/go".into(), new_tab: false });
    assert_eq!(
        say(&mut t, 4.0),
        "log: pointerdown@win>b 10,150 b1 | mousedown@win>b 10,150 b1 | captured true 20,-130 off 30,-80 | pointermove@win>b 30,20 b1 | mousemove@win>b 30,20 b1 | pointerup@win>b 30,20 b0 | mouseup@win>b 30,20 b0 | pointerenter@a>a 30,20 b0 | mouseover@a>in 30,20 b0 | mouseenter@a>a 30,20 b0 | click@win 30,20 b0"
    );
    // down and up on the link is a click that follows it
    assert_eq!(t.p.click(10.0, 210.0, false), PageAction::Navigate { url: "https://example.test/go".into(), new_tab: false });
    let said = say(&mut t, 5.0);
    assert!(said.ends_with("pointerdown@win>l 10,210 b1 | mousedown@win>l 10,210 b1 | pointerup@win>l 10,210 b0 | mouseup@win>l 10,210 b0 | click@win>l 10,210 b0"), "{said}");
    // the pointer leaves the page
    t.p.mouse_move(10.0, 10.0);
    say(&mut t, 6.0);
    t.p.mouse_leave();
    assert_eq!(say(&mut t, 7.0), "log: mouseout@a>in 10,10 b0 | mouseleave@a>a 10,10 b0");
    // what is drawn at a point
    t.p.eval("console.log(document.elementFromPoint(10, 10).id, document.elementFromPoint(10, 150).id, document.elementFromPoint(-5, 10), document.elementsFromPoint(10, 10).length)");
    t.step(8.0);
    assert_eq!(t.log.pop().unwrap(), "log: in b null 4");
}

#[test]
fn base64_round_trips_and_rejects_what_is_not_base64() {
    let mut t = T::new(
        "<script>
        var all = ''; for (var i = 0; i < 256; i++) all += String.fromCharCode(i);
        var enc = btoa(all), dec = atob(enc), same = dec.length === 256;
        for (var i = 0; i < 256; i++) if (dec.charCodeAt(i) !== i) same = false;
        function tries(f) { try { return f(); } catch (e) { return e.name; } }
        console.log(btoa('hi'), btoa('hey'), btoa(''), atob('aGk='), atob('aGk'), atob(' aG V5 '), same, enc.length);
        console.log(tries(function () { return atob('a'); }), tries(function () { return atob('a=bc'); }), tries(function () { return atob('ab!c'); }),
            tries(function () { return btoa(String.fromCharCode(256)); }), tries(function () { return atob('ab=='); }).charCodeAt(0), tries(function () { return atob('ab='); }));
        </script>",
    );
    assert_eq!(t.errors(), Vec::<&String>::new(), "console: {:?}", t.log);
    assert_eq!(t.logs(), vec!["aGk= aGV5  hi hi hey true 344", "InvalidCharacterError InvalidCharacterError InvalidCharacterError InvalidCharacterError 105 InvalidCharacterError"]);
}

#[test]
fn focus_follows_the_press_and_a_press_can_end_without_its_release() {
    use crate::page::PointerButton::{Middle, Primary};
    let mut t = T::new(
        "<style>body{margin:0} input{display:block;height:30px;width:200px;margin:0;padding:0;border:0} div{height:100px} a{display:block;height:40px}</style>
        <input id=i><div id=d></div><a id=l href='/go'>go</a><a id=m href='/other'>other</a>
        <script>
        var log = [];
        window.addEventListener('pointermove', function (e) { log.push('move b' + e.buttons); });
        window.addEventListener('pointerup', function (e) { log.push('up b' + e.buttons); });
        window.addEventListener('auxclick', function (e) { log.push('aux ' + e.target.id + ' ' + e.button); });
        function flush() { console.log(document.activeElement.id + ' | ' + log.join(' ')); log = []; }
        </script>",
    );
    assert_eq!(t.errors(), Vec::<&String>::new(), "console: {:?}", t.log);
    let say = |t: &mut T, now: f64| {
        t.p.frame(now);
        t.p.eval("flush()");
        t.step(now);
        t.log.pop().unwrap_or_default()
    };
    // a text control has the focus as soon as the button is down on it, and keeps it when the
    // button comes up somewhere else
    t.p.press(10.0, 10.0, Primary);
    assert_eq!(say(&mut t, 1.0), "log: i | ");
    t.p.mouse_move(10.0, 80.0);
    t.p.release(10.0, 80.0, Primary, false);
    assert_eq!(say(&mut t, 2.0), "log: i | move b1 up b0");
    // a press on plain content takes the focus away
    t.p.press(10.0, 80.0, Primary);
    assert_eq!(say(&mut t, 3.0), "log:  | ");
    // the pointer is taken away in the middle of a press (another tab came to the front): the
    // button counts as up, and later moves say so
    t.p.release_all();
    t.p.mouse_move(10.0, 90.0);
    assert_eq!(say(&mut t, 4.0), "log:  | up b0 move b0");
    assert!(!t.p.release_all());
    // the middle button opens a link in a new tab only if it went down and came up on it
    t.p.press(10.0, 140.0, Middle);
    assert_ne!(t.p.release(10.0, 180.0, Middle, false), PageAction::Navigate { url: "https://example.test/other".into(), new_tab: true });
    t.p.press(10.0, 140.0, Middle);
    assert_eq!(t.p.release(10.0, 150.0, Middle, false), PageAction::Navigate { url: "https://example.test/go".into(), new_tab: true });
    let said = say(&mut t, 5.0);
    assert!(said.contains("aux  1") && said.ends_with("aux l 1"), "{said}");
    // below the content there is still the root element
    t.p.eval("console.log(document.elementFromPoint(10, 500).tagName, document.elementFromPoint(10, 700))");
    t.step(6.0);
    assert_eq!(t.log.pop().unwrap(), "log: HTML null");
}

#[test]
fn a_script_talks_through_a_websocket_the_embedder_opened() {
    use crate::page::{PageSocket, SocketEvent};
    struct Wire(Rc<RefCell<Vec<String>>>);
    impl PageSocket for Wire {
        fn send_text(&self, t: String) {
            self.0.borrow_mut().push(format!("text {t}"));
        }
        fn send_binary(&self, d: Vec<u8>) {
            self.0.borrow_mut().push(format!("binary {d:?}"));
        }
        fn close(&self, code: Option<u16>, reason: &str) {
            self.0.borrow_mut().push(format!("close {code:?} {reason}"));
        }
        fn buffered(&self) -> u64 {
            7
        }
    }
    let mut t = T::new(
        "<script>
        var log = [];
        var ws = new WebSocket('wss://chat.example.test/room?x=1', ['a', 'b']);
        ws.binaryType = 'arraybuffer';
        ws.onopen = function () {
            log.push('open ' + ws.protocol + ' ' + ws.readyState + ' ' + ws.bufferedAmount);
            ws.send('hi'); ws.send(new Uint8Array([1, 2, 3]).subarray(1));
        };
        ws.addEventListener('message', function (e) {
            log.push('message ' + (typeof e.data === 'string' ? e.data : 'bytes ' + new Uint8Array(e.data).join(',')) + ' from ' + e.origin);
            if (e.data === 'bye') ws.close(3000, 'done');
        });
        ws.onerror = function () { log.push('error'); };
        ws.onclose = function (e) { log.push('close ' + e.code + ' ' + e.reason + ' ' + e.wasClean + ' ' + ws.readyState); };
        function tries(f) { try { f(); return 'ok'; } catch (e) { return e.name; } }
        var bad = [tries(function () { new WebSocket('ftp://x.test/'); }), tries(function () { new WebSocket('wss://x.test/#frag'); }),
            tries(function () { new WebSocket('wss://x.test/', ['a', 'a']); }), tries(function () { ws.close(1001); }),
            tries(function () { ws.send('too early'); })].join(' ');
        var refused = new WebSocket('/relative');
        refused.onerror = function () { log.push('refused error'); };
        refused.onclose = function (e) { log.push('refused close ' + e.code + ' ' + e.wasClean); };
        var dropped = new WebSocket('wss://later.example.test/');
        dropped.close();
        function flush() { console.log(log.join(' | ')); log = []; }
        console.log(bad, ws.url, refused.url, ws.readyState, WebSocket.OPEN, dropped.readyState);
        </script>",
    );
    assert_eq!(t.errors(), Vec::<&String>::new(), "console: {:?}", t.log);
    assert_eq!(
        t.logs(),
        vec!["SyntaxError SyntaxError SyntaxError InvalidAccessError InvalidStateError wss://chat.example.test/room?x=1 wss://example.test/relative 0 1 2"]
    );
    let reqs = t.p.take_socket_requests();
    assert_eq!(reqs.len(), 3);
    assert_eq!((reqs[0].url.as_str(), reqs[0].protocols.join("+")), ("wss://chat.example.test/room?x=1", "a+b".to_string()));
    assert_eq!(reqs[1].url, "wss://example.test/relative");
    assert!(t.p.take_socket_requests().is_empty());
    let wire = Rc::new(RefCell::new(Vec::new()));
    t.p.socket_opened(reqs[0].id, Box::new(Wire(wire.clone())));
    t.p.socket_event(reqs[0].id, SocketEvent::Open { protocol: "b".into() });
    t.p.socket_event(reqs[0].id, SocketEvent::Text("hello".into()));
    t.p.socket_event(reqs[0].id, SocketEvent::Binary(vec![9, 8]));
    t.p.socket_event(reqs[0].id, SocketEvent::Text("bye".into()));
    // the network stack refuses the second one
    t.p.socket_event(reqs[1].id, SocketEvent::Closed { code: 1006, reason: String::new(), clean: false });
    t.p.socket_event(reqs[0].id, SocketEvent::Closed { code: 3000, reason: "done".into(), clean: true });
    // an event for a socket that is gone is nobody's
    t.p.socket_event(reqs[0].id, SocketEvent::Text("late".into()));
    t.p.eval("flush()");
    t.step(1.0);
    assert_eq!(
        t.log.pop().unwrap(),
        "log: open b 1 7 | message hello from wss://chat.example.test | message bytes 9,8 from wss://chat.example.test | message bye from wss://chat.example.test \
         | refused error | refused close 1006 false | close 3000 done true 3"
            .replace("          ", "")
    );
    assert_eq!(wire.borrow().as_slice(), ["text hi", "binary [2, 3]", "close Some(3000) done"]);
    // the third was closed by the script before the connection existed: it is closed as soon
    // as there is one, and closed again when the document goes away
    let late = Rc::new(RefCell::new(Vec::new()));
    t.p.socket_opened(reqs[2].id, Box::new(Wire(late.clone())));
    assert_eq!(late.borrow().as_slice(), ["close None "]);
    assert_eq!(t.p.sockets(), 1);
    drop(t);
    assert_eq!(late.borrow().as_slice(), ["close None ", "close None "]);
}

#[test]
fn a_closing_websocket_still_hears_the_server_and_text_coding_is_native() {
    use crate::page::{PageSocket, SocketEvent};
    struct Quiet;
    impl PageSocket for Quiet {
        fn send_text(&self, _: String) {}
        fn send_binary(&self, _: Vec<u8>) {}
        fn close(&self, _: Option<u16>, _: &str) {}
        fn buffered(&self) -> u64 {
            0
        }
    }
    let mut t = T::new(
        "<script>
        var log = [];
        var ws = new WebSocket('wss://live.example.test/');
        ws.onopen = function () { ws.send('last question'); ws.close(); };
        ws.onmessage = function (e) { log.push('answer ' + e.data + ' in state ' + ws.readyState); };
        ws.onclose = function (e) { log.push('closed ' + e.wasClean); console.log(log.join(' | ')); };
        var bytes = new TextEncoder().encode('h\\u00e9 \\u20ac \\ud83d\\ude00');
        var odd = new Uint8Array([0xEF, 0xBB, 0xBF, 0x68, 0xC3, 0xA9, 0xFF, 0x21]);
        console.log(Array.prototype.join.call(bytes, ','), new TextDecoder().decode(bytes) === 'h\\u00e9 \\u20ac \\ud83d\\ude00',
            new TextDecoder().decode(odd).length, new TextDecoder().decode(odd).charCodeAt(2), new TextDecoder().decode(new Uint8Array(0)) === '');
        </script>",
    );
    assert_eq!(t.errors(), Vec::<&String>::new(), "console: {:?}", t.log);
    assert_eq!(t.logs(), vec!["104,195,169,32,226,130,172,32,240,159,152,128 true 4 65533 true"]);
    let reqs = t.p.take_socket_requests();
    t.p.socket_opened(reqs[0].id, Box::new(Quiet));
    t.p.socket_event(reqs[0].id, SocketEvent::Open { protocol: String::new() });
    t.p.socket_event(reqs[0].id, SocketEvent::Text("42".into()));
    t.p.socket_event(reqs[0].id, SocketEvent::Closed { code: 1000, reason: String::new(), clean: true });
    t.step(1.0);
    assert_eq!(t.log.pop().unwrap(), "log: answer 42 in state 2 | closed true");
}

#[test]
fn grid_lines_can_be_named_and_lengths_rounded_to_a_step() {
    let mut t = T::new(
        "<style>
        body { margin: 0 }
        #g { display: grid; width: 420px; grid-template-columns: [gutter] 20px [main start] 300px [side] 100px [end]; grid-template-rows: [top] 50px [bottom] 30px }
        #a { grid-area: top / main; height: 10px }
        #b { grid-area: top / side }
        #c { grid-area: bottom / 1 / bottom / -1 }
        #e { grid-row: top; grid-column: nowhere }
        #r { width: round(down, 107px, 8px); height: round(up, 101px, 10px); margin-left: round(to-zero, 19.9px, 5px) }
        #v { --w: round(down, max(26rem, min(600px, 507px)), .8rem); width: var(--w); height: calc(var(--w) * 1) }
        #n { width: round(nearest, 104px, 0px); height: 5px }
        </style>
        <div id=g><div id=a></div><div id=b></div><div id=c></div><div id=e></div></div><div id=r></div><div id=v></div><div id=n></div>
        <script>
        function r(id) { var q = document.getElementById(id).getBoundingClientRect(), f = function (x) { return Math.round(x * 10) / 10; }; return id + ' ' + f(q.left) + ',' + f(q.top) + ' ' + f(q.width) + 'x' + f(q.height); }
        console.log(['a', 'b', 'c', 'e', 'r', 'v', 'n'].map(r).join(' | '));
        </script>",
    );
    assert_eq!(t.errors(), Vec::<&String>::new(), "console: {:?}", t.log);
    // `a` and `b` sit in the columns their names give, `c` spans the named bottom row from the
    // first line to the last, and a name the grid does not have places like `auto` (`e` takes
    // the first free cell of the top row); lengths snap to their steps, a step of 0 changes nothing
    assert_eq!(t.logs(), vec!["a 20,0 300x10 | b 320,0 100x50 | c 0,50 420x30 | e 0,0 20x50 | r 15,80 104x110 | v 0,190 499.2x499.2 | n 0,689.2 104x5"]);
}
