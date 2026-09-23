//! Versioned binary WAL: magic header, contiguous record number, length,
//! checksum and event payload. Checksums detect corruption, not tampering.
//! Truncated tails are rejected; never silently discard a possibly-sent order.
use crate::model::*;
use crate::{
    ChildCommand, GroupControl, GroupDispatch, GroupEvent, GroupPolicy, QuantityMode, RejectReason,
    SchedulerConfig, SingleAction, SingleEvent, Target,
};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;

pub trait Journal {
    /// Must return only after the configured persistence boundary is reached.
    fn append(&mut self, event: &Event) -> io::Result<()>;
}

pub struct MemoryJournal {
    events: Vec<Event>,
    capacity: usize,
}
impl MemoryJournal {
    pub fn new(capacity: usize) -> Self {
        Self {
            events: Vec::with_capacity(capacity),
            capacity,
        }
    }
    pub fn events(&self) -> &[Event] {
        &self.events
    }
    pub fn from_events(events: &[Event], capacity: usize) -> io::Result<Self> {
        if events.len() > capacity {
            return Err(io::Error::other("memory journal capacity too small"));
        }
        let mut journal = Self::new(capacity);
        journal.events.extend_from_slice(events);
        Ok(journal)
    }
}
impl Journal for MemoryJournal {
    fn append(&mut self, event: &Event) -> io::Result<()> {
        if self.events.len() == self.capacity {
            return Err(io::Error::other("memory journal full"));
        }
        self.events.push(*event);
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Durability {
    /// Call sync_all before returning a dispatchable command. The ultimate
    /// guarantee depends on filesystem/device behavior (notably on macOS).
    SyncEveryEvent,
    /// Write to the OS cache only; recent acknowledged events can be lost.
    OsBuffered,
}

pub struct FileJournal {
    file: File,
    next: u64,
    durability: Durability,
}
const MAGIC: &[u8; 8] = b"OMSJ0001";

impl FileJournal {
    /// Refuses to overwrite an existing journal. File lock enforces one writer.
    pub fn create(path: impl AsRef<Path>, durability: Durability) -> io::Result<Self> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)?;
        file.try_lock().map_err(io::Error::other)?;
        file.write_all(MAGIC)?;
        file.sync_all()?;
        Ok(Self {
            file,
            next: 1,
            durability,
        })
    }
    /// Load with exclusive ownership and leave the cursor at the verified end.
    pub fn open(path: impl AsRef<Path>, durability: Durability) -> io::Result<(Self, Vec<Event>)> {
        let mut file = OpenOptions::new().read(true).write(true).open(path)?;
        file.try_lock().map_err(io::Error::other)?;
        let mut magic = [0; 8];
        file.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(invalid("unknown journal format"));
        }
        let mut next = 1_u64;
        let mut events = Vec::new();
        loop {
            let mut header = [0_u8; 20];
            if file.read(&mut header[..1])? == 0 {
                break;
            }
            file.read_exact(&mut header[1..])
                .map_err(|_| invalid("incomplete journal header; manual tail recovery required"))?;
            let seq = u64::from_le_bytes(header[..8].try_into().unwrap());
            let len = u32::from_le_bytes(header[8..12].try_into().unwrap()) as usize;
            let checksum = u64::from_le_bytes(header[12..].try_into().unwrap());
            if seq != next || len == 0 || len > 1024 || !len.is_multiple_of(8) {
                return Err(invalid("invalid journal record sequence or length"));
            }
            let mut payload = vec![0; len];
            file.read_exact(&mut payload).map_err(|_| {
                invalid("incomplete journal payload; manual tail recovery required")
            })?;
            if checksum != digest(seq, &payload) {
                return Err(invalid("journal checksum mismatch"));
            }
            events.push(decode(&payload)?);
            next = next
                .checked_add(1)
                .ok_or_else(|| invalid("journal sequence exhausted"))?;
        }
        Ok((
            Self {
                file,
                next,
                durability,
            },
            events,
        ))
    }
}
impl Journal for FileJournal {
    fn append(&mut self, event: &Event) -> io::Result<()> {
        let following = self
            .next
            .checked_add(1)
            .ok_or_else(|| invalid("journal sequence exhausted"))?;
        let payload = encode(event);
        let mut record = Vec::with_capacity(20 + payload.len());
        record.extend_from_slice(&self.next.to_le_bytes());
        record.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        record.extend_from_slice(&digest(self.next, &payload).to_le_bytes());
        record.extend_from_slice(&payload);
        self.file.write_all(&record)?;
        if matches!(self.durability, Durability::SyncEveryEvent) {
            self.file.sync_all()?;
        }
        self.next = following;
        Ok(())
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn digest(seq: u64, payload: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325_u64;
    for b in seq
        .to_le_bytes()
        .iter()
        .chain((payload.len() as u32).to_le_bytes().iter())
        .chain(payload.iter())
    {
        h = (h ^ u64::from(*b)).wrapping_mul(0x100000001b3);
    }
    h
}

fn lifecycle_code(s: Lifecycle) -> u64 {
    match s {
        Lifecycle::PendingNew => 0,
        Lifecycle::Working => 1,
        Lifecycle::Filled => 2,
        Lifecycle::Canceled => 3,
        Lifecycle::Rejected => 4,
        Lifecycle::Expired => 5,
    }
}

fn encode(event: &Event) -> Vec<u8> {
    let mut w = Vec::<u64>::with_capacity(20);
    match *event {
        Event::Single(SingleEvent::Submit { action, dispatch }) => {
            w.extend([8, u64::from(dispatch)]);
            w.extend(
                encode(&action.event())
                    .chunks_exact(8)
                    .map(|word| u64::from_le_bytes(word.try_into().unwrap())),
            );
        }
        Event::Single(SingleEvent::Dispatch {
            order_id,
            request_id,
            expected_version,
        }) => {
            w.extend([9, order_id, request_id, expected_version]);
        }
        Event::Group(g) => {
            w.push(7);
            encode_group(g, &mut w);
        }
        Event::New(n) => w.extend([
            1,
            n.order_id,
            n.request_id,
            n.book.strategy,
            n.book.account,
            n.book.venue,
            n.book.instrument,
            match n.book.side {
                Side::Buy => 0,
                Side::Sell => 1,
            },
            n.price as u64,
            n.total_qty,
        ]),
        Event::Cancel {
            order_id,
            request_id,
            expected_version,
        } => w.extend([2, order_id, request_id, expected_version]),
        Event::Replace {
            order_id,
            request_id,
            expected_version,
            price,
            total_qty,
        } => w.extend([
            3,
            order_id,
            request_id,
            expected_version,
            price as u64,
            total_qty,
        ]),
        Event::Timeout {
            order_id,
            request_id,
        } => w.extend([4, order_id, request_id]),
        Event::MarkUncertain { order_id } => w.extend([6, order_id]),
        Event::Report(r) => {
            w.extend([5, r.source, r.sequence, r.order_id]);
            match r.kind {
                ReportKind::Accepted {
                    request_id,
                    exchange_id,
                    price,
                    total_qty,
                } => w.extend([1, request_id, exchange_id, price as u64, total_qty]),
                ReportKind::Fill { key, qty, price } => w.extend([
                    2,
                    key.venue,
                    key.account,
                    key.trading_day,
                    key.execution_id,
                    qty,
                    price as u64,
                ]),
                ReportKind::Replaced {
                    request_id,
                    exchange_id,
                    price,
                    total_qty,
                } => w.extend([3, request_id, exchange_id, price as u64, total_qty]),
                ReportKind::Canceled { request_id } => w.extend([4, request_id.unwrap_or(0)]),
                ReportKind::Rejected { request_id } => w.extend([5, request_id]),
                ReportKind::RejectedWithReason { request_id, reason } => {
                    w.extend([9, request_id, reject_code(reason)])
                }
                ReportKind::Expired => w.push(6),
                ReportKind::Corrected {
                    key,
                    revision,
                    new_qty,
                    new_price,
                } => w.extend([
                    7,
                    key.venue,
                    key.account,
                    key.trading_day,
                    key.execution_id,
                    revision,
                    new_qty,
                    new_price as u64,
                ]),
                ReportKind::Reconciled {
                    price,
                    total_qty,
                    cum_filled,
                    leaves,
                    lifecycle,
                } => w.extend([
                    8,
                    price as u64,
                    total_qty,
                    cum_filled,
                    leaves,
                    lifecycle_code(lifecycle),
                ]),
            }
        }
    }
    w.into_iter().flat_map(u64::to_le_bytes).collect()
}

struct Words<'a> {
    bytes: &'a [u8],
    cursor: usize,
}
impl Words<'_> {
    fn u32(&mut self) -> io::Result<u32> {
        self.pop()?
            .try_into()
            .map_err(|_| invalid("u32 field overflow"))
    }
    fn book(&mut self) -> io::Result<Book> {
        Ok(Book {
            strategy: self.pop()?,
            account: self.pop()?,
            venue: self.pop()?,
            instrument: self.pop()?,
            side: match self.pop()? {
                0 => Side::Buy,
                1 => Side::Sell,
                _ => return Err(invalid("unknown side")),
            },
        })
    }
    fn rejection(&mut self) -> io::Result<RejectReason> {
        match self.pop()? {
            0 => Ok(RejectReason::Permanent),
            1 => Ok(RejectReason::Transient),
            2 => Ok(RejectReason::RateLimited),
            3 => Ok(RejectReason::Risk),
            _ => Err(invalid("unknown rejection reason")),
        }
    }
    fn pop(&mut self) -> io::Result<u64> {
        let chunk = self
            .bytes
            .get(self.cursor..self.cursor + 8)
            .ok_or_else(|| invalid("short event"))?;
        self.cursor += 8;
        Ok(u64::from_le_bytes(chunk.try_into().unwrap()))
    }
    fn key(&mut self) -> io::Result<ExecutionKey> {
        Ok(ExecutionKey {
            venue: self.pop()?,
            account: self.pop()?,
            trading_day: self.pop()?,
            execution_id: self.pop()?,
        })
    }
    fn lifecycle(&mut self) -> io::Result<Lifecycle> {
        match self.pop()? {
            0 => Ok(Lifecycle::PendingNew),
            1 => Ok(Lifecycle::Working),
            2 => Ok(Lifecycle::Filled),
            3 => Ok(Lifecycle::Canceled),
            4 => Ok(Lifecycle::Rejected),
            5 => Ok(Lifecycle::Expired),
            _ => Err(invalid("unknown lifecycle")),
        }
    }
}
fn decode(bytes: &[u8]) -> io::Result<Event> {
    let mut w = Words { bytes, cursor: 0 };
    let event = match w.pop()? {
        8 => {
            let dispatch = match w.pop()? {
                0 => false,
                1 => true,
                _ => return Err(invalid("invalid dispatch flag")),
            };
            let start = w.cursor;
            if !matches!(w.pop()?, 1..=3) {
                return Err(invalid("invalid single action tag"));
            }
            let action = SingleAction::from_event(decode(&bytes[start..])?)
                .ok_or_else(|| invalid("invalid single action"))?;
            w.cursor = bytes.len();
            Event::Single(SingleEvent::Submit { action, dispatch })
        }
        9 => Event::Single(SingleEvent::Dispatch {
            order_id: w.pop()?,
            request_id: w.pop()?,
            expected_version: w.pop()?,
        }),
        7 => Event::Group(decode_group(&mut w)?),
        1 => Event::New(NewOrder {
            order_id: w.pop()?,
            request_id: w.pop()?,
            book: Book {
                strategy: w.pop()?,
                account: w.pop()?,
                venue: w.pop()?,
                instrument: w.pop()?,
                side: match w.pop()? {
                    0 => Side::Buy,
                    1 => Side::Sell,
                    _ => return Err(invalid("unknown side")),
                },
            },
            price: w.pop()? as i64,
            total_qty: w.pop()?,
        }),
        2 => Event::Cancel {
            order_id: w.pop()?,
            request_id: w.pop()?,
            expected_version: w.pop()?,
        },
        3 => Event::Replace {
            order_id: w.pop()?,
            request_id: w.pop()?,
            expected_version: w.pop()?,
            price: w.pop()? as i64,
            total_qty: w.pop()?,
        },
        4 => Event::Timeout {
            order_id: w.pop()?,
            request_id: w.pop()?,
        },
        6 => Event::MarkUncertain { order_id: w.pop()? },
        5 => {
            let (source, sequence, order_id) = (w.pop()?, w.pop()?, w.pop()?);
            let kind = match w.pop()? {
                1 => ReportKind::Accepted {
                    request_id: w.pop()?,
                    exchange_id: w.pop()?,
                    price: w.pop()? as i64,
                    total_qty: w.pop()?,
                },
                2 => ReportKind::Fill {
                    key: w.key()?,
                    qty: w.pop()?,
                    price: w.pop()? as i64,
                },
                3 => ReportKind::Replaced {
                    request_id: w.pop()?,
                    exchange_id: w.pop()?,
                    price: w.pop()? as i64,
                    total_qty: w.pop()?,
                },
                4 => ReportKind::Canceled {
                    request_id: match w.pop()? {
                        0 => None,
                        id => Some(id),
                    },
                },
                5 => ReportKind::Rejected {
                    request_id: w.pop()?,
                },
                9 => ReportKind::RejectedWithReason {
                    request_id: w.pop()?,
                    reason: w.rejection()?,
                },
                6 => ReportKind::Expired,
                7 => ReportKind::Corrected {
                    key: w.key()?,
                    revision: w.pop()?,
                    new_qty: w.pop()?,
                    new_price: w.pop()? as i64,
                },
                8 => ReportKind::Reconciled {
                    price: w.pop()? as i64,
                    total_qty: w.pop()?,
                    cum_filled: w.pop()?,
                    leaves: w.pop()?,
                    lifecycle: w.lifecycle()?,
                },
                _ => return Err(invalid("unknown report type")),
            };
            Event::Report(Report {
                source,
                sequence,
                order_id,
                kind,
            })
        }
        _ => return Err(invalid("unknown event type")),
    };
    if w.cursor != bytes.len() {
        return Err(invalid("extra event fields"));
    }
    Ok(event)
}

