export const title = 'Credential storage choice';
export const state = v => v.local === 'ready' ? 'Private local file configured' : v.choice === 'system' ? 'System credential store' : v.verified ? 'TV verified for local file' : 'Local file selected';
const steps = {ChooseLocal:'Choose unencrypted local file',Verify:'Verify the TV',Save:'Save local file',Fail:'Verification or save fails',Remove:'Remove local file',Wait:'Wait'};
export const step = a => steps[a] ?? a;
