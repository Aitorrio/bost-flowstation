#!/usr/bin/env node
// Cached dashboard panel regressions. Run with: node contrib/test-dashboard-language-panels.cjs
const fs=require('node:fs'),path=require('node:path'),vm=require('node:vm'),assert=require('node:assert/strict');
const source=fs.readFileSync(path.join(__dirname,'../crates/tetra-entities/src/net_dashboard/html.rs'),'utf8');
function fn(name){const match=source.match(new RegExp('(?:async )?function '+name+'\\([^]*?\\n\\}'));assert(match,name);return match[0];}
function node(){return {textContent:'',innerHTML:'',value:'typed',placeholder:'old',checked:true,dataset:{},options:[],style:{},className:'',children:[],classList:{toggle(){},add(){},remove(){},contains(){return false;}},setAttribute(){},appendChild(v){this.children.push(v);}};}
let count=0,pending=[];function test(name,run){const result=run();if(result&&typeof result.then==='function'){pending.push(result.then(()=>{count++;console.log('PASS '+name);}));}else{count++;console.log('PASS '+name);}}
const forbidden=()=>{throw Error('Operational action called during translation refresh');};
function ctx(extra={}){return vm.createContext({console,t:key=>'fr:'+key,document:{getElementById:()=>null},setTimeout:forbidden,setInterval:forbidden,fetch:forbidden,...extra});}
test('LST label refresh keeps timer, modal, tab, peer and dispatch unchanged',()=>{
 const c=ctx({lstEnsureTimerTick:forbidden,openLstCallModal:forbidden,lstCallSetTab:forbidden,lstPhaseLabel:p=>'fr:'+p,lstCauseLabel:()=>'',lstFormatTimer:()=>'',lstCallElapsedSecs:()=>0,lstGssiLabel:String,lstToken:'token',lstCallPeer:7,lstCallTab:'sx',paintIcons:()=>{}});
 vm.runInContext(fn('lstUpdateCallUi'),c);vm.runInContext("lstUpdateCallUi({call_phase:'ringing',call_kind:'duplex',call_peer:42,call_inbound:true},{labelsOnly:true})",c);
 assert.equal(c.lstCallPeer,7);assert.equal(c.lstCallTab,'sx');
});
test('SDR label refresh does not reset telemetry age',()=>{
 const c=ctx({rfState:{lastHwTs:123},displayedSdrHealthCache:{old:true},setText:forbidden,renderGainList:()=>{}});
 vm.runInContext(fn('handleSdrHealth'),c);vm.runInContext('handleSdrHealth({temperature_c:50},{labelsOnly:true})',c);
 assert.equal(c.rfState.lastHwTs,123);assert.equal(c.displayedSdrHealthCache.old,true);
});
test('Setup refresh preserves typed wizard device and selected device',()=>{
 const input=node(),selected='user-selected';input.value='driver=typed';
 const c=ctx({document:{getElementById:id=>id==='wiz-device'?input:null},setupSelectedDevice:selected,paintRfPill:()=>{},renderDeviceList:()=>{},updateRfStatusBanner:()=>{}});
 vm.runInContext(fn('renderSetupStatus'),c);vm.runInContext("renderSetupStatus({config_device:'driver=server',devices:[]})",c);
 assert.equal(input.value,'driver=typed');assert.equal(c.setupSelectedDevice,selected);
});
test('Security refresh preserves key fields, selected algorithm and dirty flags',()=>{
 const fields=new Map(['secp-running','secp-saved','secp-msg','secp-sck','secp-ksg'].map(id=>[id,node()]));const select=fields.get('secp-ksg');select.value='tea3';select.options=[{value:'tea3',textContent:'old'}];fields.get('secp-msg').dataset.translationKey='secp_unsaved';
 const c=ctx({document:{getElementById:id=>fields.get(id)||null},secpData:{running:{invalid_subscriber_keys:[]},saved:null,ksgs:[{id:'tea3',name:'TEA3',available:true}]},secpChanged:true,secpSckDirty:true,secpSubs:[{issi:1,k:'secret'}],secpPosture:()=>'<span>fr:posture</span>',secpRenderSubs:()=>{},secpRenderAlgos:()=>{}});
 vm.runInContext(fn('renderSecurityLabels'),c);vm.runInContext('renderSecurityLabels()',c);
 assert.equal(fields.get('secp-sck').value,'typed');assert.equal(select.value,'tea3');assert(c.secpChanged);assert(c.secpSckDirty);assert.equal(c.secpSubs[0].k,'secret');assert.equal(fields.get('secp-msg').textContent,'fr:secp_unsaved');
});
test('Profile list repaints translated labels without touching profile editor',()=>{
 const list=node(),edit=node();
 const c=ctx({document:{getElementById:id=>id==='profileList'?list:edit,createElement:()=>node()},openEditProfile:forbidden,activateProfile:forbidden});
 vm.runInContext(fn('renderConfigProfiles'),c);vm.runInContext("renderConfigProfiles([{name:'Default',active:false}])",c);
 assert.equal(edit.value,'typed');assert.equal(list.children[0].children[1].textContent,'fr:profile_edit_btn');assert.equal(list.children[0].children[2].textContent,'fr:sys_activate');
});
test('Live SDS list repaints text while keeping composed message',()=>{
 const list=node(),clear=node(),compose=node();
 const c=ctx({document:{getElementById:id=>id==='live-sds-list'?list:id==='live-sds-clear-btn'?clear:compose,createElement:()=>node()},escHtml:String});
 vm.runInContext(fn('renderLiveSds'),c);vm.runInContext("renderLiveSds([{id:1,text:'hello',repeat_count:0,sent_count:0,protocol_id:2,source_issi:99}])",c);
 assert.equal(compose.value,'typed');assert(list.children[0].innerHTML.includes('fr:live_sds_delete'));assert(list.children[0].innerHTML.includes('fr:live_sds_forever'));
});
test('Health-only repaint does not load integration endpoints',()=>{
 const c=ctx({loadHealthIntegrations:forbidden,renderHealthTab:()=>{}});
 vm.runInContext(fn('renderStationHealth'),c);vm.runInContext("renderStationHealth({overall:'ok'})",c);
});
test('Panel refresher isolates individual rendering failures and keeps other panels updating',()=>{
 const called=[];
 const c=ctx({console:{warn(){}},renderSystemInfo:()=>{throw Error('one panel failed');},systemProfilesCache:[],systemProfilesLoadFailed:false,liveSdsLoadFailed:false,renderConfigProfiles:()=>called.push('profiles'),liveSdsItemsCache:[],renderLiveSds:()=>called.push('sds'),dashboardAuthCache:null,setupStateCache:{},renderSetupStatus:()=>called.push('setup'),renderSecurityLabels:()=>called.push('security'),stationHealthCache:{},renderStationHealth:()=>called.push('health'),systemHealthCache:{},renderSystemHealth:()=>called.push('sensors'),renderHealthIntegrations:()=>called.push('integrations'),displayedSdrHealthCache:{},handleSdrHealth:(_,opts)=>{assert(opts.labelsOnly);called.push('rf');},lstLastStatus:{},lstUpdateCallUi:(_,opts)=>{assert(opts.labelsOnly);called.push('lst');},lstSyncPttButtonHint:()=>called.push('ptt-label'),lstGeoOpen:false});
 vm.runInContext(fn('refreshCachedPanelTranslations'),c);vm.runInContext('refreshCachedPanelTranslations()',c);
 assert.deepEqual(called,['profiles','sds','setup','security','health','sensors','integrations','rf','lst','ptt-label']);
});


