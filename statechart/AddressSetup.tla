-------------------------- MODULE AddressSetup --------------------------
EXTENDS TLC
VARIABLES phase, saved, candidate, exposed, trusted, lastCommitTrusted
vars == <<phase, saved, candidate, exposed, trusted, lastCommitTrusted>>
Phases == {"empty", "entering", "verifying", "saving", "saved", "unavailable"}
Addresses == {"none", "old", "new"}
Init == /\ phase = "empty" /\ saved = "none" /\ candidate = "none" /\ exposed = FALSE /\ trusted = FALSE /\ lastCommitTrusted = TRUE
Enter == /\ phase \in {"empty", "saved", "unavailable"} /\ phase' = "entering" /\ candidate' = "none" /\ trusted' = FALSE /\ UNCHANGED <<saved, exposed, lastCommitTrusted>>
Submit == /\ phase = "entering" /\ phase' = "verifying" /\ candidate' = (IF saved = "none" THEN "old" ELSE "new") /\ UNCHANGED <<saved, exposed, trusted, lastCommitTrusted>>
Verify == /\ phase = "verifying" /\ phase' = "saving" /\ trusted' = TRUE /\ UNCHANGED <<saved, candidate, exposed, lastCommitTrusted>>
Reject == /\ phase = "verifying" /\ phase' = (IF saved = "none" THEN "empty" ELSE "saved") /\ candidate' = "none" /\ trusted' = FALSE /\ UNCHANGED <<saved, exposed, lastCommitTrusted>>
Commit == /\ phase = "saving" /\ phase' = "saved" /\ saved' = candidate /\ candidate' = "none" /\ lastCommitTrusted' = trusted /\ trusted' = FALSE /\ UNCHANGED exposed
StoreFailed == /\ phase = "saving" /\ phase' = "unavailable" /\ candidate' = "none" /\ trusted' = FALSE /\ UNCHANGED <<saved, exposed, lastCommitTrusted>>
StoreUnconfirmed == /\ phase = "saving" /\ phase' = "unavailable" /\ saved' \in {saved, candidate} /\ candidate' = "none" /\ trusted' = FALSE /\ lastCommitTrusted' = (IF saved' = candidate THEN trusted ELSE lastCommitTrusted) /\ UNCHANGED exposed
Interrupt == /\ phase \in {"entering", "verifying", "saving"} /\ phase' = (IF saved = "none" THEN "empty" ELSE "saved") /\ candidate' = "none" /\ trusted' = FALSE /\ UNCHANGED <<saved, exposed, lastCommitTrusted>>
Forget == /\ phase = "saved" /\ phase' = "empty" /\ saved' = "none" /\ candidate' = "none" /\ UNCHANGED <<exposed, trusted, lastCommitTrusted>>
Wait == /\ phase \in {"empty", "saved", "unavailable"} /\ UNCHANGED vars
Next == Enter \/ Submit \/ Verify \/ Reject \/ Commit \/ StoreFailed \/ StoreUnconfirmed \/ Interrupt \/ Forget \/ Wait
Spec == Init /\ [][Next]_vars
TypeOK == /\ phase \in Phases /\ saved \in Addresses /\ candidate \in Addresses /\ exposed \in BOOLEAN /\ trusted \in BOOLEAN /\ lastCommitTrusted \in BOOLEAN
AddressNotExposed == ~exposed
SavedStateHasAddress == phase = "saved" => saved # "none"
NoPendingOutsideSave == phase \notin {"saving", "verifying"} => candidate = "none"
SavingRequiresTrustedTV == phase = "saving" => trusted
CommitRequiresTrustedTV == lastCommitTrusted
=============================================================================
