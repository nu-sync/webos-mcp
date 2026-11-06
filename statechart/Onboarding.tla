---------------------------- MODULE Onboarding -----------------------------
EXTENDS TLC
VARIABLES phase, credential, trusted, capability
vars == <<phase, credential, trusted, capability>>
Phases == {"idle", "verifying", "reauthorizing", "pairing", "saving", "probing", "ready", "capture_unavailable", "repair"}
Credentials == {"none", "valid", "revoked"}
Capabilities == {"unknown", "available", "unavailable"}
Init == /\ phase = "idle" /\ credential = "none" /\ trusted = FALSE /\ capability = "unknown"
Start == /\ phase \in {"idle", "repair"} /\ phase' = "verifying" /\ trusted' = FALSE /\ capability' = "unknown" /\ UNCHANGED credential
VerifyIdentity == /\ phase = "verifying" /\ phase' = (IF credential = "valid" THEN "reauthorizing" ELSE "pairing") /\ trusted' = TRUE /\ UNCHANGED <<credential, capability>>
RejectIdentity == /\ phase = "verifying" /\ phase' = "idle" /\ trusted' = FALSE /\ UNCHANGED <<credential, capability>>
ReauthorizeSuccess == /\ phase = "reauthorizing" /\ phase' = "probing" /\ UNCHANGED <<credential, trusted, capability>>
ReauthorizeRejected == /\ phase = "reauthorizing" /\ phase' = "repair" /\ credential' = "revoked" /\ trusted' = FALSE /\ capability' = "unknown"
PairApproved == /\ phase = "pairing" /\ phase' = "saving" /\ UNCHANGED <<credential, trusted, capability>>
PairRejected == /\ phase = "pairing" /\ phase' = "idle" /\ trusted' = FALSE /\ UNCHANGED <<credential, capability>>
StoreSuccess == /\ phase = "saving" /\ phase' = "probing" /\ credential' = "valid" /\ UNCHANGED <<trusted, capability>>
StoreUnavailable == /\ phase = "saving" /\ phase' = "idle" /\ trusted' = FALSE /\ UNCHANGED <<credential, capability>>
CaptureAvailable == /\ phase = "probing" /\ phase' = "ready" /\ capability' = "available" /\ UNCHANGED <<credential, trusted>>
CaptureUnavailable == /\ phase = "probing" /\ phase' = "capture_unavailable" /\ capability' = "unavailable" /\ UNCHANGED <<credential, trusted>>
Interrupted == /\ phase \in {"verifying", "reauthorizing", "pairing", "saving", "probing"} /\ phase' = "idle" /\ trusted' = FALSE /\ capability' = "unknown" /\ UNCHANGED credential
Revoke == /\ phase \in {"ready", "capture_unavailable"} /\ phase' = "repair" /\ credential' = "revoked" /\ trusted' = FALSE /\ capability' = "unknown"
Remove == /\ phase \in {"idle", "ready", "capture_unavailable", "repair"} /\ phase' = "idle" /\ credential' = "none" /\ trusted' = FALSE /\ capability' = "unknown"
Stay == /\ phase \in {"idle", "ready", "capture_unavailable", "repair"} /\ UNCHANGED vars
Next == Start \/ VerifyIdentity \/ RejectIdentity \/ ReauthorizeSuccess \/ ReauthorizeRejected \/ PairApproved \/ PairRejected \/ StoreSuccess \/ StoreUnavailable \/ CaptureAvailable \/ CaptureUnavailable \/ Interrupted \/ Revoke \/ Remove \/ Stay
Spec == Init /\ [][Next]_vars
TypeOK == /\ phase \in Phases /\ credential \in Credentials /\ trusted \in BOOLEAN /\ capability \in Capabilities
ReadyRequiresTrustedCredentials == phase \in {"ready", "capture_unavailable"} => trusted /\ credential = "valid"
VisualReadyRequiresCapability == phase = "ready" => capability = "available"
NoVisualClaimWithoutCapture == capability = "unavailable" => phase = "capture_unavailable"
NoUntrustedPairOrProbe == phase \in {"reauthorizing", "pairing", "saving", "probing"} => trusted
RevokedCannotBeReady == credential = "revoked" => phase \notin {"ready", "capture_unavailable", "probing"}
=============================================================================
