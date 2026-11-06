----------------------------- MODULE TvSession -----------------------------
EXTENDS TLC
VARIABLES phase, lastResult, resources, unresolved, lateApplied, durable
vars == <<phase, lastResult, resources, unresolved, lateApplied, durable>>
Phases == {"disconnected", "connecting", "needs_observation", "observing", "ready", "stale", "checking", "acting", "uncertain", "observing_uncertain", "closing", "shutdown_closing", "stopped"}
Results == {"none", "fresh", "capture_failed", "action_pending", "action_uncertain"}
Init == /\ phase = "disconnected" /\ lastResult = "none" /\ resources = "none" /\ unresolved = FALSE /\ lateApplied = FALSE /\ durable = FALSE
Connect == /\ phase = "disconnected" /\ phase' = "connecting" /\ resources' = "owned" /\ UNCHANGED <<lastResult, unresolved, lateApplied, durable>>
Connected == /\ phase = "connecting" /\ phase' = (IF unresolved THEN "uncertain" ELSE "needs_observation") /\ lastResult' = (IF unresolved THEN "action_uncertain" ELSE "none") /\ UNCHANGED <<resources, unresolved, lateApplied, durable>>
ConnectFailed == /\ phase = "connecting" /\ phase' = "disconnected" /\ lastResult' = "none" /\ resources' = "none" /\ UNCHANGED <<unresolved, lateApplied, durable>>
Observe == /\ phase \in {"needs_observation", "ready"} /\ ~unresolved /\ phase' = "observing" /\ lastResult' = "none" /\ UNCHANGED <<resources, unresolved, lateApplied, durable>>
ObserveUncertain == /\ phase = "uncertain" /\ phase' = "observing_uncertain" /\ lastResult' = "none" /\ UNCHANGED <<resources, unresolved, lateApplied, durable>>
ObserveSuccess == /\ phase = "observing" /\ phase' = "ready" /\ lastResult' = "fresh" /\ UNCHANGED <<resources, unresolved, lateApplied, durable>>
ObserveFailure == /\ phase = "observing" /\ phase' = "needs_observation" /\ lastResult' = "capture_failed" /\ UNCHANGED <<resources, unresolved, lateApplied, durable>>
UncertainObservationDone == /\ phase = "observing_uncertain" /\ phase' = "uncertain" /\ lastResult' = "action_uncertain" /\ UNCHANGED <<resources, unresolved, lateApplied, durable>>
HeartbeatFailed == /\ phase \in {"ready", "needs_observation"} /\ phase' = "stale" /\ lastResult' = "none" /\ UNCHANGED <<resources, unresolved, lateApplied, durable>>
ObserveStale == /\ phase = "stale" /\ UNCHANGED vars
CheckConnection == /\ phase = "ready" /\ ~unresolved /\ phase' = "checking" /\ lastResult' = "none" /\ UNCHANGED <<resources, unresolved, lateApplied, durable>>
CheckFailed == /\ phase = "checking" /\ phase' = "needs_observation" /\ lastResult' = "none" /\ UNCHANGED <<resources, unresolved, lateApplied, durable>>
Act == /\ phase = "checking" /\ ~unresolved /\ phase' = "acting" /\ lastResult' = "action_pending" /\ lateApplied' = FALSE /\ durable' = TRUE /\ UNCHANGED <<resources, unresolved>>
ActionAcknowledged == /\ phase = "acting" /\ phase' = "needs_observation" /\ lastResult' = "none" /\ durable' = FALSE /\ UNCHANGED <<resources, unresolved, lateApplied>>
ActionTimeout == /\ phase = "acting" /\ phase' = "uncertain" /\ lastResult' = "action_uncertain" /\ unresolved' = TRUE /\ UNCHANGED <<resources, lateApplied, durable>>
LateDelivery == /\ unresolved /\ ~lateApplied /\ lateApplied' = TRUE /\ UNCHANGED <<phase, lastResult, resources, unresolved, durable>>
DeliverySettled == /\ phase = "uncertain" /\ unresolved /\ phase' = "needs_observation" /\ lastResult' = "none" /\ unresolved' = FALSE /\ durable' = FALSE /\ UNCHANGED <<resources, lateApplied>>
Disconnect == /\ phase \in {"connecting", "needs_observation", "observing", "ready", "stale", "checking", "acting", "uncertain", "observing_uncertain"} /\ phase' = "closing" /\ lastResult' = "none" /\ unresolved' = (unresolved \/ phase = "acting") /\ UNCHANGED <<resources, lateApplied, durable>>
Closed == /\ phase = "closing" /\ phase' = "disconnected" /\ lastResult' = "none" /\ resources' = "none" /\ UNCHANGED <<unresolved, lateApplied, durable>>
Shutdown == /\ phase \in {"connecting", "needs_observation", "observing", "ready", "stale", "checking", "acting", "uncertain", "observing_uncertain", "closing"} /\ phase' = "shutdown_closing" /\ lastResult' = "none" /\ unresolved' = (unresolved \/ phase = "acting") /\ UNCHANGED <<resources, lateApplied, durable>>
ShutdownClosed == /\ phase = "shutdown_closing" /\ phase' = "stopped" /\ lastResult' = "none" /\ resources' = "none" /\ UNCHANGED <<unresolved, lateApplied, durable>>
Stop == /\ phase = "disconnected" /\ phase' = "stopped" /\ lastResult' = "none" /\ UNCHANGED <<resources, unresolved, lateApplied, durable>>
Done == /\ phase = "stopped" /\ UNCHANGED vars
Crash == /\ phase \notin {"disconnected", "stopped"} /\ phase' = "disconnected" /\ resources' = "none" /\ lastResult' = "none" /\ unresolved' = (unresolved \/ durable) /\ UNCHANGED <<lateApplied, durable>>
CrashKeepsUncertainty == durable /\ phase = "disconnected" => unresolved
Next == Crash \/ Connect \/ Connected \/ ConnectFailed \/ Observe \/ ObserveUncertain \/ ObserveSuccess \/ ObserveFailure \/ UncertainObservationDone \/ HeartbeatFailed \/ ObserveStale \/ CheckConnection \/ CheckFailed \/ Act \/ ActionAcknowledged \/ ActionTimeout \/ LateDelivery \/ DeliverySettled \/ Disconnect \/ Closed \/ Shutdown \/ ShutdownClosed \/ Stop \/ Done
Spec == Init /\ [][Next]_vars
TypeOK == /\ phase \in Phases /\ lastResult \in Results /\ resources \in {"none", "owned"} /\ unresolved \in BOOLEAN /\ lateApplied \in BOOLEAN /\ durable \in BOOLEAN
ReadyHasFreshImage == phase = "ready" => lastResult = "fresh" /\ ~unresolved
NoPendingMarkerBeforeSend == phase = "checking" => ~durable /\ ~unresolved
NoActionWhileUnresolved == phase = "acting" => ~unresolved
UncertaintyIsVisible == phase = "uncertain" => lastResult = "action_uncertain" /\ unresolved
NoOldImageAfterFailure == lastResult = "capture_failed" => phase = "needs_observation"
StoppedIsClean == phase \in {"disconnected", "stopped"} => resources = "none"
=============================================================================
