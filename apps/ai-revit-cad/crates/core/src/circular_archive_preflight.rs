//! Allocation-bounded scan of the native JSON document for typed circular
//! snapshot authorities. This runs before the normal `Document` decode so an
//! archive cannot make serde allocate an unbounded `ReportArchive` first.
//!
//! Only the known persistence paths are traversed. Every other value is
//! consumed as `IgnoredAny`; arbitrary user maps are never recursively walked
//! looking for names that happen to resemble an authority path.
//!
//! Byte accounting is deliberately conservative: it measures the source JSON
//! span of each non-null snapshot and each `payload_json` value, including
//! snapshot-internal whitespace and the payload string's quotes/escapes. This
//! is a raw encoded span quota, so whitespace-heavy archives may be rejected
//! before their compact typed representation reaches the same limit.

use crate::CoreError;
use crate::circular_archive::MAX_CIRCULAR_SNAPSHOT_BYTES;
use serde::Deserialize;
use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use std::cell::RefCell;
use std::fmt;
use std::io::{self, BufReader, Read};
use std::rc::Rc;

const MAX_CIRCULAR_RETAINED_BYTES: usize = 64 << 20;
const SNAPSHOT_BYTES_SCOPE: &str = "circular snapshot bytes";
const RETAINED_BYTES_SCOPE: &str = "retained circular archive bytes";

/// Result of the allocation-bounded native JSON probe.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct NativeArchivePreflight {
    /// Number of non-null typed snapshot values at the authority paths.
    pub(crate) snapshot_occurrences: usize,
    /// Sum of raw encoded snapshot spans, including whitespace inside values.
    pub(crate) snapshot_bytes: usize,
    /// Sum of raw encoded `payload_json` value spans seen in stored receipts.
    /// This is zero when the document contains no typed circular snapshot.
    pub(crate) request_json_bytes: usize,
    /// Number of source bytes consumed by the successful JSON parse.
    pub(crate) source_bytes: usize,
}

/// Probe a UTF-8 native document without materializing archive payloads.
pub(crate) fn preflight_circular_archive_native(
    source: &str,
) -> Result<NativeArchivePreflight, CoreError> {
    preflight_circular_archive_reader(source.as_bytes())
}

/// Probe a native JSON reader without materializing archive payloads.
pub(crate) fn preflight_circular_archive_reader<R: Read>(
    reader: R,
) -> Result<NativeArchivePreflight, CoreError> {
    preflight_reader_as(reader, Node::Root)
}

/// Probe the serialized `Vec<Command>` stored inside one receipt's
/// `payload_json` string before its typed command vector is decoded.
pub(crate) fn preflight_circular_command_payload(
    source: &str,
) -> Result<NativeArchivePreflight, CoreError> {
    preflight_reader_as(source.as_bytes(), Node::CommandBatch)
}

fn preflight_reader_as<R: Read>(
    reader: R,
    root: Node,
) -> Result<NativeArchivePreflight, CoreError> {
    let accounting = Rc::new(RefCell::new(ReadAccounting::default()));
    let reader = CountingReader {
        inner: BufReader::new(reader),
        accounting: Rc::clone(&accounting),
    };
    let mut deserializer = serde_json::Deserializer::from_reader(reader);
    let mut context = ProbeContext {
        accounting: Rc::clone(&accounting),
        result: NativeArchivePreflight::default(),
    };

    let parsed = NodeSeed {
        node: root,
        context: &mut context,
    }
    .deserialize(&mut deserializer)
    .and_then(|()| deserializer.end());

    if let Some(hit) = accounting.borrow_mut().limit_hit.take() {
        return Err(CoreError::CircularArchiveLimit {
            scope: hit.scope,
            limit: hit.limit,
        });
    }
    parsed?;

    let bytes = accounting.borrow().bytes;
    context.result.source_bytes = bytes;
    if context.result.snapshot_occurrences == 0 {
        // Old native documents retain their historical behavior even when
        // their unrelated receipt strings are large.
        context.result.request_json_bytes = 0;
        return Ok(context.result);
    }

    let retained = context
        .result
        .snapshot_bytes
        .checked_add(context.result.request_json_bytes)
        .ok_or(CoreError::CircularArchiveLimit {
            scope: RETAINED_BYTES_SCOPE,
            limit: MAX_CIRCULAR_RETAINED_BYTES,
        })?;
    if retained > MAX_CIRCULAR_RETAINED_BYTES {
        return Err(CoreError::CircularArchiveLimit {
            scope: RETAINED_BYTES_SCOPE,
            limit: MAX_CIRCULAR_RETAINED_BYTES,
        });
    }
    Ok(context.result)
}

