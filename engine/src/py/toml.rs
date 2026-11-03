//! TOML as tomllib reads it into Python values, for the manifests the new-dependencies check reads
//! (crate::checks::dependencies): its errors, and what Python's str(), repr() and json.dumps() write of each value.

use ::toml::value::{Datetime, Offset};
use ::toml::{Table, Value};

/// The table text holds, as tomllib.loads reads it; for text that is not TOML, why not, as [`syntax_error`] words
/// it. The toml crate reads TOML 1.1, some of which tomllib before Python 3.15 refuses.
pub fn loads(text: &str) -> Result<Table, String> {
    text.parse::<Table>().map_err(|e| syntax_error(text, &e))
}

/// Why text is not TOML, as tomllib words it: the parser's message and `(at line L, column C)`, where it found the
/// problem, the line and the column counted from 1, the column in characters.
pub fn syntax_error(text: &str, e: &::toml::de::Error) -> String {
    let message = e.message().trim_end();
    match e.span() {
        Some(span) => {
            let before = &text[..span.start.min(text.len())];
            let line = before.matches('\n').count() + 1;
            let column = before.rsplit('\n').next().unwrap_or(before).chars().count() + 1;
            format!("{message} (at line {line}, column {column})")
        }
        None => message.to_string(),
    }
}

/// The name of the Python type tomllib reads value as.
pub fn python_type(value: &Value) -> &'static str {
    match value {
        Value::String(_) => "str",
        Value::Integer(_) => "int",
        Value::Float(_) => "float",
        Value::Boolean(_) => "bool",
        Value::Datetime(d) => match (d.date, d.time) {
            (Some(_), Some(_)) => "datetime",
            (Some(_), None) => "date",
            _ => "time",
        },
        Value::Array(_) => "list",
        Value::Table(_) => "dict",
    }
}

/// Whether Python takes what tomllib reads value as for true: not 0, 0.0, false, nor an empty string, list or table.
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::String(s) => !s.is_empty(),
        Value::Integer(n) => *n != 0,
        Value::Float(x) => *x != 0.0,
        Value::Boolean(b) => *b,
        Value::Datetime(_) => true,
        Value::Array(items) => !items.is_empty(),
        Value::Table(table) => !table.is_empty(),
    }
}

/// Nothing when what tomllib reads value as can be a dict's key; Python's TypeError's message when it cannot: a list
/// or a table.
pub fn hashable(value: &Value) -> Result<(), String> {
    match value {
        Value::Array(_) | Value::Table(_) => {
            let name = python_type(value);
            Err(format!("cannot use '{name}' as a dict key (unhashable type: '{name}')"))
        }
        _ => Ok(()),
    }
}

/// Whether what tomllib reads a and b as are the same dict key, as Python's == compares them: numbers by their exact
/// value, a boolean being 0 or 1; datetimes with an offset by the instant they name, and other datetimes, dates and
/// times field by field, never one of another kind; anything else only one of its own kind and value.
pub fn same_key(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Float(x), Value::Float(y)) => x == y,
        (Value::Float(x), other) | (other, Value::Float(x)) => integer(other).is_some_and(|n| float_is(*x, n)),
        (Value::Datetime(x), Value::Datetime(y)) => match (instant(x), instant(y)) {
            (Some(x), Some(y)) => x == y,
            _ => fields(x) == fields(y),
        },
        _ => match (integer(a), integer(b)) {
            (Some(m), Some(n)) => m == n,
            _ => a == b,
        },
    }
}

/// The int Python reads value as: an integer's, or a boolean's 0 or 1; None for anything else.
fn integer(value: &Value) -> Option<i64> {
    match value {
        Value::Integer(n) => Some(*n),
        Value::Boolean(b) => Some(i64::from(*b)),
        _ => None,
    }
}

/// Whether the float x is exactly the integer n, as Python compares a float with an int.
fn float_is(x: f64, n: i64) -> bool {
    x.fract() == 0.0 && x >= -(2f64.powi(63)) && x < 2f64.powi(63) && x as i64 == n
}

/// A datetime's fields as Python compares them, a missing second or fraction being 0, and the fraction cut to the
/// microsecond, as tomllib keeps it.
fn fields(d: &Datetime) -> impl PartialEq {
    let time = d.time.map(|t| (t.hour, t.minute, t.second.unwrap_or(0), t.nanosecond.unwrap_or(0) / 1000));
    (d.date, time, d.offset)
}

