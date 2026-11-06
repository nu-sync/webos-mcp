------------------------ MODULE CredentialChoice ------------------------
EXTENDS TLC
VARIABLES choice, local, verified
vars == <<choice, local, verified>>
Choices == {"system", "local"}
LocalStates == {"absent", "ready"}
Init == /\ choice = "system" /\ local = "absent" /\ verified = FALSE
ChooseLocal == /\ choice = "system" /\ choice' = "local" /\ UNCHANGED <<local, verified>>
Verify == /\ choice = "local" /\ local = "absent" /\ verified' = TRUE /\ UNCHANGED <<choice, local>>
Save == /\ choice = "local" /\ local = "absent" /\ verified /\ local' = "ready" /\ UNCHANGED <<choice, verified>>
Fail == /\ choice = "local" /\ local = "absent" /\ verified' = FALSE /\ UNCHANGED <<choice, local>>
Remove == /\ choice = "local" /\ local = "ready" /\ local' = "absent" /\ verified' = FALSE /\ UNCHANGED choice
Wait == UNCHANGED vars
Next == ChooseLocal \/ Verify \/ Save \/ Fail \/ Remove \/ Wait
Spec == Init /\ [][Next]_vars
TypeOK == /\ choice \in Choices /\ local \in LocalStates /\ verified \in BOOLEAN
LocalNeedsConsent == local = "ready" => choice = "local"
LocalNeedsVerification == local = "ready" => verified
=============================================================================