#[derive(Clone, Copy)]
struct ActiveSpan {
    start: usize,
    limit: Option<usize>,
    scope: &'static str,
}

#[derive(Clone, Copy)]
struct LimitHit {
    scope: &'static str,
    limit: usize,
}

#[derive(Default)]
struct ReadAccounting {
    bytes: usize,
    active: Option<ActiveSpan>,
    limit_hit: Option<LimitHit>,
}

struct CountingReader<R> {
    inner: R,
    accounting: Rc<RefCell<ReadAccounting>>,
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = self.inner.read(buffer)?;
        if count == 0 {
            return Ok(0);
        }

        let mut accounting = self.accounting.borrow_mut();
        accounting.bytes = accounting.bytes.saturating_add(count);
        if let Some(active) = accounting.active {
            if active
                .limit
                .is_some_and(|limit| accounting.bytes.saturating_sub(active.start) > limit)
            {
                accounting.limit_hit = Some(LimitHit {
                    scope: active.scope,
                    limit: active.limit.expect("limit was checked as present"),
                });
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    "native circular archive preflight limit exceeded",
                ));
            }
        }
        Ok(count)
    }
}

struct ProbeContext {
    accounting: Rc<RefCell<ReadAccounting>>,
    result: NativeArchivePreflight,
}

impl ProbeContext {
    /// `deserialize_option` has peeked the first non-whitespace value byte by
    /// the time `visit_some` is entered. Subtract that byte so it is charged
    /// and constrained along with the remainder of the span.
    fn begin_span(&mut self, scope: &'static str, limit: Option<usize>) -> Result<usize, LimitHit> {
        let mut accounting = self.accounting.borrow_mut();
        let start = accounting.bytes.saturating_sub(1);
        let already_seen = accounting.bytes.saturating_sub(start);
        if let Some(limit) = limit {
            if already_seen > limit {
                let hit = LimitHit { scope, limit };
                accounting.limit_hit = Some(hit);
                return Err(hit);
            }
        }
        accounting.active = Some(ActiveSpan {
            start,
            limit,
            scope,
        });
        Ok(start)
    }

    fn end_span(&mut self, start: usize) -> Result<usize, LimitHit> {
        let mut accounting = self.accounting.borrow_mut();
        accounting.active = None;
        if let Some(hit) = accounting.limit_hit {
            return Err(hit);
        }
        Ok(accounting.bytes.saturating_sub(start))
    }

    fn snapshot_span_limit(&self) -> Result<(usize, &'static str), LimitHit> {
        let retained = self
            .result
            .snapshot_bytes
            .checked_add(self.result.request_json_bytes)
            .ok_or(LimitHit {
                scope: RETAINED_BYTES_SCOPE,
                limit: MAX_CIRCULAR_RETAINED_BYTES,
            })?;
        let remaining = MAX_CIRCULAR_RETAINED_BYTES
            .checked_sub(retained)
            .ok_or(LimitHit {
                scope: RETAINED_BYTES_SCOPE,
                limit: MAX_CIRCULAR_RETAINED_BYTES,
            })?;
        if remaining < MAX_CIRCULAR_SNAPSHOT_BYTES {
            Ok((remaining, RETAINED_BYTES_SCOPE))
        } else {
            Ok((MAX_CIRCULAR_SNAPSHOT_BYTES, SNAPSHOT_BYTES_SCOPE))
        }
    }

