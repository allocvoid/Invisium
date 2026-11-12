//! en-US data for `Intl.NumberFormat`: currency symbols and names, unit names.

/// (code, symbol, narrow symbol, singular name, plural name) for the most used currencies.
const CURRENCIES: &[(&str, &str, &str, &str, &str)] = &[
    ("USD", "$", "$", "US dollar", "US dollars"),
    ("EUR", "€", "€", "euro", "euros"),
    ("JPY", "¥", "¥", "Japanese yen", "Japanese yen"),
    ("GBP", "£", "£", "British pound", "British pounds"),
    ("CNY", "CN¥", "¥", "Chinese yuan", "Chinese yuan"),
    ("AUD", "A$", "$", "Australian dollar", "Australian dollars"),
    ("CAD", "CA$", "$", "Canadian dollar", "Canadian dollars"),
    ("CHF", "CHF", "CHF", "Swiss franc", "Swiss francs"),
    ("HKD", "HK$", "$", "Hong Kong dollar", "Hong Kong dollars"),
    ("SGD", "SGD", "$", "Singapore dollar", "Singapore dollars"),
    ("SEK", "SEK", "kr", "Swedish krona", "Swedish kronor"),
    ("KRW", "₩", "₩", "South Korean won", "South Korean won"),
    ("NOK", "NOK", "kr", "Norwegian krone", "Norwegian kroner"),
    ("NZD", "NZ$", "$", "New Zealand dollar", "New Zealand dollars"),
    ("INR", "₹", "₹", "Indian rupee", "Indian rupees"),
    ("MXN", "MX$", "$", "Mexican peso", "Mexican pesos"),
    ("TWD", "NT$", "$", "New Taiwan dollar", "New Taiwan dollars"),
    ("ZAR", "ZAR", "R", "South African rand", "South African rand"),
    ("BRL", "R$", "R$", "Brazilian real", "Brazilian reals"),
    ("DKK", "DKK", "kr", "Danish krone", "Danish kroner"),
    ("PLN", "PLN", "zł", "Polish zloty", "Polish zlotys"),
    ("THB", "THB", "฿", "Thai baht", "Thai baht"),
    ("ILS", "₪", "₪", "Israeli new shekel", "Israeli new shekels"),
    ("IDR", "IDR", "Rp", "Indonesian rupiah", "Indonesian rupiahs"),
    ("CZK", "CZK", "Kč", "Czech koruna", "Czech korunas"),
    ("AED", "AED", "AED", "UAE dirham", "UAE dirhams"),
    ("TRY", "TRY", "₺", "Turkish lira", "Turkish Lira"),
    ("HUF", "HUF", "Ft", "Hungarian forint", "Hungarian forints"),
    ("CLP", "CLP", "$", "Chilean peso", "Chilean pesos"),
    ("SAR", "SAR", "SAR", "Saudi riyal", "Saudi riyals"),
    ("PHP", "₱", "₱", "Philippine peso", "Philippine pesos"),
    ("MYR", "MYR", "RM", "Malaysian ringgit", "Malaysian ringgits"),
    ("RUB", "RUB", "₽", "Russian ruble", "Russian rubles"),
    ("VND", "₫", "₫", "Vietnamese dong", "Vietnamese dong"),
];

/// ISO 4217 minor units of a currency.
pub(super) fn currency_digits(code: &str) -> u32 {
    match code {
        "JPY" | "KRW" | "CLP" | "VND" | "ISK" | "UGX" | "PYG" | "XAF" | "XOF" | "RWF" | "KMF" | "GNF" | "DJF" | "VUV" | "XPF" | "BIF" => 0,
        "BHD" | "KWD" | "OMR" | "JOD" | "IQD" | "LYD" | "TND" => 3,
        _ => 2,
    }
}

/// The text for a currency in `currencyDisplay` mode 0 symbol, 1 narrowSymbol, 2 code,
/// 3 name (`plural` picks the plural name).
pub(super) fn currency_text(code: &str, display: u8, plural: bool) -> String {
    let entry = CURRENCIES.iter().find(|c| c.0 == code);
    match (display, entry) {
        (0, Some(c)) => c.1.to_string(),
        (1, Some(c)) => c.2.to_string(),
        (3, Some(c)) => if plural { c.4 } else { c.3 }.to_string(),
        _ => code.to_string(),
    }
}

/// The name `Intl.DisplayNames` gives a currency.
pub(super) fn currency_name(code: &str) -> Option<&'static str> {
    CURRENCIES.iter().find(|c| c.0 == code).map(|c| c.3)
}