fn reject_code(reason: RejectReason) -> u64 {
    match reason {
        RejectReason::Permanent => 0,
        RejectReason::Transient => 1,
        RejectReason::RateLimited => 2,
        RejectReason::Risk => 3,
    }
}

fn encode_group(event: GroupEvent, w: &mut Vec<u64>) {
    match event {
        GroupEvent::Create {
            group_id,
            book: b,
            policy: p,
        } => w.extend([
            1,
            group_id,
            b.strategy,
            b.account,
            b.venue,
            b.instrument,
            if b.side == Side::Buy { 0 } else { 1 },
            p.max_child_leaves,
            u64::from(p.max_active_children),
            u64::from(p.max_inflight),
            p.max_reserved_qty,
            u64::from(p.max_retries),
            p.retry_delay_ms,
            p.request_timeout_ms,
        ]),
        GroupEvent::SetTarget {
            group_id,
            revision,
            target,
        } => {
            w.extend([2, group_id, revision]);
            if let Some(t) = target {
                w.extend([
                    1,
                    t.price as u64,
                    t.qty,
                    if t.quantity_mode == QuantityMode::MaintainLeaves {
                        0
                    } else {
                        1
                    },
                    u64::from(t.expires_at_ms.is_some()),
                    t.expires_at_ms.unwrap_or(0),
                ]);
            } else {
                w.push(0);
            }
        }
        GroupEvent::Control { group_id, control } => w.extend([
            3,
            group_id,
            match control {
                GroupControl::Pause => 0,
                GroupControl::Resume => 1,
                GroupControl::Stop => 2,
                GroupControl::Retry => 3,
                GroupControl::ReleaseRecovery => 4,
            },
        ]),
        GroupEvent::Clock { now_ms } => w.extend([4, now_ms]),
        GroupEvent::ConfigureScheduler(c) => w.extend([
            5,
            c.window_ms,
            u64::from(c.max_actions),
            u64::from(c.cancel_reserve),
        ]),
        GroupEvent::RecoveryHold { group_id } => w.extend([6, group_id]),
        GroupEvent::Dispatch(d) => {
            w.extend([7, d.group_id, d.target_revision]);
            match d.command {
                ChildCommand::New {
                    order_id,
                    request_id,
                    price,
                    total_qty,
                } => w.extend([1, order_id, request_id, price as u64, total_qty]),
                ChildCommand::Cancel {
                    order_id,
                    request_id,
                    expected_version,
                } => w.extend([2, order_id, request_id, expected_version]),
                ChildCommand::Replace {
                    order_id,
                    request_id,
                    expected_version,
                    price,
                    total_qty,
                } => w.extend([
                    3,
                    order_id,
                    request_id,
                    expected_version,
                    price as u64,
                    total_qty,
                ]),
            }
        }
    }
}

