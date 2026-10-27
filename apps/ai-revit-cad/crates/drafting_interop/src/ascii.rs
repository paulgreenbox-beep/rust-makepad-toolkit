//! Bounded lexical validation that must run before CADCraft's permissive parser.

use cadcraft_dxf::Kind;

pub(crate) const MAX_DXF_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_DXF_TAGS: usize = 200_000;
const MAX_LINE_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RawTag {
    pub code: i32,
    pub value: String,
    pub line: usize,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PreflightLimits {
    pub max_bytes: usize,
    pub max_tags: usize,
    pub max_line_bytes: usize,
}

impl Default for PreflightLimits {
    fn default() -> Self {
        Self {
            max_bytes: MAX_DXF_BYTES,
            max_tags: MAX_DXF_TAGS,
            max_line_bytes: MAX_LINE_BYTES,
        }
    }
}

/// Validate ASCII tag pairing and all typed numeric fields before CADCraft parses them.
/// CADCraft intentionally maps malformed numeric values to zero, so it cannot be
/// the first validation boundary for untrusted exchange files.
pub(crate) fn preflight_ascii(bytes: &[u8]) -> Result<(Vec<RawTag>, &[u8]), String> {
    preflight_ascii_with_limits(bytes, PreflightLimits::default())
}

fn preflight_ascii_with_limits<'a>(
    bytes: &'a [u8],
    limits: PreflightLimits,
) -> Result<(Vec<RawTag>, &'a [u8]), String> {
    if bytes.len() > limits.max_bytes {
        return Err(format!(
            "DXF exceeds the {} byte import budget",
            limits.max_bytes
        ));
    }
    if bytes.starts_with(b"AutoCAD Binary DXF\r\n\x1a\0") {
        return Err("binary DXF is outside the ASCII-only import boundary".into());
    }

    let content = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    if content.iter().any(|byte| {
        *byte > 0x7F || *byte == 0 || (*byte < 0x20 && !matches!(*byte, b'\r' | b'\n' | b'\t'))
    }) {
        return Err("DXF input must be printable 7-bit ASCII (UTF-8 BOM is allowed)".into());
    }
    let text = std::str::from_utf8(content).map_err(|_| "DXF input is not valid ASCII")?;
    let line_count = text.lines().count();
    if line_count == 0 || line_count % 2 != 0 {
        return Err("DXF must contain complete group-code/value line pairs".into());
    }
    let pair_count = line_count / 2;
    if pair_count > limits.max_tags {
        return Err(format!(
            "DXF exceeds the {} tag import budget",
            limits.max_tags
        ));
    }

    let mut raw = Vec::with_capacity(pair_count);
    let mut lines = text.lines();
    for pair in 0..pair_count {
        let code_line_index = pair * 2;
        let value_line_index = code_line_index + 1;
        let code_text = lines
            .next()
            .ok_or_else(|| "DXF group code line is missing".to_string())?
            .trim();
        let value = lines
            .next()
            .ok_or_else(|| "DXF group value line is missing".to_string())?;
        if code_text.is_empty() {
            return Err(format!("line {}: group code is empty", code_line_index + 1));
        }
        if code_text.len() > limits.max_line_bytes || value.len() > limits.max_line_bytes {
            return Err(format!(
                "line {}: DXF line exceeds the {} byte limit",
                code_line_index + 1,
                limits.max_line_bytes
            ));
        }
        let code = code_text
            .parse::<i32>()
            .map_err(|_| format!("line {}: invalid group code", code_line_index + 1))?;
        if code < 0 {
            return Err(format!(
                "line {}: negative DXF group codes are invalid",
                code_line_index + 1
            ));
        }
        validate_typed_value(code, value, value_line_index + 1)?;
        raw.push(RawTag {
            code,
            value: value.to_string(),
            line: code_line_index + 1,
        });
    }

    Ok((raw, content))
}

fn validate_typed_value(code: i32, value: &str, line: usize) -> Result<(), String> {
    let trimmed = value.trim();
    match cadcraft_dxf::kind_of(code) {
        Kind::Real => {
            let number = trimmed
                .parse::<f64>()
                .map_err(|_| format!("line {line}: malformed real value for group {code}"))?;
            if !number.is_finite() {
                return Err(format!(
                    "line {line}: non-finite real value for group {code}"
                ));
            }
        }
        Kind::Int16 => {
            trimmed.parse::<i16>().map_err(|_| {
                format!("line {line}: malformed or out-of-range integer value for group {code}")
            })?;
        }
        Kind::Int32 => {
            trimmed.parse::<i32>().map_err(|_| {
                format!("line {line}: malformed or out-of-range integer value for group {code}")
            })?;
        }
        Kind::Int64 => {
            trimmed
                .parse::<i64>()
                .map_err(|_| format!("line {line}: malformed integer value for group {code}"))?;
        }
        Kind::Bool => {
            if trimmed != "0" && trimmed != "1" {
                return Err(format!("line {line}: boolean group {code} must be 0 or 1"));
            }
        }
        Kind::Hex => {
            if trimmed.len() % 2 != 0 || !trimmed.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(format!(
                    "line {line}: malformed hexadecimal value for group {code}"
                ));
            }
        }
        Kind::Str => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ascii_pairs_and_optional_bom() {
        let bytes =
            b"\xEF\xBB\xBF  0\r\nSECTION\r\n  2\r\nHEADER\r\n  0\r\nENDSEC\r\n  0\r\nEOF\r\n";
        let (tags, content) =
            preflight_ascii(bytes).expect("valid ASCII DXF should pass preflight");
        assert_eq!(tags.len(), 4);
        assert_eq!(content, &bytes[3..]);
    }

    #[test]
    fn rejects_malformed_numbers_before_cadcraft_can_coerce_them() {
        for value in ["not-a-number", "NaN", "inf", "-Infinity"] {
            let input = format!("10\n{value}\n");
            assert!(
                preflight_ascii(input.as_bytes()).is_err(),
                "accepted {value}"
            );
        }
        assert!(preflight_ascii(b"90\n1.5\n").is_err());
        assert!(preflight_ascii(b"290\n2\n").is_err());
    }

    #[test]
    fn rejects_binary_and_non_ascii_input() {
        assert!(preflight_ascii(b"AutoCAD Binary DXF\r\n\x1a\0garbage").is_err());
        assert!(preflight_ascii(b"1\n\xE9\n").is_err());
    }

    #[test]
    fn enforces_byte_tag_and_line_budgets() {
        let limits = PreflightLimits {
            max_bytes: 64,
            max_tags: 1,
            max_line_bytes: 8,
        };
        assert!(preflight_ascii_with_limits(b"1\na\n2\nb\n", limits).is_err());
        let limits = PreflightLimits {
            max_bytes: 64,
            max_tags: 4,
            max_line_bytes: 2,
        };
        assert!(preflight_ascii_with_limits(b"1\nlong\n", limits).is_err());
        let limits = PreflightLimits {
            max_bytes: 4,
            max_tags: 4,
            max_line_bytes: 8,
        };
        assert!(preflight_ascii_with_limits(b"1\n12345\n", limits).is_err());
    }

    #[test]
    fn rejects_odd_pairs_and_bad_group_codes() {
        assert!(preflight_ascii(b"0\nSECTION\n2\n").is_err());
        assert!(preflight_ascii(b"oops\nVALUE\n").is_err());
        assert!(preflight_ascii(b"-1\nVALUE\n").is_err());
    }
}
