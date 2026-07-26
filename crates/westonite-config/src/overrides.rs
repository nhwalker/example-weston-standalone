//! `-o key.path=value` dotted overrides (D10): parse into the TOML
//! value tree *before* deserialization so overrides get exactly the
//! same typing, unknown-key, and validation treatment as the file.

use toml::Value;

/// Apply one `section.key=value` (or deeper `a.b.c=value`) override.
/// Values parse as TOML when they can (numbers, booleans, arrays) and
/// fall back to strings, so `-o core.xwayland=true` and
/// `-o shell.background-color=0xff336699` both do what they look like.
/// Array-of-table paths accept an index: `-o output.0.mode=off`.
pub(crate) fn apply(root: &mut toml::Table, spec: &str) -> Result<(), String> {
    let Some((path, raw_value)) = spec.split_once('=') else {
        return Err(format!("override '{spec}': expected KEY.PATH=VALUE"));
    };
    let segments: Vec<&str> = path.split('.').collect();
    if segments.is_empty() || segments.iter().any(|s| s.is_empty()) {
        return Err(format!("override '{spec}': empty key path segment"));
    }

    let value = parse_value(raw_value);

    // First segment always names a table entry at the root.
    let mut current: &mut Value = root
        .entry(segments[0].to_string())
        .or_insert_with(|| Value::Table(toml::Table::new()));

    for (i, seg) in segments.iter().enumerate().skip(1) {
        let last = i == segments.len() - 1;
        if let Ok(index) = seg.parse::<usize>() {
            let arr = match current {
                Value::Array(a) => a,
                other => {
                    *other = Value::Array(Vec::new());
                    match other {
                        Value::Array(a) => a,
                        _ => unreachable!(),
                    }
                }
            };
            while arr.len() <= index {
                arr.push(Value::Table(toml::Table::new()));
            }
            if last {
                arr[index] = value;
                return Ok(());
            }
            current = &mut arr[index];
        } else {
            let table = match current {
                Value::Table(t) => t,
                other => {
                    *other = Value::Table(toml::Table::new());
                    match other {
                        Value::Table(t) => t,
                        _ => unreachable!(),
                    }
                }
            };
            if last {
                table.insert((*seg).to_string(), value);
                return Ok(());
            }
            current = table
                .entry((*seg).to_string())
                .or_insert_with(|| Value::Table(toml::Table::new()));
        }
    }

    // Single-segment path: `-o core=...` — only meaningful when the
    // value is a table; reject to keep overrides key-shaped.
    Err(format!(
        "override '{spec}': path must have at least two segments"
    ))
}

fn parse_value(raw: &str) -> Value {
    // Try full TOML value syntax first (numbers, bools, arrays,
    // quoted strings), fall back to a plain string.
    match format!("v = {raw}").parse::<toml::Table>() {
        Ok(mut t) => t
            .remove("v")
            .unwrap_or_else(|| Value::String(raw.to_string())),
        Err(_) => Value::String(raw.to_string()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn scalar_and_typed_values() {
        let mut root = toml::Table::new();
        apply(&mut root, "core.xwayland=true").unwrap();
        apply(&mut root, "core.backend=headless").unwrap();
        apply(&mut root, "keyboard.repeat-rate=40").unwrap();
        assert_eq!(root["core"]["xwayland"], Value::Boolean(true));
        assert_eq!(root["core"]["backend"], Value::String("headless".into()));
        assert_eq!(root["keyboard"]["repeat-rate"], Value::Integer(40));
    }

    #[test]
    fn array_of_tables_index() {
        let mut root = toml::Table::new();
        apply(&mut root, "output.0.mode=off").unwrap();
        apply(&mut root, "output.1.name=HDMI-A-1").unwrap();
        let outs = root["output"].as_array().unwrap();
        assert_eq!(outs[0]["mode"], Value::String("off".into()));
        assert_eq!(outs[1]["name"], Value::String("HDMI-A-1".into()));
    }

    #[test]
    fn malformed_specs_are_errors() {
        let mut root = toml::Table::new();
        assert!(apply(&mut root, "no-equals").is_err());
        assert!(apply(&mut root, "onlyroot=1").is_err());
        assert!(apply(&mut root, "a..b=1").is_err());
    }
}
