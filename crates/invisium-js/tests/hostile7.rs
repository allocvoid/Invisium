//! G4: huge Unicode identifiers and combining-mark floods lex in linear time or fail with a
//! SyntaxError; a species constructor that re-enters `then` ends as a stack overflow.

use invisium_js::{ErrorKind, Limits, Runtime};
use std::time::{Duration, Instant};

const N: usize = 100_000;

fn eval(src: &str) -> Result<String, ErrorKind> {
    let mut rt = Runtime::new(Limits::default());
    match rt.eval(src, "hostile.js") {
        Ok(v) => Ok(rt.to_string_lossy(v)),
        Err(e) => Err(e.kind),
    }
}

fn timed(src: &str) -> (Result<String, ErrorKind>, Duration) {
    let t = Instant::now();
    let r = eval(src);
    (r, t.elapsed())
}

#[test]
fn a_hundred_thousand_character_identifier() {
    let ascii = "a".repeat(N);
    let (r, d) = timed(&format!("var {0} = 1; {0} + 1", ascii));
    assert_eq!(r, Ok("2".to_string()));
    assert!(d < Duration::from_secs(5), "ascii identifier took {:?}", d);

    let cyrillic = "ж".repeat(N);
    let (r, d) = timed(&format!("var {0} = 2; {0} * 2", cyrillic));
    assert_eq!(r, Ok("4".to_string()));
    assert!(d < Duration::from_secs(5), "cyrillic identifier took {:?}", d);

    let marks = format!("\u{1000}{}", "\u{103a}".repeat(N));
    let (r, d) = timed(&format!("var {0} = 3; {0}", marks));
    assert_eq!(r, Ok("3".to_string()));
    assert!(d < Duration::from_secs(5), "combining-mark identifier took {:?}", d);

    let escaped = "\\u0061".repeat(N / 6);
    let (r, d) = timed(&format!("var {} = 4; typeof a", escaped));
    assert_eq!(r, Ok("undefined".to_string()));
    assert!(d < Duration::from_secs(5), "escaped identifier took {:?}", d);
}

#[test]
fn a_source_of_a_hundred_thousand_combining_marks() {
    let (r, d) = timed(&"\u{301}".repeat(N));
    assert_eq!(r, Err(ErrorKind::Syntax));
    assert!(d < Duration::from_secs(5), "took {:?}", d);
    let (r, d) = timed(&"\u{103a}\u{200d}".repeat(N / 2));
    assert_eq!(r, Err(ErrorKind::Syntax));
    assert!(d < Duration::from_secs(5), "took {:?}", d);
}

#[test]
fn a_species_constructor_that_re_enters_then() {
    let src = "var p = Promise.resolve(); function S(ex) { p.then(); ex(function () {}, function () {}); } p.constructor = { [Symbol.species]: S }; p.then();";
    assert_eq!(eval(src), Err(ErrorKind::StackOverflow));
    // through `new` on a class whose constructor calls then on a promise of its own species
    let src = "class P extends Promise { constructor(ex) { super(ex); P.resolve(1).then(); } } P.resolve(0);";
    assert_eq!(eval(src), Err(ErrorKind::StackOverflow));
}
