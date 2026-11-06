---------------------------- MODULE Observation ----------------------------
EXTENDS Naturals, TLC
VARIABLES phase, generation, revision, fresh, accepted, delivery, sends,
          observation, cleanup, supplied
vars == <<phase, generation, revision, fresh, accepted, delivery, sends,
          observation, cleanup, supplied>>
NoToken == <<2, 0>>
CurrentToken == <<generation, revision>>
Phases == {"needs_view", "capturing", "ready", "checking_token", "sending", "capture_failed",
           "recovering", "uncertain", "closing", "cleanup_unconfirmed", "disconnected"}
Init == /\ phase = "needs_view" /\ generation = 0 /\ revision = 0
        /\ fresh = FALSE /\ accepted = NoToken /\ delivery = "none" /\ sends = 0
        /\ observation = "none" /\ cleanup = "owned" /\ supplied = NoToken
Observe == /\ phase \in {"needs_view", "ready", "uncertain"} /\ revision < 3
           /\ phase' = "capturing" /\ fresh' = FALSE /\ observation' = "none"
           /\ UNCHANGED <<generation, revision, accepted, delivery, sends, cleanup, supplied>>
Frame == /\ phase = "capturing" /\ revision < 3
         /\ phase' = (IF delivery = "unknown" THEN "uncertain" ELSE "ready")
         /\ revision' = revision + 1 /\ fresh' = (delivery # "unknown")
         /\ observation' = "frame"
         /\ UNCHANGED <<generation, accepted, delivery, sends, cleanup, supplied>>
CaptureFailed == /\ phase = "capturing" /\ phase' = "capture_failed"
                 /\ fresh' = FALSE /\ observation' = "failed"
                 /\ UNCHANGED <<generation, revision, accepted, delivery, sends, cleanup, supplied>>
Recover == /\ phase = "capture_failed" /\ phase' = "recovering"
           /\ UNCHANGED <<generation, revision, fresh, accepted, delivery, sends, observation, cleanup, supplied>>
RecoveryOK == /\ phase = "recovering"
              /\ phase' = (IF delivery = "unknown" THEN "uncertain" ELSE "needs_view")
              /\ observation' = "none"
              /\ UNCHANGED <<generation, revision, fresh, accepted, delivery, sends, cleanup, supplied>>
RecoveryFailed == /\ phase = "recovering" /\ phase' = "capture_failed"
                  /\ UNCHANGED <<generation, revision, fresh, accepted, delivery, sends, observation, cleanup, supplied>>
RequestTokens == {CurrentToken}
                 \cup (IF revision > 1 THEN {<<generation, revision - 1>>} ELSE {})
                 \cup (IF generation = 1 THEN {<<0, revision>>} ELSE {})
RequestAction == /\ phase = "ready" /\ fresh /\ revision < 3 /\ sends = 0
                 /\ phase' = "checking_token" /\ supplied' \in RequestTokens
                 /\ UNCHANGED <<generation, revision, fresh, accepted, delivery, sends, observation, cleanup>>
Send == /\ phase = "checking_token" /\ supplied = CurrentToken
        /\ fresh /\ sends = 0 /\ delivery = "none" /\ phase' = "sending"
        /\ accepted' = supplied /\ supplied' = NoToken
        /\ fresh' = FALSE /\ sends' = sends + 1 /\ observation' = "none"
        /\ UNCHANGED <<generation, revision, delivery, cleanup>>
RejectStaleToken == /\ phase = "checking_token" /\ supplied # CurrentToken
                    /\ phase' = "ready" /\ supplied' = NoToken
                    /\ UNCHANGED <<generation, revision, fresh, accepted, delivery, sends, observation, cleanup>>
SendComplete == /\ phase = "sending" /\ phase' = "capturing" /\ delivery' = "sent"
                /\ UNCHANGED <<generation, revision, fresh, accepted, sends, observation, cleanup, supplied>>
SendUnknown == /\ phase = "sending" /\ phase' = "uncertain" /\ delivery' = "unknown"
               /\ UNCHANGED <<generation, revision, fresh, accepted, sends, observation, cleanup, supplied>>
Close == /\ phase \in {"needs_view", "capturing", "ready", "checking_token", "sending", "capture_failed", "recovering", "uncertain"}
         /\ phase' = "closing" /\ fresh' = FALSE /\ cleanup' = "pending" /\ supplied' = NoToken
         /\ delivery' = (IF phase = "sending" THEN "unknown" ELSE delivery)
         /\ UNCHANGED <<generation, revision, accepted, sends, observation>>
CloseConfirmed == /\ phase = "closing" /\ phase' = "disconnected" /\ cleanup' = "confirmed"
                  /\ UNCHANGED <<generation, revision, fresh, accepted, delivery, sends, observation, supplied>>
CloseFailed == /\ phase = "closing" /\ phase' = "cleanup_unconfirmed" /\ cleanup' = "unknown"
               /\ UNCHANGED <<generation, revision, fresh, accepted, delivery, sends, observation, supplied>>
RetryClose == /\ phase = "cleanup_unconfirmed" /\ phase' = "closing" /\ cleanup' = "pending"
              /\ UNCHANGED <<generation, revision, fresh, accepted, delivery, sends, observation, supplied>>
Reconnect == /\ phase = "disconnected" /\ generation = 0
             /\ generation' = 1 /\ revision' = 0 /\ cleanup' = "owned"
             /\ phase' = (IF delivery = "unknown" THEN "uncertain" ELSE "needs_view")
             /\ observation' = "none" /\ fresh' = FALSE
             /\ UNCHANGED <<accepted, delivery, sends, supplied>>
Done == /\ phase \in {"disconnected", "cleanup_unconfirmed"} /\ UNCHANGED vars
Next == Observe \/ Frame \/ CaptureFailed \/ Recover \/ RecoveryOK \/ RecoveryFailed
        \/ RequestAction \/ Send \/ RejectStaleToken \/ SendComplete \/ SendUnknown
        \/ Close \/ CloseConfirmed \/ CloseFailed \/ RetryClose \/ Reconnect \/ Done
Spec == Init /\ [][Next]_vars
TypeOK == /\ phase \in Phases /\ generation \in 0..1 /\ revision \in 0..3
          /\ fresh \in BOOLEAN /\ accepted \in ({0, 1, 2} \X (0..3))
          /\ supplied \in ({0, 1, 2} \X (0..3))
          /\ delivery \in {"none", "sent", "unknown"} /\ sends \in 0..2
          /\ observation \in {"none", "frame", "failed"}
          /\ cleanup \in {"owned", "pending", "unknown", "confirmed"}
ActionUsesCurrentObservation == phase = "sending" => accepted = CurrentToken
AtMostOneSendPerCall == sends <= 1
RecoveryPreservesUncertainty == delivery = "unknown" => ~fresh /\ phase # "sending"
ReadyRequiresFreshFrame == phase = "ready" => fresh /\ observation = "frame" /\ delivery # "unknown"
FailureInvalidatesObservation == phase = "capture_failed" => ~fresh /\ observation = "failed"
CleanDisconnectIsConfirmed == phase = "disconnected" => cleanup = "confirmed"
CleanupBlocksActions == cleanup \in {"pending", "unknown", "confirmed"} => ~fresh /\ phase # "sending"
=============================================================================
