export const title = 'TV onboarding';
const labels = {idle:'Setup not active',verifying:'Confirm TV identity',reauthorizing:'Check saved pairing',pairing:'Await TV pairing',saving:'Save pairing securely',probing:'Check visual capability',ready:'Ready for visual control',capture_unavailable:'Connected without visual capture',repair:'Pairing needs repair'};
export const state = v => labels[v.phase];
const steps = {Start:'Start setup',VerifyIdentity:'Identity verified',RejectIdentity:'Identity rejected',ReauthorizeSuccess:'Saved pairing accepted',ReauthorizeRejected:'Saved pairing rejected',PairApproved:'Pairing approved',PairRejected:'Pairing rejected',StoreSuccess:'Credential saved',StoreUnavailable:'Secure store unavailable',CaptureAvailable:'Capture supported',CaptureUnavailable:'Capture unavailable',Interrupted:'Setup interrupted',Revoke:'Pairing revoked',Remove:'Remove local device',Stay:'Wait'};
export const step = a => steps[a] ?? a;
