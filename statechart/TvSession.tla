----------------------------- MODULE TvSession -----------------------------
EXTENDS TLC
VARIABLES phase, lastResult, resources
vars == <<phase, lastResult, resources>>
Phases == {"disconnected", "connecting", "needs_observation", "observing", "ready", "acting", "uncertain", "closing", "shutdown_closing", "stopped"}
Results == {"none", "fresh", "capture_failed", "action_pending", "action_uncertain"}
Init == /\ phase = "disconnected" /\ lastResult = "none" /\ resources = "none"
Connect == /\ phase = "disconnected" /\ phase' = "connecting" /\ UNCHANGED lastResult /\ resources' = "owned"
Connected == /\ phase = "connecting" /\ phase' = "needs_observation" /\ lastResult' = "none" /\ UNCHANGED resources
ConnectFailed == /\ phase = "connecting" /\ phase' = "disconnected" /\ lastResult' = "none" /\ resources' = "none"
Observe == /\ phase \in {"needs_observation", "ready", "uncertain"} /\ phase' = "observing" /\ lastResult' = "none" /\ UNCHANGED resources
ObserveSuccess == /\ phase = "observing" /\ phase' = "ready" /\ lastResult' = "fresh" /\ UNCHANGED resources
ObserveFailure == /\ phase = "observing" /\ phase' = "needs_observation" /\ lastResult' = "capture_failed" /\ UNCHANGED resources
Act == /\ phase = "ready" /\ phase' = "acting" /\ lastResult' = "action_pending" /\ UNCHANGED resources
ActionAcknowledged == /\ phase = "acting" /\ phase' = "needs_observation" /\ lastResult' = "none" /\ UNCHANGED resources
ActionTimeout == /\ phase = "acting" /\ phase' = "uncertain" /\ lastResult' = "action_uncertain" /\ UNCHANGED resources
Disconnect == /\ phase \in {"connecting", "needs_observation", "observing", "ready", "acting", "uncertain"} /\ phase' = "closing" /\ lastResult' = "none" /\ UNCHANGED resources
Closed == /\ phase = "closing" /\ phase' = "disconnected" /\ lastResult' = "none" /\ resources' = "none"
Shutdown == /\ phase \in {"connecting", "needs_observation", "observing", "ready", "acting", "uncertain", "closing"} /\ phase' = "shutdown_closing" /\ lastResult' = "none" /\ UNCHANGED resources
ShutdownClosed == /\ phase = "shutdown_closing" /\ phase' = "stopped" /\ lastResult' = "none" /\ resources' = "none"
Stop == /\ phase = "disconnected" /\ phase' = "stopped" /\ lastResult' = "none" /\ UNCHANGED resources
Done == /\ phase = "stopped" /\ UNCHANGED vars
Next == Connect \/ Connected \/ ConnectFailed \/ Observe \/ ObserveSuccess \/ ObserveFailure \/ Act \/ ActionAcknowledged \/ ActionTimeout \/ Disconnect \/ Closed \/ Shutdown \/ ShutdownClosed \/ Stop \/ Done
Spec == Init /\ [][Next]_vars
TypeOK == /\ phase \in Phases /\ lastResult \in Results /\ resources \in {"none", "owned"}
ReadyHasFreshImage == phase = "ready" => lastResult = "fresh"
UncertaintyIsVisible == phase = "uncertain" => lastResult = "action_uncertain"
NoOldImageAfterFailure == lastResult = "capture_failed" => phase = "needs_observation"
ActionRequiresObservation == phase = "acting" => lastResult = "action_pending"
StoppedIsClean == phase \in {"disconnected", "stopped"} => resources = "none"
=============================================================================