fn decode_group(w: &mut Words<'_>) -> io::Result<GroupEvent> {
    Ok(match w.pop()? {
        1 => GroupEvent::Create {
            group_id: w.pop()?,
            book: w.book()?,
            policy: GroupPolicy {
                max_child_leaves: w.pop()?,
                max_active_children: w.u32()?,
                max_inflight: w.u32()?,
                max_reserved_qty: w.pop()?,
                max_retries: w.u32()?,
                retry_delay_ms: w.pop()?,
                request_timeout_ms: w.pop()?,
            },
        },
        2 => {
            let group_id = w.pop()?;
            let revision = w.pop()?;
            let target = match w.pop()? {
                0 => None,
                1 => {
                    let price = w.pop()? as i64;
                    let qty = w.pop()?;
                    let quantity_mode = match w.pop()? {
                        0 => QuantityMode::MaintainLeaves,
                        1 => QuantityMode::TotalExecution,
                        _ => return Err(invalid("unknown quantity mode")),
                    };
                    let present = w.pop()?;
                    let at = w.pop()?;
                    let expires_at_ms = match present {
                        0 if at == 0 => None,
                        1 => Some(at),
                        _ => return Err(invalid("invalid expiry")),
                    };
                    Some(Target {
                        price,
                        qty,
                        quantity_mode,
                        expires_at_ms,
                    })
                }
                _ => return Err(invalid("invalid target presence")),
            };
            GroupEvent::SetTarget {
                group_id,
                revision,
                target,
            }
        }
        3 => GroupEvent::Control {
            group_id: w.pop()?,
            control: match w.pop()? {
                0 => GroupControl::Pause,
                1 => GroupControl::Resume,
                2 => GroupControl::Stop,
                3 => GroupControl::Retry,
                4 => GroupControl::ReleaseRecovery,
                _ => return Err(invalid("unknown group control")),
            },
        },
        4 => GroupEvent::Clock { now_ms: w.pop()? },
        5 => GroupEvent::ConfigureScheduler(SchedulerConfig {
            window_ms: w.pop()?,
            max_actions: w.u32()?,
            cancel_reserve: w.u32()?,
        }),
        6 => GroupEvent::RecoveryHold { group_id: w.pop()? },
        7 => {
            let group_id = w.pop()?;
            let target_revision = w.pop()?;
            let command = match w.pop()? {
                1 => ChildCommand::New {
                    order_id: w.pop()?,
                    request_id: w.pop()?,
                    price: w.pop()? as i64,
                    total_qty: w.pop()?,
                },
                2 => ChildCommand::Cancel {
                    order_id: w.pop()?,
                    request_id: w.pop()?,
                    expected_version: w.pop()?,
                },
                3 => ChildCommand::Replace {
                    order_id: w.pop()?,
                    request_id: w.pop()?,
                    expected_version: w.pop()?,
                    price: w.pop()? as i64,
                    total_qty: w.pop()?,
                },
                _ => return Err(invalid("unknown child command")),
            };
            GroupEvent::Dispatch(GroupDispatch {
                group_id,
                target_revision,
                command,
            })
        }
        _ => return Err(invalid("unknown group event")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_codec_roundtrips_and_rejects_nested_or_invalid_actions() {
        let actions = [
            SingleAction::New(NewOrder {
                order_id: 1,
                request_id: 1,
                book: Book {
                    strategy: 1,
                    account: 2,
                    venue: 3,
                    instrument: 4,
                    side: Side::Sell,
                },
                price: -100,
                total_qty: 10,
            }),
            SingleAction::Replace {
                order_id: 1,
                request_id: 2,
                expected_version: 4,
                price: -99,
                total_qty: 20,
            },
            SingleAction::Cancel {
                order_id: 1,
                request_id: 3,
                expected_version: 5,
            },
        ];
        for action in actions {
            for dispatch in [false, true] {
                let event = Event::Single(SingleEvent::Submit { action, dispatch });
                assert_eq!(decode(&encode(&event)).unwrap(), event);
                let bytes = encode(&event);
                assert!(decode(&bytes[..bytes.len() - 8]).is_err());
                let mut bad = bytes.clone();
                bad[8..16].copy_from_slice(&2u64.to_le_bytes());
                assert!(decode(&bad).is_err());
                let mut nested = bytes;
                nested[16..24].copy_from_slice(&8u64.to_le_bytes());
                assert!(decode(&nested).is_err());
            }
        }
        let event = Event::Single(SingleEvent::Dispatch {
            order_id: 1,
            request_id: 2,
            expected_version: 9,
        });
        assert_eq!(decode(&encode(&event)).unwrap(), event);
    }
    #[test]
    fn group_codec_covers_controls_targets_dispatches_and_rejections() {
        let book = Book {
            strategy: 1,
            account: 2,
            venue: 3,
            instrument: 4,
            side: Side::Sell,
        };
        let mut groups = vec![
            GroupEvent::Create {
                group_id: 9,
                book,
                policy: GroupPolicy::default(),
            },
            GroupEvent::Clock { now_ms: 1234 },
            GroupEvent::ConfigureScheduler(SchedulerConfig::default()),
            GroupEvent::RecoveryHold { group_id: 9 },
            GroupEvent::SetTarget {
                group_id: 9,
                revision: 1,
                target: None,
            },
        ];
        for mode in [QuantityMode::MaintainLeaves, QuantityMode::TotalExecution] {
            for expires_at_ms in [None, Some(0), Some(u64::MAX)] {
                groups.push(GroupEvent::SetTarget {
                    group_id: 9,
                    revision: 2,
                    target: Some(Target {
                        price: -42,
                        qty: 99,
                        quantity_mode: mode,
                        expires_at_ms,
                    }),
                });
            }
        }
        for control in [
            GroupControl::Pause,
            GroupControl::Resume,
            GroupControl::Stop,
            GroupControl::Retry,
            GroupControl::ReleaseRecovery,
        ] {
            groups.push(GroupEvent::Control {
                group_id: 9,
                control,
            });
        }
        for command in [
            ChildCommand::New {
                order_id: 8,
                request_id: 7,
                price: -4,
                total_qty: 3,
            },
            ChildCommand::Cancel {
                order_id: 8,
                request_id: 7,
                expected_version: 6,
            },
            ChildCommand::Replace {
                order_id: 8,
                request_id: 7,
                expected_version: 6,
                price: -5,
                total_qty: 9,
            },
        ] {
            groups.push(GroupEvent::Dispatch(GroupDispatch {
                group_id: 9,
                target_revision: 2,
                command,
            }));
        }
        for event in groups.into_iter().map(Event::Group) {
            assert_eq!(decode(&encode(&event)).unwrap(), event);
        }
        for reason in [
            RejectReason::Permanent,
            RejectReason::Transient,
            RejectReason::RateLimited,
            RejectReason::Risk,
        ] {
            let event = Event::Report(Report {
                source: 1,
                sequence: 1,
                order_id: 8,
                kind: ReportKind::RejectedWithReason {
                    request_id: 7,
                    reason,
                },
            });
            assert_eq!(decode(&encode(&event)).unwrap(), event);
        }
    }

    #[test]
    fn codec_covers_all_variants_and_signed_prices() {
        let key = ExecutionKey {
            venue: 2,
            account: 3,
            trading_day: 20260919,
            execution_id: 4,
        };
        let mut events = vec![
            Event::New(NewOrder {
                order_id: 1,
                request_id: 2,
                book: Book {
                    strategy: 1,
                    account: 3,
                    venue: 2,
                    instrument: 4,
                    side: Side::Sell,
                },
                price: -123,
                total_qty: 99,
            }),
            Event::Cancel {
                order_id: 1,
                request_id: 3,
                expected_version: 5,
            },
            Event::Replace {
                order_id: 1,
                request_id: 4,
                expected_version: 6,
                price: -99,
                total_qty: 100,
            },
            Event::Timeout {
                order_id: 1,
                request_id: 4,
            },
            Event::MarkUncertain { order_id: 1 },
        ];
        let reports = [
            ReportKind::Accepted {
                request_id: 2,
                exchange_id: 7,
                price: -123,
                total_qty: 99,
            },
            ReportKind::Fill {
                key,
                qty: 10,
                price: -123,
            },
            ReportKind::Replaced {
                request_id: 4,
                exchange_id: 8,
                price: -99,
                total_qty: 100,
            },
            ReportKind::Canceled {
                request_id: Some(3),
            },
            ReportKind::Canceled { request_id: None },
            ReportKind::Rejected { request_id: 4 },
            ReportKind::Expired,
            ReportKind::Corrected {
                key,
                revision: 1,
                new_qty: 0,
                new_price: -100,
            },
            ReportKind::Reconciled {
                price: -123,
                total_qty: 99,
                cum_filled: 0,
                leaves: 99,
                lifecycle: Lifecycle::Working,
            },
        ];
        events.extend(reports.into_iter().map(|kind| {
            Event::Report(Report {
                source: 8,
                sequence: 9,
                order_id: 1,
                kind,
            })
        }));
        for event in events {
            assert_eq!(decode(&encode(&event)).unwrap(), event);
        }
    }
}
