use super::*;
use jobs::{JobOutput, PublishedResult};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum TableSelector {
    Specified,
    Search { candidate_id: DecimalU64, evaluation_key: String },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "table", rename_all = "snake_case", deny_unknown_fields)]
pub enum PageKind { Slices(TableSelector), BishopIterations(TableSelector), SearchCandidates, ArchiveBytes }
#[derive(Clone, Serialize)]
pub(super) struct CursorState {
    pub sequence: DecimalU64,
    request_digest: String,
    content_digest: String,
    kind: PageKind,
    offset: DecimalU64,
}
#[derive(Serialize)]
#[serde(tag = "kind", content = "rows", rename_all = "snake_case")]
enum PageData<'a> {
    Slices(&'a [Slice]), BishopIterations(&'a [BishopIteration]),
    SearchCandidates(&'a [CandidateSummary]), ArchiveBytes(&'a [u8]),
}
impl PageData<'_> {
    fn len(&self) -> usize {
        match self { Self::Slices(v) => v.len(), Self::BishopIterations(v) => v.len(),
            Self::SearchCandidates(v) => v.len(), Self::ArchiveBytes(v) => v.len() }
    }
    fn range(&self, start: usize, end: usize) -> PageData<'_> {
        match self { Self::Slices(v) => PageData::Slices(&v[start..end]),
            Self::BishopIterations(v) => PageData::BishopIterations(&v[start..end]),
            Self::SearchCandidates(v) => PageData::SearchCandidates(&v[start..end]),
            Self::ArchiveBytes(v) => PageData::ArchiveBytes(&v[start..end]) }
    }
}
#[derive(Serialize)]
struct Page<'a> {
    format: &'static str,
    sequence: DecimalU64,
    request_digest: &'a str,
    content_digest: &'a str,
    offset: DecimalU64,
    total_rows_or_bytes: DecimalU64,
    next_cursor: Option<&'a str>,
    data: PageData<'a>,
}
pub struct EncodedPage {
    /// Complete bounded JSON page, never a clone of the whole result.
    pub utf8: Vec<u8>,
    pub next_cursor: Option<String>,
    pub rows: usize,
}
fn data<'a>(archive: &'a PublishedResult, kind: &PageKind) -> Result<PageData<'a>, HostError> {
    match kind {
        PageKind::ArchiveBytes => Ok(PageData::ArchiveBytes(archive.canonical_bytes())),
        PageKind::SearchCandidates => match archive.output() {
            JobOutput::Search(r) => Ok(PageData::SearchCandidates(&r.candidates)),
            _ => Err(HostError::InvalidPage),
        },
        PageKind::Slices(selector) | PageKind::BishopIterations(selector) => {
            let (slices, bishop) = match (archive.output(), selector) {
                (JobOutput::Specified(r), TableSelector::Specified) => (&r.slices, Some(&r.bishop_result)),
                (JobOutput::Search(r), TableSelector::Search { candidate_id, evaluation_key }) => {
                    if evaluation_key.len() > 128 { return Err(HostError::InvalidPage); }
                    let table = r.retained_tables.iter().find(|t| t.candidate_id == candidate_id.0
                        && t.evaluation_key == *evaluation_key).ok_or(HostError::InvalidPage)?;
                    (&table.evaluation.slices, table.evaluation.bishop.as_ref().ok())
                }
                _ => return Err(HostError::InvalidPage),
            };
            if matches!(kind, PageKind::Slices(_)) { Ok(PageData::Slices(slices)) }
            else { Ok(PageData::BishopIterations(&bishop.ok_or(HostError::InvalidPage)?.iterations)) }
        }
    }
}
impl CircularHostController {
    /// Dispatcher authorization remains external. Each page checks current epoch
    /// and stale state, and binds opaque cursor to exact sequence/request/content.
    pub fn result_page(&mut self, doc: &Document, ctx: &HostContext, sequence: DecimalU64,
        kind: PageKind, cursor: Option<&str>, rows: usize, bytes_cap: usize)
        -> Result<EncodedPage, HostError> {
        if rows == 0 || rows > self.limits.max_page_rows || bytes_cap < 1024
            || bytes_cap > self.limits.max_page_bytes { return Err(HostError::InvalidPage); }
        self.after_mutation(doc, ctx); self.access(doc, ctx, sequence.0)?;
        let archive = self.manager.result(sequence.0)?;
        let start = if let Some(token) = cursor {
            if token.len() != 71 { return Err(HostError::CursorGone); }
            let state = self.cursors.get(token).ok_or(HostError::CursorGone)?;
            if state.sequence != sequence || state.kind != kind || state.request_digest != archive.request_digest
                || state.content_digest != archive.content_digest { return Err(HostError::CursorMismatch); }
            usize::try_from(state.offset.0).map_err(|_| HostError::InvalidPage)?
        } else { 0 };
        let all = data(archive, &kind)?;
        if start > all.len() { return Err(HostError::InvalidPage); }
        let is_bytes = matches!(kind, PageKind::ArchiveBytes);
        let take = if is_bytes { bytes_cap.saturating_sub(1024) / 4 } else { rows };
        if take == 0 { return Err(HostError::PageBudget); }
        let end = start.saturating_add(take).min(all.len());
        let next = (end < all.len()).then(|| CursorState { sequence,
            request_digest: archive.request_digest.clone(), content_digest: archive.content_digest.clone(),
            kind: kind.clone(), offset: DecimalU64(end as u64) });
        let token = next.as_ref().map(|state| hash(&("circular-result-cursor-v1", state), 2048)).transpose()?;
        let page = Page { format: "circular-result-page-v1", sequence,
            request_digest: &archive.request_digest, content_digest: &archive.content_digest,
            offset: DecimalU64(start as u64), total_rows_or_bytes: DecimalU64(all.len() as u64),
            next_cursor: token.as_deref(), data: all.range(start, end) };
        let mut sink = BoundedBytes::new(bytes_cap);
        serde_json::to_writer(&mut sink, &page).map_err(|_| HostError::PageBudget)?;
        let count = if is_bytes { usize::from(end > start) } else { end - start };
        let utf8 = sink.bytes;
        // Register cursor only AFTER complete successful page serialization.
        if let (Some(token), Some(state)) = (&token, next) {
            if !self.cursors.contains_key(token) {
                while self.cursors.len() >= self.limits.max_cursors {
                    if let Some(old) = self.cursor_order.pop_front() { self.cursors.remove(&old); }
                }
                self.cursor_order.push_back(token.clone()); self.cursors.insert(token.clone(), state);
            }
        }
        Ok(EncodedPage { utf8, next_cursor: token, rows: count })
    }
}