    fn request_span_limit(&self) -> Result<Option<usize>, LimitHit> {
        if self.result.snapshot_occurrences == 0 {
            return Ok(None);
        }
        let retained = self
            .result
            .snapshot_bytes
            .checked_add(self.result.request_json_bytes)
            .ok_or(LimitHit {
                scope: RETAINED_BYTES_SCOPE,
                limit: MAX_CIRCULAR_RETAINED_BYTES,
            })?;
        Ok(Some(
            MAX_CIRCULAR_RETAINED_BYTES
                .checked_sub(retained)
                .ok_or(LimitHit {
                    scope: RETAINED_BYTES_SCOPE,
                    limit: MAX_CIRCULAR_RETAINED_BYTES,
                })?,
        ))
    }

    fn record_snapshot(&mut self, bytes: usize) -> Result<(), LimitHit> {
        let occurrences = self
            .result
            .snapshot_occurrences
            .checked_add(1)
            .ok_or(LimitHit {
                scope: RETAINED_BYTES_SCOPE,
                limit: MAX_CIRCULAR_RETAINED_BYTES,
            })?;
        let total = self
            .result
            .snapshot_bytes
            .checked_add(bytes)
            .ok_or(LimitHit {
                scope: RETAINED_BYTES_SCOPE,
                limit: MAX_CIRCULAR_RETAINED_BYTES,
            })?;
        let retained = total
            .checked_add(self.result.request_json_bytes)
            .ok_or(LimitHit {
                scope: RETAINED_BYTES_SCOPE,
                limit: MAX_CIRCULAR_RETAINED_BYTES,
            })?;
        if retained > MAX_CIRCULAR_RETAINED_BYTES {
            return Err(LimitHit {
                scope: RETAINED_BYTES_SCOPE,
                limit: MAX_CIRCULAR_RETAINED_BYTES,
            });
        }
        self.result.snapshot_occurrences = occurrences;
        self.result.snapshot_bytes = total;
        Ok(())
    }

    fn record_request_json(&mut self, bytes: usize) -> Result<(), LimitHit> {
        let total = self
            .result
            .request_json_bytes
            .checked_add(bytes)
            .ok_or(LimitHit {
                scope: RETAINED_BYTES_SCOPE,
                limit: MAX_CIRCULAR_RETAINED_BYTES,
            })?;
        if self.result.snapshot_occurrences != 0 {
            let retained = total
                .checked_add(self.result.snapshot_bytes)
                .ok_or(LimitHit {
                    scope: RETAINED_BYTES_SCOPE,
                    limit: MAX_CIRCULAR_RETAINED_BYTES,
                })?;
            if retained > MAX_CIRCULAR_RETAINED_BYTES {
                return Err(LimitHit {
                    scope: RETAINED_BYTES_SCOPE,
                    limit: MAX_CIRCULAR_RETAINED_BYTES,
                });
            }
        }
        self.result.request_json_bytes = total;
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Node {
    Root,
    CommandBatch,
    Document,
    Engineering,
    CircularCases,
    CircularCase,
    Operations,
    Operation,
    OperationKind,
    EngineeringChanged,
    EngineeringCommand,
    SaveCircularSnapshot,
    Receipts,
    StoredCommand,
    RecordedCommands,
    Command,
    HistoryStack,
    HistoryEntry,
    Changes,
    Change,
    CircularCaseChange,
    OperationChange,
    RevisionStates,
    RevisionState,
    RevisionDelta,
    HistoryStackAnchor,
}

struct NodeSeed<'a> {
    node: Node,
    context: &'a mut ProbeContext,
}

impl<'de> DeserializeSeed<'de> for NodeSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        deserializer.deserialize_any(NodeVisitor {
            node: self.node,
            context: self.context,
        })
    }
}

struct NodeVisitor<'a> {
    node: Node,
    context: &'a mut ProbeContext,
}

