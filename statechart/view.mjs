export const title = 'TV control session';
const labels = {disconnected:'TV disconnected', connecting:'Connecting to TV', needs_observation:'Fresh view needed', observing:'Capturing screen', ready:'Ready with fresh view', stale:'TV connection stale', checking:'Checking TV connection', acting:'Sending action', uncertain:'Action delivery unresolved', observing_uncertain:'Inspecting unresolved action', closing:'Closing TV session', shutdown_closing:'Stopping and closing TV session', stopped:'MCP server stopped'};
export const state = v => labels[v.phase];
const steps = {Crash:'Server stops unexpectedly', Connect:'Connect requested', Connected:'TV connection ready', ConnectFailed:'Connection failed', Observe:'Request screenshot', ObserveUncertain:'Inspect screen', ObserveSuccess:'Fresh screenshot received', ObserveFailure:'Screenshot failed', UncertainObservationDone:'Delivery still unresolved', HeartbeatFailed:'Keepalive fails', ObserveStale:'Inspect screen', CheckConnection:'Check connection before action', CheckFailed:'Connection check failed', Act:'Send action', ActionAcknowledged:'Send completes [TV receipt unconfirmed]', ActionTimeout:'Action timed out', LateDelivery:'Delayed action arrives', DeliverySettled:'Delivery resolved by trusted evidence [not implemented]', Disconnect:'Disconnect requested', Closed:'Connections closed', Shutdown:'MCP server stops', ShutdownClosed:'Connections closed', Stop:'MCP server stops', Done:'Remain stopped'};
export const step = (a, from) => {
  if (a === 'Crash' && !['acting', 'uncertain'].includes(from.phase)) return null;
  if (a === 'Shutdown' && from.phase !== 'closing') return null;
  if (a === 'Disconnect' && ['connecting', 'observing', 'checking', 'observing_uncertain'].includes(from.phase)) return null;
  return steps[a] ?? a;
};