/// The sanctioned simple units: (identifier, short form, singular long, plural long).
/// A short form starting with `!` attaches without a space.
const UNITS: &[(&str, &str, &str, &str)] = &[
    ("acre", "ac", "acre", "acres"),
    ("bit", "bit", "bit", "bits"),
    ("byte", "byte", "byte", "bytes"),
    ("celsius", "!°C", "degree Celsius", "degrees Celsius"),
    ("centimeter", "cm", "centimeter", "centimeters"),
    ("day", "day|days", "day", "days"),
    ("degree", "deg", "degree", "degrees"),
    ("fahrenheit", "!°F", "degree Fahrenheit", "degrees Fahrenheit"),
    ("fluid-ounce", "fl oz", "fluid ounce", "fluid ounces"),
    ("foot", "ft", "foot", "feet"),
    ("gallon", "gal", "gallon", "gallons"),
    ("gigabit", "Gb", "gigabit", "gigabits"),
    ("gigabyte", "GB", "gigabyte", "gigabytes"),
    ("gram", "g", "gram", "grams"),
    ("hectare", "ha", "hectare", "hectares"),
    ("hour", "hr", "hour", "hours"),
    ("inch", "in", "inch", "inches"),
    ("kilobit", "kb", "kilobit", "kilobits"),
    ("kilobyte", "kB", "kilobyte", "kilobytes"),
    ("kilogram", "kg", "kilogram", "kilograms"),
    ("kilometer", "km", "kilometer", "kilometers"),
    ("liter", "L", "liter", "liters"),
    ("megabit", "Mb", "megabit", "megabits"),
    ("megabyte", "MB", "megabyte", "megabytes"),
    ("meter", "m", "meter", "meters"),
    ("microsecond", "μs", "microsecond", "microseconds"),
    ("mile", "mi", "mile", "miles"),
    ("mile-scandinavian", "smi", "mile-scandinavian", "miles-scandinavian"),
    ("milliliter", "mL", "milliliter", "milliliters"),
    ("millimeter", "mm", "millimeter", "millimeters"),
    ("millisecond", "ms", "millisecond", "milliseconds"),
    ("minute", "min", "minute", "minutes"),
    ("month", "mth|mths", "month", "months"),
    ("nanosecond", "ns", "nanosecond", "nanoseconds"),
    ("ounce", "oz", "ounce", "ounces"),
    ("percent", "!%", "percent", "percent"),
    ("petabyte", "PB", "petabyte", "petabytes"),
    ("pound", "lb", "pound", "pounds"),
    ("second", "sec", "second", "seconds"),
    ("stone", "st", "stone", "stones"),
    ("terabit", "Tb", "terabit", "terabits"),
    ("terabyte", "TB", "terabyte", "terabytes"),
    ("week", "wk|wks", "week", "weeks"),
    ("yard", "yd", "yard", "yards"),
    ("year", "yr|yrs", "year", "years"),
];

/// Short forms of the per-units that have their own symbol.
const PER_UNITS: &[(&str, &str, &str, &str)] = &[
    ("kilometer-per-hour", "km/h", "kilometer per hour", "kilometers per hour"),
    ("mile-per-hour", "mph", "mile per hour", "miles per hour"),
    ("meter-per-second", "m/s", "meter per second", "meters per second"),
    ("liter-per-kilometer", "L/km", "liter per kilometer", "liters per kilometer"),
    ("mile-per-gallon", "mpg", "mile per gallon", "miles per gallon"),
];

fn simple(id: &str) -> Option<&'static (&'static str, &'static str, &'static str, &'static str)> {
    UNITS.iter().find(|u| u.0 == id)
}

/// Whether `id` is a sanctioned unit or a `-per-` pair of them.
pub(super) fn unit_ok(id: &str) -> bool {
    if simple(id).is_some() {
        return true;
    }
    match id.split_once("-per-") {
        Some((a, b)) => simple(a).is_some() && simple(b).is_some(),
        None => false,
    }
}

/// How a unit is written after a number: (text, attach without a space). `display`: 0 short,
/// 1 narrow, 2 long.
pub(super) fn unit_text(id: &str, display: u8, plural: bool) -> (String, bool) {
    if let Some(p) = PER_UNITS.iter().find(|p| p.0 == id) {
        let long = if plural { p.3 } else { p.2 };
        return if display == 2 { (long.to_string(), false) } else { (p.1.to_string(), false) };
    }
    if let Some((a, b)) = id.split_once("-per-") {
        let (ta, _) = unit_text(a, display, plural);
        let tb = match simple(b) {
            Some(u) if display == 2 => u.2.to_string(),
            Some(u) if u.0 == "second" => "s".to_string(),
            Some(u) if u.0 == "hour" => "h".to_string(),
            Some(u) => short_of(u.1, false).0,
            None => b.to_string(),
        };
        return if display == 2 { (format!("{} per {}", ta, tb), false) } else { (format!("{}/{}", ta, tb), false) };
    }
    match simple(id) {
        Some(u) if display == 2 => (if plural { u.3 } else { u.2 }.to_string(), false),
        Some(u) => {
            let (text, attach) = short_of(u.1, plural);
            // narrow forms of the symbol units drop the space after the number
            let symbol = matches!(text.as_str(), "km" | "m" | "cm" | "mm" | "kg" | "g" | "L" | "mL" | "lb" | "oz" | "mi");
            (text, attach || (display == 1 && symbol))
        }
        None => (id.to_string(), false),
    }
}

fn short_of(spec: &str, plural: bool) -> (String, bool) {
    let (spec, attach) = match spec.strip_prefix('!') {
        Some(s) => (s, true),
        None => (spec, false),
    };
    let text = match spec.split_once('|') {
        Some((one, many)) => if plural { many } else { one },
        None => spec,
    };
    (text.to_string(), attach)
}
