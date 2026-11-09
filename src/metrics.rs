//! In-memory aggregate timings. Labels cannot contain device or request data.
use serde::Serialize;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize)]
pub enum Operation {
    #[serde(rename = "tv_connect")]
    Connect,
    #[serde(rename = "tv_observe")]
    Observe,
    #[serde(rename = "tv_press")]
    Press,
    #[serde(rename = "tv_launch")]
    Launch,
    #[serde(rename = "tv_disconnect")]
    Disconnect,
}

impl Operation {
    const ALL: [Self; 5] = [
        Self::Connect,
        Self::Observe,
        Self::Press,
        Self::Launch,
        Self::Disconnect,
    ];

    fn stages(self) -> &'static [Stage] {
        match self {
            Self::Connect => &[
                Stage::CredentialLookup,
                Stage::ConnectionSetup,
                Stage::TcpConnect,
                Stage::TlsHandshake,
                Stage::WebsocketHandshake,
                Stage::Registration,
            ],
            Self::Observe => &[
                Stage::CaptureRequest,
                Stage::ImageTransfer,
                Stage::ImageValidation,
                Stage::ConnectionSetup,
                Stage::ConnectionCleanup,
            ],
            Self::Press | Self::Launch => &[
                Stage::InputConnection,
                Stage::CommandDelivery,
                Stage::CaptureRequest,
                Stage::ImageTransfer,
                Stage::ImageValidation,
                Stage::ConnectionSetup,
                Stage::ConnectionCleanup,
            ],
            Self::Disconnect => &[Stage::ConnectionCleanup],
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    CredentialLookup,
    ConnectionSetup,
    TcpConnect,
    TlsHandshake,
    WebsocketHandshake,
    Registration,
    CaptureRequest,
    ImageTransfer,
    ImageValidation,
    InputConnection,
    CommandDelivery,
    ConnectionCleanup,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    Success,
    Failure,
    Timeout,
    Cancelled,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Outcomes {
    pub success: u64,
    pub failure: u64,
    pub timeout: u64,
    pub cancelled: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Statistics {
    pub count: u64,
    pub outcomes: Outcomes,
    pub total_ms: Option<f64>,
    pub min_ms: Option<f64>,
    pub max_ms: Option<f64>,
    pub mean_ms: Option<f64>,
    /// On saturation, counts stop accepting samples; overflowing sums become null.
    pub saturated: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    pub scope: &'static str,
    pub collection: &'static str,
    pub since_start_seconds: f64,
    pub unit: &'static str,
    pub operations: BTreeMap<Operation, Statistics>,
    pub stages: BTreeMap<Operation, BTreeMap<Stage, Statistics>>,
}

#[derive(Default)]
struct Accumulator {
    count: u64,
    outcomes: Outcomes,
    total_nanos: u64,
    min_nanos: Option<u64>,
    max_nanos: Option<u64>,
    timing_overflow: bool,
    count_saturated: bool,
}

impl Accumulator {
    fn record(&mut self, elapsed: Duration, outcome: Outcome) {
        if self.count == u64::MAX {
            self.count_saturated = true;
            return;
        }
        self.count += 1;
        let counter = match outcome {
            Outcome::Success => &mut self.outcomes.success,
            Outcome::Failure => &mut self.outcomes.failure,
            Outcome::Timeout => &mut self.outcomes.timeout,
            Outcome::Cancelled => &mut self.outcomes.cancelled,
        };
        *counter += 1;
        let nanos = u64::try_from(elapsed.as_nanos()).unwrap_or_else(|_| {
            self.timing_overflow = true;
            u64::MAX
        });
        self.total_nanos = self.total_nanos.checked_add(nanos).unwrap_or_else(|| {
            self.timing_overflow = true;
            u64::MAX
        });
        self.min_nanos = Some(self.min_nanos.map_or(nanos, |old| old.min(nanos)));
        self.max_nanos = Some(self.max_nanos.map_or(nanos, |old| old.max(nanos)));
    }

    fn snapshot(&self) -> Statistics {
        let total = (self.count > 0 && !self.timing_overflow)
            .then_some(self.total_nanos as f64 / 1_000_000.0);
        Statistics {
            count: self.count,
            outcomes: self.outcomes.clone(),
            total_ms: total,
            min_ms: self.min_nanos.map(|n| n as f64 / 1_000_000.0),
            max_ms: self.max_nanos.map(|n| n as f64 / 1_000_000.0),
            mean_ms: total.map(|total| total / self.count as f64),
            saturated: self.timing_overflow || self.count_saturated,
        }
    }
}

struct Counters {
    operations: BTreeMap<Operation, Accumulator>,
    stages: BTreeMap<Operation, BTreeMap<Stage, Accumulator>>,
}

impl Default for Counters {
    fn default() -> Self {
        Self {
            operations: Operation::ALL
                .into_iter()
                .map(|op| (op, Accumulator::default()))
                .collect(),
            stages: Operation::ALL
                .into_iter()
                .map(|op| {
                    (
                        op,
                        op.stages()
                            .iter()
                            .map(|&stage| (stage, Accumulator::default()))
                            .collect(),
                    )
                })
                .collect(),
        }
    }
}

struct Shared {
    enabled: bool,
    started: Instant,
    counters: Mutex<Counters>,
}

#[derive(Clone)]
pub struct Metrics(Arc<Shared>);

impl Default for Metrics {
    fn default() -> Self {
        Self::new(true)
    }
}

impl Metrics {
    pub fn new(enabled: bool) -> Self {
        Self(Arc::new(Shared {
            enabled,
            started: Instant::now(),
            counters: Mutex::new(Counters::default()),
        }))
    }

    pub fn begin(&self, operation: Operation) -> Measurement {
        self.measurement(operation, None)
    }

    pub fn stage(&self, operation: Operation, stage: Stage) -> Measurement {
        self.measurement(operation, Some(stage))
    }

    fn measurement(&self, operation: Operation, stage: Option<Stage>) -> Measurement {
        Measurement {
            recording: self.0.enabled.then(|| Recording {
                metrics: self.clone(),
                operation,
                stage,
                started: Instant::now(),
            }),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let counters = self.0.counters.lock().unwrap_or_else(|p| p.into_inner());
        Snapshot {
            scope: "current_server_process",
            collection: if self.0.enabled {
                "enabled"
            } else {
                "disabled"
            },
            since_start_seconds: self.0.started.elapsed().as_secs_f64(),
            unit: "milliseconds",
            operations: counters
                .operations
                .iter()
                .map(|(&op, value)| (op, value.snapshot()))
                .collect(),
            stages: counters
                .stages
                .iter()
                .map(|(&op, stages)| {
                    (
                        op,
                        stages
                            .iter()
                            .map(|(&stage, value)| (stage, value.snapshot()))
                            .collect(),
                    )
                })
                .collect(),
        }
    }

    fn record(
        &self,
        operation: Operation,
        stage: Option<Stage>,
        elapsed: Duration,
        outcome: Outcome,
    ) {
        let mut counters = self.0.counters.lock().unwrap_or_else(|p| p.into_inner());
        let accumulator = match stage {
            Some(stage) => counters
                .stages
                .entry(operation)
                .or_default()
                .entry(stage)
                .or_default(),
            None => counters.operations.entry(operation).or_default(),
        };
        accumulator.record(elapsed, outcome);
    }
}

struct Recording {
    metrics: Metrics,
    operation: Operation,
    stage: Option<Stage>,
    started: Instant,
}

#[must_use = "keep the measurement until the operation finishes"]
pub struct Measurement {
    recording: Option<Recording>,
}

impl Measurement {
    pub fn finish(mut self, outcome: Outcome) {
        self.record(outcome);
    }

    fn record(&mut self, outcome: Outcome) {
        if let Some(recording) = self.recording.take() {
            recording.metrics.record(
                recording.operation,
                recording.stage,
                recording.started.elapsed(),
                outcome,
            );
        }
    }
}

impl Drop for Measurement {
    fn drop(&mut self) {
        self.record(Outcome::Cancelled);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn finalization_counts_each_outcome_once_and_excludes_in_flight_work() {
        let metrics = Metrics::new(true);
        let active = metrics.begin(Operation::Observe);
        assert_eq!(metrics.snapshot().operations[&Operation::Observe].count, 0);
        assert_eq!(
            metrics.snapshot().operations[&Operation::Observe].total_ms,
            None
        );
        active.finish(Outcome::Success);
        metrics.begin(Operation::Observe).finish(Outcome::Failure);
        metrics.begin(Operation::Observe).finish(Outcome::Timeout);
        drop(metrics.begin(Operation::Observe));
        let snapshot = metrics.snapshot();
        let observe = &snapshot.operations[&Operation::Observe];
        assert_eq!(observe.count, 4);
        assert_eq!(
            serde_json::to_value(&observe.outcomes).unwrap(),
            json!({
                "success": 1, "failure": 1, "timeout": 1, "cancelled": 1
            })
        );
        assert_eq!(snapshot.operations[&Operation::Connect].count, 0);
        assert_eq!(snapshot.operations[&Operation::Connect].mean_ms, None);
        assert_eq!(
            snapshot.stages[&Operation::Observe][&Stage::ImageTransfer].count,
            0
        );
    }

    #[test]
    fn stage_timing_uses_the_same_shared_clock_without_changing_operation_counts() {
        let metrics = Metrics::new(true);
        let other = metrics.clone();
        let stage = other.stage(Operation::Observe, Stage::ImageTransfer);
        std::thread::sleep(Duration::from_millis(3));
        stage.finish(Outcome::Success);
        let snapshot = metrics.snapshot();
        let stage = &snapshot.stages[&Operation::Observe][&Stage::ImageTransfer];
        assert_eq!(stage.count, 1);
        assert_eq!(stage.outcomes.success, 1);
        assert!(stage.min_ms.unwrap() >= 3.0);
        assert_eq!(stage.min_ms, stage.max_ms);
        assert_eq!(stage.total_ms, stage.mean_ms);
        assert_eq!(snapshot.operations[&Operation::Observe].count, 0);
    }

    #[test]
    fn disabled_collection_does_not_record_completion_or_cancellation() {
        let metrics = Metrics::new(false);
        metrics.begin(Operation::Connect).finish(Outcome::Success);
        drop(metrics.begin(Operation::Observe));
        metrics
            .stage(Operation::Observe, Stage::CaptureRequest)
            .finish(Outcome::Failure);
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.collection, "disabled");
        assert_eq!(snapshot.operations[&Operation::Connect].count, 0);
        assert_eq!(
            snapshot.operations[&Operation::Observe].outcomes.cancelled,
            0
        );
        assert_eq!(
            snapshot.stages[&Operation::Observe][&Stage::CaptureRequest].count,
            0
        );
    }

    #[test]
    fn cancelling_an_in_flight_future_finalizes_it_once() {
        use std::{
            future::Future,
            task::{Context, Poll, Waker},
        };
        let metrics = Metrics::new(true);
        let operation_metrics = metrics.clone();
        let mut operation = Box::pin(async move {
            let measurement = operation_metrics.begin(Operation::Press);
            std::future::pending::<()>().await;
            measurement.finish(Outcome::Success);
        });
        let mut context = Context::from_waker(Waker::noop());
        assert_eq!(operation.as_mut().poll(&mut context), Poll::Pending);
        assert_eq!(metrics.snapshot().operations[&Operation::Press].count, 0);
        drop(operation);
        let snapshot = metrics.snapshot();
        let press = &snapshot.operations[&Operation::Press];
        assert_eq!(press.count, 1);
        assert_eq!(press.outcomes.cancelled, 1);
        assert_eq!(press.outcomes.success, 0);
        assert_eq!(press.outcomes.failure, 0);
        assert_eq!(press.outcomes.timeout, 0);
    }

    #[test]
    fn serialized_snapshot_has_only_allowlisted_keys_and_no_event_payloads() {
        let metrics = Metrics::new(true);
        metrics.begin(Operation::Press).finish(Outcome::Success);
        let value = serde_json::to_value(metrics.snapshot()).unwrap();
        let keys: Vec<_> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "collection",
                "operations",
                "scope",
                "since_start_seconds",
                "stages",
                "unit"
            ]
        );
        let operation_keys: Vec<_> = value["operations"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            operation_keys,
            [
                "tv_connect",
                "tv_disconnect",
                "tv_launch",
                "tv_observe",
                "tv_press"
            ]
        );
        let press_keys: Vec<_> = value["operations"]["tv_press"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            press_keys,
            [
                "count",
                "max_ms",
                "mean_ms",
                "min_ms",
                "outcomes",
                "saturated",
                "total_ms"
            ]
        );
        assert_eq!(value["scope"], "current_server_process");
        assert_eq!(value["unit"], "milliseconds");
    }

    #[test]
    fn aggregate_values_and_overflow_preserve_disjoint_outcome_counts() {
        let mut value = Accumulator::default();
        value.record(Duration::from_millis(2), Outcome::Success);
        value.record(Duration::from_millis(4), Outcome::Failure);
        let snapshot = value.snapshot();
        assert_eq!(snapshot.total_ms, Some(6.0));
        assert_eq!(snapshot.min_ms, Some(2.0));
        assert_eq!(snapshot.max_ms, Some(4.0));
        assert_eq!(snapshot.mean_ms, Some(3.0));
        value.total_nanos = u64::MAX;
        value.record(Duration::from_nanos(1), Outcome::Timeout);
        let snapshot = value.snapshot();
        assert!(snapshot.saturated);
        assert_eq!(snapshot.total_ms, None);
        assert_eq!(snapshot.mean_ms, None);
        value.count = u64::MAX;
        value.outcomes = Outcomes {
            success: u64::MAX,
            ..Outcomes::default()
        };
        value.record(Duration::from_secs(1), Outcome::Cancelled);
        let snapshot = value.snapshot();
        assert_eq!(snapshot.count, u64::MAX);
        assert_eq!(snapshot.outcomes.success, u64::MAX);
        assert_eq!(snapshot.outcomes.cancelled, 0);
    }
}