impl NodeVisitor<'_> {
    fn visit_sequence<'de, A>(
        &mut self,
        mut sequence: A,
        child: Option<Node>,
    ) -> Result<(), A::Error>
    where
        A: SeqAccess<'de>,
    {
        if let Some(child) = child {
            while sequence
                .next_element_seed(NodeSeed {
                    node: child,
                    context: &mut *self.context,
                })?
                .is_some()
            {}
        } else {
            while sequence.next_element::<IgnoredAny>()?.is_some() {}
        }
        Ok(())
    }
}

impl<'de> Visitor<'de> for NodeVisitor<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a native document value")
    }

    fn visit_map<A>(self, mut map: A) -> Result<(), A::Error>
    where
        A: MapAccess<'de>,
    {
        match self.node {
            Node::Root => {
                while let Some(field) = map.next_key::<RootField>()? {
                    match field {
                        RootField::Document => read_value(&mut map, Node::Document, self.context)?,
                        RootField::Engineering => {
                            read_value(&mut map, Node::Engineering, self.context)?
                        }
                        RootField::Operations => {
                            read_value(&mut map, Node::Operations, self.context)?
                        }
                        RootField::CommandReceipts => {
                            read_value(&mut map, Node::Receipts, self.context)?
                        }
                        RootField::UndoStack | RootField::RedoStack => {
                            read_value(&mut map, Node::HistoryStack, self.context)?
                        }
                        RootField::RevisionStates => {
                            read_value(&mut map, Node::RevisionStates, self.context)?
                        }
                        RootField::Other => skip_value(&mut map)?,
                    }
                }
            }
            Node::Document => {
                while let Some(field) = map.next_key::<DocumentField>()? {
                    match field {
                        DocumentField::Engineering => {
                            read_value(&mut map, Node::Engineering, self.context)?
                        }
                        DocumentField::Operations => {
                            read_value(&mut map, Node::Operations, self.context)?
                        }
                        DocumentField::CommandReceipts => {
                            read_value(&mut map, Node::Receipts, self.context)?
                        }
                        DocumentField::UndoStack | DocumentField::RedoStack => {
                            read_value(&mut map, Node::HistoryStack, self.context)?
                        }
                        DocumentField::RevisionStates => {
                            read_value(&mut map, Node::RevisionStates, self.context)?
                        }
                        DocumentField::Other => skip_value(&mut map)?,
                    }
                }
            }
            Node::Engineering => {
                while let Some(field) = map.next_key::<EngineeringField>()? {
                    match field {
                        EngineeringField::CircularCases => {
                            read_value(&mut map, Node::CircularCases, self.context)?
                        }
                        EngineeringField::Other => skip_value(&mut map)?,
                    }
                }
            }
            Node::CircularCases | Node::Operations | Node::Receipts | Node::RevisionStates => {
                let child = match self.node {
                    Node::CircularCases => Node::CircularCase,
                    Node::Operations => Node::Operation,
                    Node::Receipts => Node::StoredCommand,
                    Node::RevisionStates => Node::RevisionState,
                    _ => unreachable!(),
                };
                while map.next_key::<AnyMapKey>()?.is_some() {
                    read_value(&mut map, child, self.context)?;
                }
            }
            Node::CircularCase => {
                while let Some(field) = map.next_key::<CircularCaseField>()? {
                    match field {
                        CircularCaseField::CircularSnapshot => {
                            map.next_value_seed(SnapshotSeed {
                                context: &mut *self.context,
                            })?;
                        }
                        CircularCaseField::Other => skip_value(&mut map)?,
                    }
                }
            }
            Node::Operation => {
                while let Some(field) = map.next_key::<OperationField>()? {
                    match field {
                        OperationField::Kind => {
                            read_value(&mut map, Node::OperationKind, self.context)?
                        }
                        OperationField::Other => skip_value(&mut map)?,
                    }
                }
            }
            Node::OperationKind => {
                while let Some(variant) = map.next_key::<OperationKindVariant>()? {
                    match variant {
                        OperationKindVariant::EngineeringChanged => {
                            read_value(&mut map, Node::EngineeringChanged, self.context)?
                        }
                        OperationKindVariant::Other => skip_value(&mut map)?,
                    }
                }
            }
            Node::EngineeringChanged => {
                while let Some(field) = map.next_key::<EngineeringChangedField>()? {
                    match field {
                        EngineeringChangedField::Command => {
                            read_value(&mut map, Node::EngineeringCommand, self.context)?
                        }
                        EngineeringChangedField::Other => skip_value(&mut map)?,
                    }
                }
            }
            Node::EngineeringCommand => {
                while let Some(variant) = map.next_key::<EngineeringCommandVariant>()? {
                    match variant {
                        EngineeringCommandVariant::SaveCircularSnapshot => {
                            read_value(&mut map, Node::SaveCircularSnapshot, self.context)?
                        }
                        EngineeringCommandVariant::Other => skip_value(&mut map)?,
                    }
                }
            }
            Node::SaveCircularSnapshot => {
                while let Some(field) = map.next_key::<SaveSnapshotField>()? {
                    match field {
                        SaveSnapshotField::Snapshot => {
                            map.next_value_seed(SnapshotSeed {
                                context: &mut *self.context,
                            })?;
                        }
                        SaveSnapshotField::Other => skip_value(&mut map)?,
                    }
                }
            }
            Node::StoredCommand => {
                while let Some(field) = map.next_key::<StoredCommandField>()? {
                    match field {
                        StoredCommandField::PayloadJson => {
                            map.next_value_seed(PayloadSeed {
                                context: &mut *self.context,
                            })?;
                        }
                        StoredCommandField::RecordedCommands => {
                            read_value(&mut map, Node::RecordedCommands, self.context)?
                        }
                        StoredCommandField::Other => skip_value(&mut map)?,
                    }
                }
            }
            Node::Command => {
                while let Some(variant) = map.next_key::<CommandVariant>()? {
                    match variant {
                        CommandVariant::Engineering => {
                            read_value(&mut map, Node::EngineeringCommand, self.context)?
                        }
                        CommandVariant::Other => skip_value(&mut map)?,
                    }
                }
            }
            Node::HistoryEntry => {
                while let Some(field) = map.next_key::<HistoryEntryField>()? {
                    match field {
                        HistoryEntryField::Changes => {
                            read_value(&mut map, Node::Changes, self.context)?
                        }
                        HistoryEntryField::Other => skip_value(&mut map)?,
                    }
                }
            }
            Node::Change => {
                while let Some(variant) = map.next_key::<ChangeVariant>()? {
                    match variant {
                        ChangeVariant::CircularEngineeringCase => {
                            read_value(&mut map, Node::CircularCaseChange, self.context)?
                        }
                        ChangeVariant::Operation => {
                            read_value(&mut map, Node::OperationChange, self.context)?
                        }
                        ChangeVariant::Other => skip_value(&mut map)?,
                    }
                }
            }
            Node::CircularCaseChange => {
                while let Some(field) = map.next_key::<BeforeAfterField>()? {
                    match field {
                        BeforeAfterField::Before | BeforeAfterField::After => {
                            map.next_value_seed(OptionalNodeSeed {
                                node: Node::CircularCase,
                                context: &mut *self.context,
                            })?;
                        }
                        BeforeAfterField::Other => skip_value(&mut map)?,
                    }
                }
            }
            Node::OperationChange => {
                while let Some(field) = map.next_key::<BeforeAfterField>()? {
                    match field {
                        BeforeAfterField::Before | BeforeAfterField::After => {
                            map.next_value_seed(OptionalNodeSeed {
                                node: Node::Operation,
                                context: &mut *self.context,
                            })?;
                        }
                        BeforeAfterField::Other => skip_value(&mut map)?,
                    }
                }
            }
            Node::RevisionState => {
                while let Some(field) = map.next_key::<RevisionStateField>()? {
                    match field {
                        RevisionStateField::Engineering => {
                            read_value(&mut map, Node::Engineering, self.context)?
                        }
                        RevisionStateField::Operations => {
                            read_value(&mut map, Node::Operations, self.context)?
                        }
                        RevisionStateField::Delta => {
                            read_value(&mut map, Node::RevisionDelta, self.context)?
                        }
                        RevisionStateField::HistoryStackAnchor => {
                            read_value(&mut map, Node::HistoryStackAnchor, self.context)?
                        }
                        RevisionStateField::Other => skip_value(&mut map)?,
                    }
                }
            }
            Node::RevisionDelta => {
                while let Some(field) = map.next_key::<RevisionDeltaField>()? {
                    match field {
                        RevisionDeltaField::Changes => {
                            read_value(&mut map, Node::Changes, self.context)?
                        }
                        RevisionDeltaField::Other => skip_value(&mut map)?,
                    }
                }
            }
            Node::HistoryStackAnchor => {
                while let Some(field) = map.next_key::<HistoryAnchorField>()? {
                    match field {
                        HistoryAnchorField::Undo | HistoryAnchorField::Redo => {
                            read_value(&mut map, Node::HistoryStack, self.context)?
                        }
                        HistoryAnchorField::Other => skip_value(&mut map)?,
                    }
                }
            }
            Node::CommandBatch | Node::RecordedCommands | Node::HistoryStack | Node::Changes => {
                // A wrong container shape is left for the typed decoder to
                // reject. It is not a typed authority path.
                while map.next_key::<AnyMapKey>()?.is_some() {
                    skip_value(&mut map)?;
                }
            }
        }
        Ok(())
    }

    fn visit_seq<A>(self, sequence: A) -> Result<(), A::Error>
    where
        A: SeqAccess<'de>,
    {
        let child = match self.node {
            Node::CommandBatch | Node::RecordedCommands => Some(Node::Command),
            Node::HistoryStack => Some(Node::HistoryEntry),
            Node::Changes => Some(Node::Change),
            _ => None,
        };
        let mut this = self;
        this.visit_sequence(sequence, child)
    }

    fn visit_unit<E>(self) -> Result<(), E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_none<E>(self) -> Result<(), E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_bool<E>(self, _value: bool) -> Result<(), E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_i64<E>(self, _value: i64) -> Result<(), E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_u64<E>(self, _value: u64) -> Result<(), E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_f64<E>(self, _value: f64) -> Result<(), E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_str<E>(self, _value: &str) -> Result<(), E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_string<E>(self, _value: String) -> Result<(), E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_bytes<E>(self, _value: &[u8]) -> Result<(), E>
    where
        E: de::Error,
    {
        Ok(())
    }
}

