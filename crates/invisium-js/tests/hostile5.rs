//! F5 (Intl part): absurd `Intl` inputs end in errors or bounded work, never a panic or an
//! unbounded loop. Runs on the default 2 MiB test-thread stack.

use invisium_js::{ErrorKind, Limits, Runtime};

fn small() -> Runtime {
    Runtime::new(Limits { step_budget: 2_000_000, ..Limits::default() })
}

/// Evaluates `src`; `Ok(text)` or `Err((kind, name))`.
fn eval(rt: &mut Runtime, src: &str) -> Result<String, (ErrorKind, String)> {
    match rt.eval(src, "hostile.js") {
        Ok(v) => Ok(rt.to_string_lossy(v)),
        Err(e) => Err((e.kind, e.name)),
    }
}

#[test]
fn huge_locale_lists_and_iterables_stop_on_the_step_budget() {
    let mut rt = small();
    assert_eq!(eval(&mut rt, "Intl.getCanonicalLocales({ length: 2 ** 32 })").unwrap_err().0, ErrorKind::StepBudget);
    let mut rt = small();
    let r = eval(&mut rt, "new Intl.ListFormat().format({ [Symbol.iterator]: function* () { while (true) yield 'a' } })");
    assert!(matches!(r, Err((ErrorKind::StepBudget, _)) | Err((ErrorKind::OutOfMemory, _))), "{:?}", r);
    let mut rt = small();
    let r = eval(&mut rt, "new Intl.NumberFormat('en', { notation: 'scientific' }).format(10n ** 100000n)");
    assert!(matches!(r, Ok(_) | Err((ErrorKind::StepBudget, _))), "{:?}", r);
}

#[test]
fn absurd_option_values_are_range_errors() {
    let mut rt = Runtime::new(Limits::default());
    let src = "var r = []; for (var f of [
        () => new Intl.NumberFormat('en', { maximumFractionDigits: 1e9 }),
        () => new Intl.NumberFormat('en', { minimumIntegerDigits: -1 }),
        () => new Intl.NumberFormat('en', { maximumSignificantDigits: Infinity }),
        () => new Intl.DateTimeFormat('en', { fractionalSecondDigits: 1e9 }),
        () => new Intl.DateTimeFormat('en').format(NaN),
        () => new Intl.DateTimeFormat('en').format(1e300),
        () => new Intl.RelativeTimeFormat('en').format(Infinity, 'day'),
        () => new Intl.PluralRules('en', { minimumFractionDigits: 1e9 }),
        () => Intl.getCanonicalLocales('x'.repeat(100000)),
        () => new Intl.DateTimeFormat('en', { timeZone: 'x'.repeat(1000) }),
    ]) { try { f(); r.push('ok') } catch (e) { r.push(e.name) } } r.join()";
    assert_eq!(eval(&mut rt, src).unwrap(), vec!["RangeError"; 10].join(","));
}

#[test]
fn extreme_but_valid_inputs_finish() {
    let mut rt = Runtime::new(Limits::default());
    let src = "[new Intl.NumberFormat('en', { minimumFractionDigits: 100 }).format(1e308).length,
        new Intl.NumberFormat('en', { maximumFractionDigits: 100 }).format(5e-324),
        new Intl.NumberFormat('en', { notation: 'compact' }).format(Number.MAX_VALUE).length,
        [...new Intl.Segmenter('en', { granularity: 'word' }).segment('ab '.repeat(20000))].length,
        new Intl.ListFormat().format(Array(100000).fill('a')).length,
        'a'.repeat(1e6).localeCompare('a'.repeat(1e6) + 'b')].join()";
    let out = eval(&mut rt, src).expect("valid inputs format");
    let parts: Vec<&str> = out.split(',').collect();
    // 1e308 has 309 integer digits, 102 group separators, a point and 100 fraction digits
    assert_eq!(parts[0], "512");
    assert_eq!(parts[1], "0");
    assert_eq!(parts[3], "40000");
    assert_eq!(parts[5], "-1");
}