/// The instant an offset datetime names, as (seconds since 1970-01-01 in UTC, microseconds: the fraction cut as
/// tomllib cuts it); None for one without an offset, a date or a time.
fn instant(d: &Datetime) -> Option<(i64, u32)> {
    let (date, time, offset) = (d.date?, d.time?, d.offset?);
    let minutes = match offset {
        Offset::Z => 0,
        Offset::Custom { minutes } => i64::from(minutes),
    };
    let civil = jiff::civil::Date::new(i16::try_from(date.year).ok()?, date.month as i8, date.day as i8).ok()?;
    let days = i64::from(civil.since(jiff::civil::date(1970, 1, 1)).ok()?.get_days());
    let clock = i64::from(time.hour) * 3600 + i64::from(time.minute) * 60 + i64::from(time.second.unwrap_or(0));
    Some((days * 86_400 + clock - minutes * 60, time.nanosecond.unwrap_or(0) / 1000))
}

/// value as Python's str() writes it: a string as it is, a datetime, date or time in ISO 8601 with a blank between
/// the date and the time (`1979-05-27 07:32:00+00:00`), anything else as repr.
pub fn str(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Datetime(d) => datetime_str(d),
        other => repr(other),
    }
}

/// A TOML datetime as Python's str writes what tomllib reads it as.
fn datetime_str(d: &Datetime) -> String {
    let date = d.date.map(|date| format!("{:04}-{:02}-{:02}", date.year, date.month, date.day));
    let clock = d.time.map(|t| {
        let micro = t.nanosecond.unwrap_or(0) / 1000;
        let fraction = if micro == 0 { String::new() } else { format!(".{micro:06}") };
        format!("{:02}:{:02}:{:02}{fraction}", t.hour, t.minute, t.second.unwrap_or(0))
    });
    let zone = match d.offset {
        None => String::new(),
        Some(Offset::Z) => "+00:00".to_string(),
        Some(Offset::Custom { minutes }) => {
            let sign = if minutes < 0 { '-' } else { '+' };
            format!("{sign}{:02}:{:02}", minutes.unsigned_abs() / 60, minutes.unsigned_abs() % 60)
        }
    };
    match (date, clock) {
        (Some(date), Some(clock)) => format!("{date} {clock}{zone}"),
        (Some(date), None) => date,
        (None, clock) => clock.unwrap_or_default(),
    }
}

/// value as Python's repr() writes it, but for its strings, which are quoted as Rust's Debug quotes a str: `"text"`,
/// `5`, `2.5`, `True`, `[1, "a"]`, `{"k": 1}`, or a datetime's `datetime.date(2020, 1, 31)`.
pub fn repr(value: &Value) -> String {
    match value {
        Value::String(s) => format!("{s:?}"),
        Value::Integer(n) => n.to_string(),
        Value::Float(x) => float_text(*x).replace("Infinity", "inf").replace("NaN", "nan"),
        Value::Boolean(b) => if *b { "True" } else { "False" }.to_string(),
        Value::Datetime(d) => datetime_repr(d),
        Value::Array(items) => format!("[{}]", items.iter().map(repr).collect::<Vec<_>>().join(", ")),
        Value::Table(table) => {
            format!(
                "{{{}}}",
                table.iter().map(|(k, v)| format!("{k:?}: {}", repr(v))).collect::<Vec<_>>().join(", ")
            )
        }
    }
}

/// value as json.dumps writes it: ", " and ": " between items and keys, every character outside printable ASCII
/// escaped, and a float as Python's repr writes it; for a datetime, date or time anywhere in it, json.dumps's
/// TypeError's message.
pub fn json_dumps(value: &Value) -> Result<String, String> {
    let mut out = String::new();
    write_json(value, &mut out)?;
    Ok(out)
}