fn read_value<'de, A>(map: &mut A, node: Node, context: &mut ProbeContext) -> Result<(), A::Error>
where
    A: MapAccess<'de>,
{
    map.next_value_seed(NodeSeed { node, context })
}

fn skip_value<'de, A>(map: &mut A) -> Result<(), A::Error>
where
    A: MapAccess<'de>,
{
    let _: IgnoredAny = map.next_value()?;
    Ok(())
}

struct OptionalNodeSeed<'a> {
    node: Node,
    context: &'a mut ProbeContext,
}

impl<'de> DeserializeSeed<'de> for OptionalNodeSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        deserializer.deserialize_option(OptionalNodeVisitor {
            node: self.node,
            context: self.context,
        })
    }
}

struct OptionalNodeVisitor<'a> {
    node: Node,
    context: &'a mut ProbeContext,
}

impl<'de> Visitor<'de> for OptionalNodeVisitor<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an optional native value")
    }

    fn visit_none<E>(self) -> Result<(), E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_unit<E>(self) -> Result<(), E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_some<D>(self, deserializer: D) -> Result<(), D::Error>
    where
        D: de::Deserializer<'de>,
    {
        NodeSeed {
            node: self.node,
            context: self.context,
        }
        .deserialize(deserializer)
    }
}

struct SnapshotSeed<'a> {
    context: &'a mut ProbeContext,
}

