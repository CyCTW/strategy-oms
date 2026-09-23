//! Adapter seam. A successful send only confirms transport handling, not venue
//! acceptance. Prefer Engine::on_report for guarded normalized report ingress.
use crate::model::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayCommand {
    New(NewOrder),
    Change {
        order_id: OrderId,
        exchange_id: Option<u64>,
        request: Request,
    },
}

pub trait Gateway {
    type Error;
    fn send(&mut self, command: GatewayCommand) -> Result<(), Self::Error>;
}

#[derive(Default)]
pub struct SimGateway {
    pub sent: Vec<GatewayCommand>,
}
impl Gateway for SimGateway {
    type Error = std::convert::Infallible;
    fn send(&mut self, command: GatewayCommand) -> Result<(), Self::Error> {
        self.sent.push(command);
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimulationError {
    Capacity,
    DeliveryUnknown,
}
impl std::fmt::Display for SimulationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for SimulationError {}

/// Scriptable, bounded test transport. It never invents an acceptance: tests
/// explicitly schedule venue results, including duplicates and sequence gaps.
pub struct FaultGateway {
    sent: Vec<GatewayCommand>,
    reports: std::collections::BTreeMap<(u64, u64), Report>,
    capacity: usize,
    serial: u64,
    fail_next: bool,
}
impl FaultGateway {
    pub fn new(capacity: usize) -> Self {
        Self {
            sent: Vec::with_capacity(capacity),
            reports: std::collections::BTreeMap::new(),
            capacity,
            serial: 0,
            fail_next: false,
        }
    }
    pub fn sent(&self) -> &[GatewayCommand] {
        &self.sent
    }
    /// Next send records an attempt and returns an ambiguous transport failure.
    pub fn fail_next_send(&mut self) {
        self.fail_next = true;
    }
    /// Schedule twice with the same Report to inject a duplicate. Omit the
    /// report for a timeout, or supply non-contiguous sequence numbers for a gap.
    pub fn schedule_report(&mut self, at_ms: u64, report: Report) -> Result<(), SimulationError> {
        if self.reports.len() >= self.capacity {
            return Err(SimulationError::Capacity);
        }
        let serial = self
            .serial
            .checked_add(1)
            .ok_or(SimulationError::Capacity)?;
        self.reports.insert((at_ms, serial), report);
        self.serial = serial;
        Ok(())
    }
    pub fn poll_report(&mut self, now_ms: u64) -> Option<Report> {
        if self
            .reports
            .first_key_value()
            .is_some_and(|((at, _), _)| *at <= now_ms)
        {
            self.reports.pop_first().map(|(_, report)| report)
        } else {
            None
        }
    }
}
impl Gateway for FaultGateway {
    type Error = SimulationError;
    fn send(&mut self, command: GatewayCommand) -> Result<(), Self::Error> {
        if self.sent.len() >= self.capacity {
            return Err(SimulationError::Capacity);
        }
        self.sent.push(command);
        if std::mem::take(&mut self.fail_next) {
            Err(SimulationError::DeliveryUnknown)
        } else {
            Ok(())
        }
    }
}
