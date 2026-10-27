//! Allocation-free lexical budget check BEFORE serde allocates untrusted strings/Vecs.
//! Full JSON syntax/type validation remains serde's job. Only root markdown is large.
use super::{ReportError, ReportLimits};
pub(super) fn check(bytes: &[u8], limits: ReportLimits) -> Result<(), ReportError> {
    if bytes.len() > limits.archive_bytes { return Err(ReportError::ArchiveBudget); }
    let mut parser = Parser { bytes, at: 0, limits };
    parser.value(0, false)?;
    parser.space();
    if parser.at != bytes.len() { return Err(ReportError::InvalidArchive); }
    Ok(())
}
struct Parser<'a> { bytes: &'a [u8], at: usize, limits: ReportLimits }
// Semantic field names match serde JSON decoding, including escaped ASCII,
// UTF8 and surrogate pairs. No String/Vec allocation before archive admission.
fn key_equal(raw: &[u8], expected: &str) -> Result<bool, ReportError> {
    fn hex(raw: &[u8], at: &mut usize) -> Result<u32, ReportError> {
        let digits = raw.get(*at..at.saturating_add(4)).ok_or(ReportError::InvalidArchive)?;
        let mut value = 0;
        for b in digits { value = value * 16 + match b {
            b'0'..=b'9' => u32::from(b - b'0'), b'a'..=b'f' => u32::from(b - b'a' + 10),
            b'A'..=b'F' => u32::from(b - b'A' + 10), _ => return Err(ReportError::InvalidArchive),
        }; }
        *at += 4; Ok(value)
    }
    let mut at = 0; let mut expected = expected.chars(); let mut equal = true;
    while at < raw.len() {
        let first = raw[at]; at += 1;
        let scalar = if first == b'\\' {
            let escape = *raw.get(at).ok_or(ReportError::InvalidArchive)?; at += 1;
            match escape {
                b'"' => '"', b'\\' => '\\', b'/' => '/', b'b' => '\u{8}', b'f' => '\u{c}',
                b'n' => '\n', b'r' => '\r', b't' => '\t',
                b'u' => {
                    let first = hex(raw, &mut at)?;
                    let value = if (0xd800..=0xdbff).contains(&first) {
                        if raw.get(at..at.saturating_add(2)) != Some(b"\\u") { return Err(ReportError::InvalidArchive); }
                        at += 2; let low = hex(raw, &mut at)?;
                        if !(0xdc00..=0xdfff).contains(&low) { return Err(ReportError::InvalidArchive); }
                        0x10000 + ((first - 0xd800) << 10) + low - 0xdc00
                    } else { first };
                    char::from_u32(value).ok_or(ReportError::InvalidArchive)?
                }
                _ => return Err(ReportError::InvalidArchive),
            }
        } else {
            let length = if first < 0x80 { 1 } else if first < 0xe0 { 2 } else if first < 0xf0 { 3 } else { 4 };
            let start = at - 1; let end = start + length;
            let bytes = raw.get(start..end).ok_or(ReportError::InvalidArchive)?;
            at = end;
            std::str::from_utf8(bytes).map_err(|_| ReportError::InvalidArchive)?
                .chars().next().ok_or(ReportError::InvalidArchive)?
        };
        equal &= expected.next() == Some(scalar);
    }
    Ok(equal && expected.next().is_none())
}
fn root_key(raw: &[u8]) -> Result<Option<usize>, ReportError> {
    for (id, name) in ["format", "formatter", "input_digest_algorithm", "bundle", "markdown",
        "bundle_digest", "report_digest", "archive_digest"].into_iter().enumerate() {
        if key_equal(raw, name)? { return Ok(Some(id)); }
    }
    Ok(None)
}
impl Parser<'_> {
    fn space(&mut self) {
        while self.bytes.get(self.at).is_some_and(u8::is_ascii_whitespace) { self.at += 1; }
    }
    fn take(&mut self, byte: u8) -> Result<(), ReportError> {
        self.space();
        if self.bytes.get(self.at) != Some(&byte) { return Err(ReportError::InvalidArchive); }
        self.at += 1; Ok(())
    }
    fn string(&mut self, cap: usize) -> Result<&[u8], ReportError> {
        self.take(b'"')?;
        let start = self.at;
        let mut decoded_upper = 0usize;
        loop {
            let b = *self.bytes.get(self.at).ok_or(ReportError::InvalidArchive)?;
            self.at += 1;
            match b {
                b'"' => return Ok(&self.bytes[start..self.at - 1]),
                b'\\' => {
                    let escape = *self.bytes.get(self.at).ok_or(ReportError::InvalidArchive)?;
                    self.at += 1;
                    if escape == b'u' {
                        let end = self.at.checked_add(4).ok_or(ReportError::InputBudget)?;
                        let digits = self.bytes.get(self.at..end).ok_or(ReportError::InvalidArchive)?;
                        if !digits.iter().all(u8::is_ascii_hexdigit) { return Err(ReportError::InvalidArchive); }
                        self.at = end;
                        // At most 3 UTF8 bytes per \uXXXX, including a safe
                        // overestimate for each half of a surrogate pair.
                        decoded_upper = decoded_upper.saturating_add(3);
                    } else if b"\"\\/bfnrt".contains(&escape) {
                        decoded_upper = decoded_upper.saturating_add(1);
                    } else { return Err(ReportError::InvalidArchive); }
                }
                0..=31 => return Err(ReportError::InvalidArchive),
                _ => decoded_upper = decoded_upper.saturating_add(1),
            }
            if decoded_upper > cap { return Err(ReportError::InputBudget); }
        }
    }
    fn value(&mut self, depth: usize, large_text: bool) -> Result<(), ReportError> {
        if depth > self.limits.nesting { return Err(ReportError::InputBudget); }
        self.space();
        match self.bytes.get(self.at) {
            Some(b'"') => {
                let cap = if large_text { self.limits.output_bytes }
                    else { self.limits.string_bytes };
                self.string(cap)?; Ok(())
            }
            Some(b'{') => {
                self.at += 1; self.space();
                if self.bytes.get(self.at) == Some(&b'}') { self.at += 1; return Ok(()); }
                let mut items = 0;
                let mut seen_root_keys = 0u16;
                loop {
                    items += 1;
                    if items > self.limits.collection_items { return Err(ReportError::InputBudget); }
                    let root = depth == 0;
                    let key = self.string(self.limits.string_bytes)?;
                    let known = if root { root_key(key)? } else { None };
                    if let Some(id) = known {
                        let bit = 1u16 << id;
                        if seen_root_keys & bit != 0 { return Err(ReportError::InvalidArchive); }
                        seen_root_keys |= bit;
                    }
                    let key_is_markdown = known == Some(4);
                    let key_is_bundle = known == Some(3);
                    self.take(b':')?;
                    let start = self.at;
                    self.value(depth + 1, root && key_is_markdown)?;
                    if root && key_is_bundle && self.at - start > self.limits.input_bytes {
                        return Err(ReportError::InputBudget);
                    }
                    self.space();
                    match self.bytes.get(self.at) {
                        Some(b'}') => { self.at += 1; return Ok(()); }
                        Some(b',') => self.at += 1,
                        _ => return Err(ReportError::InvalidArchive),
                    }
                }
            }
            Some(b'[') => {
                self.at += 1; self.space();
                if self.bytes.get(self.at) == Some(&b']') { self.at += 1; return Ok(()); }
                let mut items = 0;
                loop {
                    items += 1;
                    if items > self.limits.collection_items { return Err(ReportError::InputBudget); }
                    self.value(depth + 1, false)?;
                    self.space();
                    match self.bytes.get(self.at) {
                        Some(b']') => { self.at += 1; return Ok(()); }
                        Some(b',') => self.at += 1,
                        _ => return Err(ReportError::InvalidArchive),
                    }
                }
            }
            Some(_) => {
                let start = self.at;
                while self.bytes.get(self.at).is_some_and(|b|
                    !b.is_ascii_whitespace() && !b",]}".contains(b)) {
                    self.at += 1;
                    if self.at - start > 128 { return Err(ReportError::InvalidArchive); }
                }
                if self.at == start { Err(ReportError::InvalidArchive) } else { Ok(()) }
            }
            None => Err(ReportError::InvalidArchive),
        }
    }
}