impl<'de> DeserializeSeed<'de> for SnapshotSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        deserializer.deserialize_option(SnapshotVisitor {
            context: self.context,
        })
    }
}

struct SnapshotVisitor<'a> {
    context: &'a mut ProbeContext,
}

impl<'de> Visitor<'de> for SnapshotVisitor<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a nullable typed circular snapshot")
    }

    fn visit_none<E>(self) -> Result<(), E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_unit<E>(self) -> Result<(), E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_some<D>(self, deserializer: D) -> Result<(), D::Error>
    where
        D: de::Deserializer<'de>,
    {
        let (limit, scope) = self.context.snapshot_span_limit().map_err(|hit| {
            self.context.accounting.borrow_mut().limit_hit = Some(hit);
            <D::Error as de::Error>::custom("native circular archive preflight limit exceeded")
        })?;
        let start = self.context.begin_span(scope, Some(limit)).map_err(|hit| {
            self.context.accounting.borrow_mut().limit_hit = Some(hit);
            <D::Error as de::Error>::custom("native circular archive preflight limit exceeded")
        })?;

        let parsed = IgnoredAny::deserialize(deserializer);
        let measured = self.context.end_span(start);
        match (parsed, measured) {
            (_, Err(hit)) => {
                self.context.accounting.borrow_mut().limit_hit = Some(hit);
                Err(<D::Error as de::Error>::custom(
                    "native circular archive preflight limit exceeded",
                ))
            }
            (Err(error), Ok(_)) => Err(error),
            (Ok(_), Ok(bytes)) => self.context.record_snapshot(bytes).map_err(|hit| {
                self.context.accounting.borrow_mut().limit_hit = Some(hit);
                <D::Error as de::Error>::custom("native circular archive preflight limit exceeded")
            }),
        }
    }
}

