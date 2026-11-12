//! `Intl.DateTimeFormat` and the `Date.prototype.toLocale*String` methods routed through it.
//! `T` is Tuesday 2024-03-05 00:07:03.678 UTC; the runtime clock is fixed at 01:07:03.678.

macro_rules! dt {
    ($s:expr) => {
        concat!(
            "var T = Date.UTC(2024, 2, 5, 0, 7, 3, 678); ",
            "var F = (o, t) => new Intl.DateTimeFormat('en-US', o).format(t === undefined ? T : t); ",
            $s
        )
    };
}

pub const CASES: &[(&str, &str)] = &[
    // --- styles
    (dt!("F()"), "3/5/2024"),
    (dt!("F({ dateStyle: 'full' })"), "Tuesday, March 5, 2024"),
    (dt!("[F({ dateStyle: 'long' }), F({ dateStyle: 'medium' }), F({ dateStyle: 'short' })].join('|')"), "March 5, 2024|Mar 5, 2024|3/5/24"),
    (dt!("F({ timeStyle: 'full' })"), "12:07:03 AM Coordinated Universal Time"),
    (dt!("[F({ timeStyle: 'long' }), F({ timeStyle: 'medium' }), F({ timeStyle: 'short' })].join('|')"), "12:07:03 AM UTC|12:07:03 AM|12:07 AM"),
    (dt!("F({ dateStyle: 'long', timeStyle: 'short' })"), "March 5, 2024 at 12:07 AM"),
    (dt!("F({ dateStyle: 'full', timeStyle: 'long' })"), "Tuesday, March 5, 2024 at 12:07:03 AM UTC"),
    (dt!("[F({ dateStyle: 'medium', timeStyle: 'medium' }), F({ dateStyle: 'short', timeStyle: 'short' })].join('|')"), "Mar 5, 2024, 12:07:03 AM|3/5/24, 12:07 AM"),
    // --- date components
    (dt!("F({ month: 'long', day: 'numeric' })"), "March 5"),
    (dt!("[F({ year: 'numeric', month: 'long' }), F({ year: 'numeric', month: 'short' })].join('|')"), "March 2024|Mar 2024"),
    (dt!("F({ year: 'numeric', month: 'short', day: 'numeric' })"), "Mar 5, 2024"),
    (dt!("[F({ weekday: 'long' }), F({ weekday: 'short' }), F({ weekday: 'narrow' })].join('|')"), "Tuesday|Tue|T"),
    (dt!("[F({ month: 'narrow' }), F({ month: '2-digit' }), F({ month: 'numeric' }), F({ month: 'long' })].join('|')"), "M|03|3|March"),
    (dt!("F({ year: 'numeric', month: '2-digit', day: '2-digit' })"), "03/05/2024"),
    (dt!("[F({ year: '2-digit', month: 'numeric', day: 'numeric' }), F({ month: '2-digit', day: '2-digit' })].join('|')"), "3/5/24|03/05"),
    (dt!("[F({ day: 'numeric' }), F({ year: 'numeric' }), F({ day: '2-digit' })].join('|')"), "5|2024|05"),
    (dt!("F({ weekday: 'short', year: 'numeric', month: 'short', day: 'numeric' })"), "Tue, Mar 5, 2024"),
    (dt!("F({ weekday: 'long', month: 'long', day: 'numeric' })"), "Tuesday, March 5"),
    (dt!("[F({ year: 'numeric', era: 'short' }), F({ year: 'numeric', month: 'long', day: 'numeric', era: 'long' })].join('|')"), "2024 AD|March 5, 2024 Anno Domini"),
    (dt!("F({ year: 'numeric', era: 'short' }, Date.UTC(-100, 0, 1))"), "101 BC"),
    // --- time components, hour cycles, midnight and noon
    (dt!("[F({ hour: 'numeric', minute: '2-digit' }), F({ hour: 'numeric' })].join('|')"), "12:07 AM|12 AM"),
    (dt!("F({ hour: 'numeric', minute: 'numeric', hour12: false })"), "00:07"),
    (dt!("[12, 13, 23].map(h => F({ hour: 'numeric', minute: '2-digit' }, Date.UTC(2024, 0, 1, h, h === 13 ? 30 : 0))).join('|')"), "12:00 PM|1:30 PM|11:00 PM"),
    (dt!("F({ hour: 'numeric', minute: '2-digit', hourCycle: 'h11' }, Date.UTC(2024, 0, 1, 0, 5))"), "0:05 AM"),
    (dt!("[F({ hour: 'numeric', minute: '2-digit', hourCycle: 'h23' }, Date.UTC(2024, 0, 1, 12)), F({ hour: 'numeric', minute: '2-digit', hourCycle: 'h24' }, Date.UTC(2024, 0, 1, 0, 5))].join('|')"), "12:00|24:05"),
    (dt!("F({ hour: 'numeric', hourCycle: 'h23', hour12: true }, Date.UTC(2024, 0, 1, 15))"), "3 PM"),
    (dt!("F({ hour: '2-digit', minute: '2-digit' }, Date.UTC(2024, 0, 1, 9, 5))"), "09:05 AM"),
    (dt!("[F({ minute: '2-digit', second: '2-digit', fractionalSecondDigits: 3 }), F({ second: 'numeric', fractionalSecondDigits: 1 })].join('|')"), "07:03.678|3.6"),
    (dt!("F({ hour: 'numeric', minute: '2-digit', second: '2-digit', hour12: false }, Date.UTC(2024, 0, 1, 23, 59, 59))"), "23:59:59"),
    // --- time zone names
    (dt!("F({ timeZoneName: 'short' })"), "3/5/2024, UTC"),
    (dt!("F({ hour: 'numeric', timeZoneName: 'long' })"), "12 AM Coordinated Universal Time"),
    (dt!("F({ hour: 'numeric', timeZone: 'America/New_York', timeZoneName: 'short' })"), "12 AM UTC"),
    // --- Date.prototype.toLocale*String
    (dt!("var d = new Date(T); [d.toLocaleString(), d.toLocaleDateString(), d.toLocaleTimeString()].join('|')"), "3/5/2024, 12:07:03 AM|3/5/2024|12:07:03 AM"),
    (dt!("new Date(T).toLocaleDateString('en-US', { month: 'long', day: 'numeric', year: 'numeric' })"), "March 5, 2024"),
    (dt!("new Date(T).toLocaleTimeString('en-US', { hour: '2-digit', minute: '2-digit' })"), "12:07 AM"),
    (dt!("[new Date(T).toLocaleDateString(undefined, { weekday: 'long' }), new Date(T).toLocaleTimeString([], { hour12: false })].join('|')"), "Tuesday|00:07:03"),
    (dt!("[new Date(T).toLocaleString('de-DE'), new Date(T).toLocaleString('ja-JP', { timeZone: 'Asia/Tokyo' })].join('|')"), "3/5/2024, 12:07:03 AM|3/5/2024, 12:07:03 AM"),
    (dt!("new Date(T).toLocaleDateString('en-US', { hour: 'numeric' })"), "3/5/2024, 12 AM"),
    (dt!("new Date(T).toLocaleString('en-US', { dateStyle: 'medium', timeStyle: 'short' })"), "Mar 5, 2024, 12:07 AM"),
    (dt!("var r = []; for (var f of [() => new Date(T).toLocaleDateString('en', { timeStyle: 'short' }), () => new Date(T).toLocaleTimeString('en', { dateStyle: 'short' })]) { try { f() } catch (e) { r.push(e.name) } } r.join()"), "TypeError,TypeError"),
    (dt!("new Date(NaN).toLocaleString() + '|' + new Date(NaN).toLocaleDateString()"), "Invalid Date|Invalid Date"),
    // --- resolvedOptions (privacy: always en-US and UTC)
    (dt!("var r = new Intl.DateTimeFormat('de-DE', { timeZone: 'America/New_York' }).resolvedOptions(); [r.locale, r.timeZone, r.calendar, r.numberingSystem, r.year, r.month, r.day].join()"), "en-US,UTC,gregory,latn,numeric,numeric,numeric"),
    (dt!("var r = new Intl.DateTimeFormat().resolvedOptions(); r.locale + '|' + r.timeZone + '|' + ('hour' in r)"), "en-US|UTC|false"),
    (dt!("var r = new Intl.DateTimeFormat('en', { hour: 'numeric' }).resolvedOptions(); [r.hourCycle, r.hour12, r.hour, 'year' in r].join()"), "h12,true,numeric,false"),
    (dt!("var r = new Intl.DateTimeFormat('fr', { dateStyle: 'long', timeStyle: 'short', hour12: false }).resolvedOptions(); [r.dateStyle, r.timeStyle, r.hourCycle, r.locale].join()"), "long,short,h23,en-US"),
    // --- formatToParts and the bound format
    (dt!("new Intl.DateTimeFormat('en-US', { dateStyle: 'medium' }).formatToParts(T).map(p => p.type + '=' + p.value).join()"), "month=Mar,literal= ,day=5,literal=, ,year=2024"),
    (dt!("new Intl.DateTimeFormat('en-US', { hour: 'numeric', minute: '2-digit' }).formatToParts(T).map(p => p.type + '=' + p.value).join()"), "hour=12,literal=:,minute=07,literal= ,dayPeriod=AM"),
    (dt!("var x = new Intl.DateTimeFormat('en-US'); var f = x.format; [T, Date.UTC(2020, 0, 1)].map(f).join('|') + '|' + (f === x.format) + '|' + f.length"), "3/5/2024|1/1/2020|true|1"),
    (dt!("[new Intl.DateTimeFormat('en-US', { timeStyle: 'medium' }).format(), Intl.DateTimeFormat('en-US').format(new Date(T))].join('|')"), "1:07:03 AM|3/5/2024"),
    // --- formatRange
    (dt!("new Intl.DateTimeFormat('en-US', { month: 'short', day: 'numeric', year: 'numeric' }).formatRange(T, Date.UTC(2024, 2, 7))"), "Mar 5\u{2009}\u{2013}\u{2009}7, 2024"),
    (dt!("new Intl.DateTimeFormat('en-US', { month: 'short', day: 'numeric', year: 'numeric' }).formatRange(T, Date.UTC(2024, 3, 7))"), "Mar 5\u{2009}\u{2013}\u{2009}Apr 7, 2024"),
    (dt!("new Intl.DateTimeFormat('en-US', { month: 'short', day: 'numeric', year: 'numeric' }).formatRange(Date.UTC(2023, 11, 30), Date.UTC(2024, 0, 2))"), "Dec 30, 2023\u{2009}\u{2013}\u{2009}Jan 2, 2024"),
    (dt!("new Intl.DateTimeFormat('en-US').formatRange(Date.UTC(2024, 0, 1), Date.UTC(2024, 0, 5))"), "1/1/2024\u{2009}\u{2013}\u{2009}1/5/2024"),
    (dt!("var x = new Intl.DateTimeFormat('en-US', { hour: 'numeric', minute: '2-digit' }); x.formatRange(Date.UTC(2024, 0, 1, 10), Date.UTC(2024, 0, 1, 11, 30)) + '|' + x.formatRange(Date.UTC(2024, 0, 1, 10), Date.UTC(2024, 0, 1, 14))"), "10:00\u{2009}\u{2013}\u{2009}11:30 AM|10:00 AM\u{2009}\u{2013}\u{2009}2:00 PM"),
    (dt!("new Intl.DateTimeFormat('en-US', { dateStyle: 'medium' }).formatRange(T, T + 1000)"), "Mar 5, 2024"),
    (dt!("new Intl.DateTimeFormat('en-US', { hour: 'numeric', minute: '2-digit' }).formatRangeToParts(Date.UTC(2024, 0, 1, 10), Date.UTC(2024, 0, 1, 11, 30)).map(p => p.source === 'shared' ? 'S' : p.source[0]).join('')"), "sssSeeeee"),
    // --- errors, tags and the namespace
    (dt!("var r = []; for (var o of [{ dateStyle: 'long', year: 'numeric' }, { month: 'huge' }, { fractionalSecondDigits: 5 }, { timeZone: 'Bad Zone!' }, { hourCycle: 'h13' }]) { try { new Intl.DateTimeFormat('en', o); r.push('ok') } catch (e) { r.push(e.name) } } r.join()"), "TypeError,RangeError,RangeError,RangeError,RangeError"),
    (dt!("var r = []; for (var l of ['not a tag!', 'en-', '', 5]) { try { new Intl.DateTimeFormat(l); r.push('ok') } catch (e) { r.push(e.name) } } r.join()"), "RangeError,RangeError,RangeError,ok"),
    (dt!("var r = []; for (var t of [NaN, Infinity, 8.64e15 + 1]) { try { F({}, t); r.push('ok') } catch (e) { r.push(e.name) } } r.join()"), "RangeError,RangeError,RangeError"),
    (dt!("[Object.prototype.toString.call(new Intl.DateTimeFormat()), typeof Intl, String(Intl), typeof Intl.DateTimeFormat.prototype.formatToParts].join()"), "[object Intl.DateTimeFormat],object,[object Intl],function"),
    (dt!("Intl.DateTimeFormat.supportedLocalesOf(['en-US', 'de-DE', 'en', 'en-GB']).join()"), "en-US,en"),
    (dt!("Intl.getCanonicalLocales(['EN-us', 'zh-hant-tw', 'en-US']).join() + '|' + Intl.getCanonicalLocales('DE').join()"), "en-US,zh-Hant-TW|de"),
    (dt!("var r = []; try { Intl.DateTimeFormat.prototype.format } catch (e) { r.push(e.name) } try { Intl.DateTimeFormat.prototype.formatToParts.call({}, 0) } catch (e) { r.push(e.name) } r.join()"), "TypeError,TypeError"),
];