/// Write value at out's end as json_dumps writes it; errors as its.
fn write_json(value: &Value, out: &mut String) -> Result<(), String> {
    match value {
        Value::String(s) => write_ascii_string(s, out),
        Value::Integer(n) => out.push_str(&n.to_string()),
        Value::Float(x) => out.push_str(&float_text(*x)),
        Value::Boolean(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Datetime(_) => return Err(format!("Object of type {} is not JSON serializable", python_type(value))),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_json(item, out)?;
            }
            out.push(']');
        }
        Value::Table(table) => {
            out.push('{');
            for (i, (k, v)) in table.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_ascii_string(k, out);
                out.push_str(": ");
                write_json(v, out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

/// s as a JSON string with json.dumps's ensure_ascii: `"` and `\` escaped, "\n", "\r", "\t", "\b" and "\f" by
/// their letters, every other character outside space to `~` as `\u` and four hex digits, a surrogate pair beyond
/// U+FFFF.
fn write_ascii_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            _ => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
    }
    out.push('"');
}

/// x as Python's float.__repr__ writes it, which json.dumps uses: the shortest digits that read back as x, in
/// positional notation for exponents from -4 to 15 with at least one digit after the point, else in scientific
/// notation with a sign and at least two digits in the exponent; NaN and Infinity as json.dumps writes them.
fn float_text(x: f64) -> String {
    if x.is_nan() {
        return "NaN".to_string();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    // Rust's {:e} gives the shortest round-trip digits, as d.ddde<exp>.
    let sci = format!("{x:e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let sign = if mantissa.starts_with('-') { "-" } else { "" };
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let body = if (-4..16).contains(&exp) { positional(&digits, exp + 1) } else { scientific(&digits, exp) };
    format!("{sign}{body}")
}

/// The number whose significant digits are digits, without sign, with the decimal point after point of them, in
/// positional notation with at least one digit after the point: 0.0012 for "12" at -2, 1200.0 for "12" at 4.
fn positional(digits: &str, point: i32) -> String {
    if point <= 0 {
        format!("0.{}{}", "0".repeat(point.unsigned_abs() as usize), digits)
    } else if point as usize >= digits.len() {
        format!("{}{}.0", digits, "0".repeat(point as usize - digits.len()))
    } else {
        format!("{}.{}", &digits[..point as usize], &digits[point as usize..])
    }
}

/// The number whose significant digits are digits, without sign, times ten to exp, in scientific notation with a
/// sign and at least two digits in the exponent: 1.25e+16 for "125" at 16, 5e-05 for "5" at -5.
fn scientific(digits: &str, exp: i32) -> String {
    let rest = if digits.len() > 1 { format!(".{}", &digits[1..]) } else { String::new() };
    let exp_sign = if exp < 0 { '-' } else { '+' };
    format!("{}{rest}e{exp_sign}{:02}", &digits[..1], exp.unsigned_abs())
}

/// A TOML datetime as Python's repr writes what tomllib reads it as: datetime.datetime, datetime.date or
/// datetime.time, a second and a microsecond only when not 0, and its offset as a datetime.timezone.
fn datetime_repr(d: &Datetime) -> String {
    let clock = d.time.map(|t| {
        let (second, micro) = (t.second.unwrap_or(0), t.nanosecond.unwrap_or(0) / 1000);
        let mut fields = vec![t.hour.to_string(), t.minute.to_string()];
        if second != 0 || micro != 0 {
            fields.push(second.to_string());
        }
        if micro != 0 {
            fields.push(micro.to_string());
        }
        fields.join(", ")
    });
    let date = d.date.map(|date| format!("{}, {}, {}", date.year, date.month, date.day));
    match (date, clock) {
        (Some(date), Some(clock)) => {
            let zone = match d.offset {
                None => String::new(),
                Some(Offset::Z) | Some(Offset::Custom { minutes: 0 }) => ", tzinfo=datetime.timezone.utc".to_string(),
                Some(Offset::Custom { minutes }) => {
                    let seconds = i64::from(minutes) * 60;
                    let delta = if seconds < 0 {
                        format!("days=-1, seconds={}", 86_400 + seconds)
                    } else {
                        format!("seconds={seconds}")
                    };
                    format!(", tzinfo=datetime.timezone(datetime.timedelta({delta}))")
                }
            };
            format!("datetime.datetime({date}, {clock}{zone})")
        }
        (Some(date), None) => format!("datetime.date({date})"),
        (None, clock) => format!("datetime.time({})", clock.unwrap_or_default()),
    }
}

#[cfg(test)]
mod tests;