test('DGNA language repaint preserves a draft query after focus moves to language control',()=>{
 const input=node(),menu=node(),wrap=node();input.value='268';
 const c=ctx({document:{activeElement:node(),getElementById:id=>id==='dgna-group-picker'?input:id==='dgna-group-picker-menu'?menu:wrap},dgnaUi:{selectedGssi:26891,groupQuery:'268',groupPickerOpen:true},dgnaLibraryGroups:()=>[{gssi:26891,mnemonic:'Tetra PT',attached_count:1,device_count:2}],escHtml:String,dgnaGroupPickerLabel:()=> '26891 Tetra PT'});
 vm.runInContext(fn('renderDgnaGroupPicker'),c);vm.runInContext('renderDgnaGroupPicker({labelsOnly:true})',c);
 assert.equal(input.value,'268');assert(menu.innerHTML.includes('26891'));assert.equal(c.dgnaUi.groupQuery,'268');
});
test('DGNA label-only page repaint does not normalize attachment values',()=>{
 let pickerOptions;
 const c=ctx({document:{getElementById:()=>node()},syncDgnaAttachmentModePicker:forbidden,renderDgnaLibrary:()=>{},renderDgnaGroupPicker:opts=>{pickerOptions=opts;},renderDgnaAssignmentSummary:()=>{},renderDgnaTargetsTable:()=>{},renderDgnaActivity:()=>{}});
 vm.runInContext(fn('renderDgnaPage'),c);vm.runInContext('renderDgnaPage({labelsOnly:true})',c);assert(pickerOptions.labelsOnly);
});
test('OTA label refresh preserves machine output, progress state, open details and timers',()=>{
 const fields=new Map(['update-progress-bar','update-progress-pct','update-phase','update-current-line','update-status-msg','update-log-toggle','ota-channel-hint','update-terminal','ota-tech-wrap','ota-step-progress','update-modal'].map(id=>[id,node()]));
 const terminal=fields.get('update-terminal');terminal.textContent='ERROR: HEAD contains 1 local commit';
 const tech=fields.get('ota-tech-wrap');tech.classList={contains:()=>true,remove:forbidden,add:forbidden,toggle:forbidden};
 const modal=fields.get('update-modal');modal.classList={contains:()=>true,remove:forbidden,add:forbidden,toggle:forbidden};
 const c=ctx({document:{getElementById:id=>fields.get(id)||null},otaLastCheck:null,otaPendingCheck:null,otaProgressSnapshot:null,otaChannelHintSnapshot:null,updateLogExpanded:true,updateStartedAt:12345,updatePollTimer:17,updateElapsedTimer:18,formatUpdateElapsed:()=>{},applyUpdateCheckUi:forbidden,paintOtaReview:forbidden});
 for(const name of ['tipForOtaPhase','setUpdateProgress','setOtaStatus','setOtaChannelHint','setOtaChannelHintKey','refreshOtaTranslations'])vm.runInContext(fn(name),c);
 vm.runInContext("setUpdateProgress(22,'update_phase_error','ERROR: HEAD contains 1 local commit',true,false);setOtaStatus('ERROR: raw reply');setOtaChannelHintKey('ota_check_failed')",c);
 const snapshot=c.otaProgressSnapshot;c.t=key=>'pt:'+key;vm.runInContext('refreshOtaTranslations()',c);
 assert.equal(fields.get('update-phase').textContent,'pt:update_phase_error');assert.equal(fields.get('update-current-line').textContent,'ERROR: HEAD contains 1 local commit');assert.equal(fields.get('update-status-msg').textContent,'ERROR: raw reply');assert.equal(terminal.textContent,'ERROR: HEAD contains 1 local commit');assert.equal(fields.get('update-log-toggle').textContent,'pt:update_hide_log');assert.equal(fields.get('ota-channel-hint').textContent,'pt:ota_check_failed');assert.equal(c.otaProgressSnapshot,snapshot);assert.equal(c.updateStartedAt,12345);assert.equal(c.updatePollTimer,17);assert.equal(c.updateElapsedTimer,18);assert(c.updateLogExpanded);
});
test('OTA friendly status re-translates without changing completed progress',()=>{
 const fields=new Map(['update-status-msg','update-current-line','update-phase','update-progress-pct','update-progress-bar'].map(id=>[id,node()]));
 const c=ctx({document:{getElementById:id=>fields.get(id)||null},otaLastCheck:null,otaPendingCheck:null,otaProgressSnapshot:null,otaChannelHintSnapshot:null,updateLogExpanded:false,updateStartedAt:0});
 for(const name of ['tipForOtaPhase','setUpdateProgress','setOtaStatus','refreshOtaTranslations'])vm.runInContext(fn(name),c);
 vm.runInContext("setOtaStatus(t('update_done_current'),'update_done_current');setUpdateProgress(100,'update_phase_done',t('update_done_current'),false,false)",c);c.t=key=>'pt:'+key;vm.runInContext('refreshOtaTranslations()',c);
 assert.equal(fields.get('update-status-msg').textContent,'pt:update_done_current');assert.equal(fields.get('update-current-line').textContent,'pt:update_done_current');assert.equal(fields.get('update-progress-pct').textContent,'100%');
});
test('OTA review label-only refresh keeps raw release notes and expanded technical details',()=>{
 const versions=node(),title=node(),notes=node(),list=node(),tech=node();notes.innerHTML='<p>Original notes</p>';list.innerHTML='Original commits';tech.classList={remove:forbidden,add:forbidden,toggle:forbidden};
 const c=ctx({document:{getElementById:id=>id==='ota-review-versions'?versions:id==='ota-notes-title'?title:id==='ota-notes'?notes:id==='ota-changelog-list'?list:id==='ota-tech-wrap'?tech:null},otaTargetLabel:()=> 'v0.5.5',escapeHtml:String,formatOtaNotesHtml:forbidden});
 vm.runInContext(fn('paintOtaReview'),c);vm.runInContext("paintOtaReview({current:'v0.5.4',release_notes:'backend source',changelog:[{sha:'abc',title:'raw commit'}],notes_source:'release'},{labelsOnly:true})",c);
 assert(title.textContent.includes('fr:ota_notes_from_release'));assert.equal(notes.innerHTML,'<p>Original notes</p>');assert.equal(list.innerHTML,'Original commits');
});