struct PayloadSeed<'a> {
    context: &'a mut ProbeContext,
}

impl<'de> DeserializeSeed<'de> for PayloadSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        deserializer.deserialize_option(PayloadVisitor {
            context: self.context,
        })
    }
}

struct PayloadVisitor<'a> {
    context: &'a mut ProbeContext,
}

impl<'de> Visitor<'de> for PayloadVisitor<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a stored request JSON value")
    }

    fn visit_none<E>(self) -> Result<(), E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_unit<E>(self) -> Result<(), E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_some<D>(self, deserializer: D) -> Result<(), D::Error>
    where
        D: de::Deserializer<'de>,
    {
        let limit = self.context.request_span_limit().map_err(|hit| {
            self.context.accounting.borrow_mut().limit_hit = Some(hit);
            <D::Error as de::Error>::custom("native circular archive preflight limit exceeded")
        })?;
        let start = self
            .context
            .begin_span(RETAINED_BYTES_SCOPE, limit)
            .map_err(|hit| {
                self.context.accounting.borrow_mut().limit_hit = Some(hit);
                <D::Error as de::Error>::custom("native circular archive preflight limit exceeded")
            })?;

        let parsed = IgnoredAny::deserialize(deserializer);
        let measured = self.context.end_span(start);
        match (parsed, measured) {
            (_, Err(hit)) => {
                self.context.accounting.borrow_mut().limit_hit = Some(hit);
                Err(<D::Error as de::Error>::custom(
                    "native circular archive preflight limit exceeded",
                ))
            }
            (Err(error), Ok(_)) => Err(error),
            (Ok(_), Ok(bytes)) => self.context.record_request_json(bytes).map_err(|hit| {
                self.context.accounting.borrow_mut().limit_hit = Some(hit);
                <D::Error as de::Error>::custom("native circular archive preflight limit exceeded")
            }),
        }
    }
}

macro_rules! field_enum {
    ($name:ident { $($variant:ident),* $(,)? }) => {
        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "snake_case")]
        enum $name {
            $($variant,)*
            #[serde(other)]
            Other,
        }
    };
}

