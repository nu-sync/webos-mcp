export const title = 'Observe, act once, and recover';
const labels = {
  needs_view: 'New observation needed', capturing: 'Capturing screen', ready: 'Current observation available',
  checking_token: 'Checking observation identity', sending: 'Sending action once', capture_failed: 'Capture unavailable', recovering: 'Recovering capture',
  uncertain: 'Action delivery unresolved', closing: 'Closing connections',
  cleanup_unconfirmed: 'Cleanup unconfirmed', disconnected: 'Disconnected; cleanup confirmed',
};
export const state = v => labels[v.phase];
const labelsForStep = {
  Observe: 'Request observation', Frame: 'New frame returned; UI settling unknown',
  CaptureFailed: 'Capture failed; action is not repeated', Recover: 'Retry observation only',
  RecoveryOK: 'Capture connection restored', RecoveryFailed: 'Capture recovery failed',
  RequestAction: 'Submit action with observation identity', Send: 'Send using matching observation',
  RejectStaleToken: 'Reject stale observation; no send', SendComplete: 'Send completes; capture result',
  SendUnknown: 'Delivery uncertain', Close: 'Disconnect or cancellation',
  CloseConfirmed: 'Connection cleanup confirmed', CloseFailed: 'Cleanup fails or is interrupted',
  RetryClose: 'Retry owned cleanup', Reconnect: 'Reconnect with a new observation generation', Done: 'Remain stopped',
};
export const step = (action, from) => {
  if (action === 'Close' && !['ready', 'uncertain', 'capture_failed'].includes(from.phase)) return null;
  return labelsForStep[action] ?? action;
};