test('Profile fetch errors survive language repaint and a later successful response clears failure state',async()=>{
 const list=node();let rendered=0;
 const c=ctx({console:{warn(){}},document:{getElementById:()=>list},escHtml:String,systemProfilesCache:[{name:'old'}],systemProfilesLoadFailed:false,liveSdsItemsCache:null,liveSdsLoadFailed:false,renderConfigProfiles:()=>{rendered++;list.innerHTML='new successful rows';},renderSecurityLabels:()=>{},renderHealthIntegrations:()=>{},fetch:async()=>({ok:false,status:503})});
 for(const name of ['loadConfigProfiles','refreshCachedPanelTranslations'])vm.runInContext(fn(name),c);
 await vm.runInContext('loadConfigProfiles()',c);assert(c.systemProfilesLoadFailed);const failure=list.innerHTML;
 vm.runInContext('refreshCachedPanelTranslations()',c);assert.equal(list.innerHTML,failure);assert.equal(rendered,0);
 c.fetch=async()=>{throw Error('raw connection failure');};await vm.runInContext('loadConfigProfiles()',c);assert(c.systemProfilesLoadFailed);assert(list.innerHTML.includes('raw connection failure'));
 c.fetch=async()=>({ok:true,json:async()=>[{name:'fresh'}]});await vm.runInContext('loadConfigProfiles()',c);assert.equal(c.systemProfilesLoadFailed,false);assert.equal(c.systemProfilesCache[0].name,'fresh');assert.equal(rendered,1);vm.runInContext('refreshCachedPanelTranslations()',c);assert.equal(rendered,2);
});
test('Live SDS fetch errors survive language repaint and successful refresh resumes translated rows',async()=>{
 const list=node();let rendered=0;
 const c=ctx({console:{warn(){}},document:{getElementById:()=>list},escHtml:String,systemProfilesCache:null,systemProfilesLoadFailed:false,liveSdsItemsCache:[{text:'old'}],liveSdsLoadFailed:false,renderLiveSds:()=>{rendered++;list.innerHTML='fresh SDS rows';},renderSecurityLabels:()=>{},renderHealthIntegrations:()=>{},fetch:async()=>({ok:false,status:503})});
 for(const name of ['loadLiveSds','refreshCachedPanelTranslations'])vm.runInContext(fn(name),c);
 await vm.runInContext('loadLiveSds()',c);assert(c.liveSdsLoadFailed);assert(list.innerHTML.includes('503'));const failure=list.innerHTML;vm.runInContext('refreshCachedPanelTranslations()',c);assert.equal(list.innerHTML,failure);assert.equal(rendered,0);
 c.fetch=async()=>{throw Error('raw SDS failure');};await vm.runInContext('loadLiveSds()',c);assert(c.liveSdsLoadFailed);assert(list.innerHTML.includes('raw SDS failure'));
 c.fetch=async()=>({ok:true,json:async()=>[{text:'fresh'}]});await vm.runInContext('loadLiveSds()',c);assert.equal(c.liveSdsLoadFailed,false);assert.equal(c.liveSdsItemsCache[0].text,'fresh');assert.equal(rendered,1);vm.runInContext('refreshCachedPanelTranslations()',c);assert.equal(rendered,2);
});
test('Security response errors replace unsaved translation metadata and survive a language change',()=>{
 const message=node();
 const c=ctx({document:{getElementById:id=>id==='secp-msg'?message:null},secpChanged:false,secpData:{running:{invalid_subscriber_keys:[]},ksgs:[]},secpPosture:()=>'',secpRenderSubs:()=>{},secpRenderAlgos:()=>{}});
 for(const name of ['secpMsg','secpDirty','renderSecurityLabels'])vm.runInContext(fn(name),c);
 vm.runInContext('secpDirty()',c);assert.equal(message.dataset.translationKey,'secp_unsaved');vm.runInContext("secpMsg('ERROR: subscriber key rejected',false)",c);assert.equal(message.dataset.translationKey,undefined);c.t=key=>'pt:'+key;vm.runInContext('renderSecurityLabels()',c);assert(c.secpChanged);assert.equal(message.textContent,'ERROR: subscriber key rejected');vm.runInContext('secpDirty()',c);c.t=key=>'fr:'+key;vm.runInContext('renderSecurityLabels()',c);assert.equal(message.textContent,'fr:secp_unsaved');
});
Promise.all(pending).then(()=>console.log(`${count} panel refresh tests passed`)).catch(error=>{console.error(error);process.exitCode=1;});
