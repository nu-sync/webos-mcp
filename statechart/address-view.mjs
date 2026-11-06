export const title = 'Private TV address setup';
const labels = {empty:'No address saved',entering:'Enter TV address',verifying:'Verifying the TV',saving:'Saving securely',saved:'Address saved',unavailable:'Secure store unavailable'};
export const state = v => labels[v.phase];
const names = {Enter:'Enter or change address',Submit:'Submit address',Verify:'TV identity verified',Reject:'Identity rejected',Commit:'Secure save succeeds',StoreFailed:'Secure save fails',StoreUnconfirmed:'Secure save could not be verified',Interrupt:'Setup interrupted',Forget:'Forget saved address',Wait:'Wait'};
export const step = a => names[a] ?? a;