field_enum!(RootField {
    Document,
    Engineering,
    Operations,
    CommandReceipts,
    UndoStack,
    RedoStack,
    RevisionStates,
});
field_enum!(DocumentField {
    Engineering,
    Operations,
    CommandReceipts,
    UndoStack,
    RedoStack,
    RevisionStates,
});
field_enum!(EngineeringField { CircularCases });
field_enum!(CircularCaseField { CircularSnapshot });
field_enum!(OperationField { Kind });
field_enum!(EngineeringChangedField { Command });
field_enum!(SaveSnapshotField { Snapshot });
field_enum!(StoredCommandField {
    PayloadJson,
    RecordedCommands
});
field_enum!(HistoryEntryField { Changes });
field_enum!(BeforeAfterField { Before, After });
field_enum!(RevisionStateField {
    Engineering,
    Operations,
    Delta,
    HistoryStackAnchor,
});
field_enum!(RevisionDeltaField { Changes });
field_enum!(HistoryAnchorField { Undo, Redo });

macro_rules! variant_enum {
    ($name:ident { $($variant:ident => $wire:literal),* $(,)? }) => {
        #[derive(Deserialize)]
        enum $name {
            $(#[serde(rename = $wire)] $variant,)*
            #[serde(other)]
            Other,
        }
    };
}

variant_enum!(OperationKindVariant { EngineeringChanged => "engineering_changed" });
variant_enum!(EngineeringCommandVariant { SaveCircularSnapshot => "save_circular_snapshot" });
variant_enum!(CommandVariant { Engineering => "engineering" });
variant_enum!(ChangeVariant {
    CircularEngineeringCase => "CircularEngineeringCase",
    Operation => "Operation",
});

#[derive(Deserialize)]
#[serde(field_identifier)]
enum AnyMapKey {
    #[serde(other)]
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preflight(source: &str) -> NativeArchivePreflight {
        preflight_circular_archive_native(source).expect("probe should accept valid JSON")
    }

    #[test]
    fn escaped_semantic_keys_reach_typed_snapshot_authority() {
        let usage = preflight(
            r#"{"document":{"engine\u0065ring":{"circular_cases":{"1":{"circular_snapshot":{"archive":[]}}}}}}"#,
        );
        assert_eq!(usage.snapshot_occurrences, 1);
        assert!(usage.snapshot_bytes > 0);
    }

    #[test]
    fn retained_only_snapshot_authorities_are_counted() {
        let usage = preflight(
            r#"{"undo_stack":[{"changes":[{"CircularEngineeringCase":{"before":{"circular_snapshot":{"a":1}},"after":null}},{"Operation":{"before":{"kind":{"engineering_changed":{"command":{"save_circular_snapshot":{"snapshot":{"b":2}}}}}},"after":null}}]}],"revision_states":{"3":{"delta":{"changes":[{"CircularEngineeringCase":{"before":null,"after":{"circular_snapshot":{"c":3}}}}]},"history_stack_anchor":{"redo":[{"changes":[{"CircularEngineeringCase":{"before":null,"after":{"circular_snapshot":{"d":4}}}}]}]}}}}"#,
        );
        assert_eq!(usage.snapshot_occurrences, 4);
    }

    #[test]
    fn duplicate_typed_fields_are_each_charged() {
        let usage = preflight(
            r#"{"engineering":{"circular_cases":{"1":{"circular_snapshot":{"a":1},"circular_snapshot":{"b":2}}}}}"#,
        );
        assert_eq!(usage.snapshot_occurrences, 2);
    }

    #[test]
    fn standalone_command_payload_uses_the_same_snapshot_quotas() {
        let usage = preflight_circular_command_payload(
            r#"[{"engineering":{"save_circular_snapshot":{"analysis_id":1,"snapshot":{"archive":[]}}}}]"#,
        )
        .expect("command payload should parse");
        assert_eq!(usage.snapshot_occurrences, 1);
        assert_eq!(usage.request_json_bytes, 0);
    }

    #[test]
    fn archive_absent_document_keeps_request_bytes_out_of_archive_budget() {
        let usage = preflight(
            r#"{"command_receipts":{"cmd":{"payload_json":"[1,2,3]","recorded_commands":[]}},"engineering":{"slope_cases":{}}}"#,
        );
        assert_eq!(usage.snapshot_occurrences, 0);
        assert_eq!(usage.snapshot_bytes, 0);
        assert_eq!(usage.request_json_bytes, 0);
    }
}
