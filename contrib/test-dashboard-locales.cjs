#!/usr/bin/env node
// Dashboard translation checks. Run with: node contrib/test-dashboard-locales.cjs
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const source = fs.readFileSync(path.join(__dirname, '../crates/tetra-entities/src/net_dashboard/html.rs'), 'utf8');
const dashboard = source.split('pub const DASHBOARD_HTML: &str = r#"')[1].split('"#;')[0];
const login = source.split('pub const LOGIN_HTML: &str = r##"')[1].split('"##;')[0];
const dictionary = dashboard.slice(dashboard.indexOf('const LANGS={'), dashboard.indexOf('// Fill gaps from English'));
const langs = vm.runInNewContext(dictionary + ';LANGS');
const placeholders = value => [...value.matchAll(/\{\w+\}/g)].map(match => match[0]).sort();

for (const language of ['pt', 'fr']) {
  assert.deepEqual(Object.keys(langs[language]).sort(), Object.keys(langs.en).sort(), `${language}: dictionary coverage`);
  for (const [key, english] of Object.entries(langs.en)) {
    const value = langs[language][key];
    assert.equal(typeof value, 'string', `${language}.${key}`);
    assert.ok(value.trim(), `${language}.${key}: empty translation`);
    assert.deepEqual(placeholders(value), placeholders(english), `${language}.${key}: placeholders`);
    const commands = [...english.matchAll(/`([^`]+)`/g)].map(match => match[1]);
    for (const command of commands) assert.ok(value.includes('`' + command + '`'), `${language}.${key}: literal command`);
  }
  assert.equal([...dashboard.matchAll(new RegExp(`data-lang="${language}"`, 'g'))].length, 1, `${language}: shared desktop/mobile language menu`);
}

// Keys used by static content and accessible field attributes must resolve.
for (const match of dashboard.matchAll(/data-i18n(?:-tab|-section|-placeholder|-title|-aria-label)?="([^"]+)"/g)) {
  assert.ok(langs.en[match[1]], `Missing UI key: ${match[1]}`);
}
for (const match of dashboard.matchAll(/\bt\(['"]([^'"]+)['"]/g)) {
  assert.ok(langs.en[match[1]], `Missing dynamic key: ${match[1]}`);
}

// Parse all embedded scripts, including the login page, without executing APIs.
for (const match of source.matchAll(/<script\b[^>]*>([\s\S]*?)<\/script>/g)) new vm.Script(match[1]);
// Driver output is machine-readable evidence, not a display label.
assert.ok(dashboard.includes("out.includes('already installed')"));
assert.ok(dashboard.includes("out.includes('already loaded')"));
assert.ok(dashboard.includes("s.enabled==='enabled'"));

const languageFunction = dashboard.slice(dashboard.indexOf('function dashboardLanguage('), dashboard.indexOf('let currentLang='));
const context = vm.createContext({});
vm.runInContext(dictionary + languageFunction, context);
for (const [input, expected] of [['pt-PT', 'pt'], ['pt', 'pt'], ['fr-FR', 'fr'], ['fr-CA', 'fr'], ['FR', 'fr'], ['es', 'es'], ['unsupported', 'es']]) {
  context.input = input;
  assert.equal(vm.runInContext('dashboardLanguage(input)', context), expected);
}

const loginScript = login.slice(login.indexOf('const LOGIN_I18N='), login.indexOf('const form ='));
for (const [saved, browser, expected] of [[null, 'pt-PT', 'pt-PT'], ['fr', 'pt-PT', 'fr'], [null, 'fr-FR', 'fr'], ['pt-PT', 'en-US', 'pt-PT']]) {
  const elements = new Map();
  const document = {documentElement: {}, getElementById: id => {
    if (!elements.has(id)) elements.set(id, {});
    return elements.get(id);
  }};
  vm.runInNewContext(loginScript, {document, navigator: {language: browser}, localStorage: {getItem: () => saved}});
  assert.equal(document.documentElement.lang, expected, 'Login regional/saved language');
  assert.ok(elements.get('lbl-user').textContent);
  assert.ok(elements.get('submit-btn').textContent);
}
// The shared language menu exposes every existing dictionary, with native names.
const menuMarkup = dashboard.slice(dashboard.indexOf('<div class="language-wrap">'), dashboard.indexOf('<!-- Session logout'));
const menuCodes = [...menuMarkup.matchAll(/data-lang="([^"]+)"/g)].map(match => match[1]);
assert.deepEqual(menuCodes.slice().sort(), Object.keys(langs).sort(), 'Every available language appears once');
assert.deepEqual(menuCodes, ['en', 'es', 'fr', 'pt', 'ro', 'de', 'hu', 'zh'], 'Language menu order');
const triggerMarkup = menuMarkup.slice(menuMarkup.indexOf('<button'), menuMarkup.indexOf('</button>'));
assert.equal(triggerMarkup.replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ').trim(), 'ES', 'Trigger shows only the selected code');
assert.equal([...menuMarkup.matchAll(/role="menuitemradio"/g)].length, menuCodes.length);
assert.ok(menuMarkup.includes('aria-controls="language-pop"'));
assert.ok(menuMarkup.includes('Português (Portugal)'));
assert.ok(menuMarkup.includes('Français'));
assert.ok(menuMarkup.includes('简体中文'));
assert.ok(menuMarkup.includes(' hidden>'), 'Closed menu must be absent from keyboard navigation');

// Exercise the real menu handlers, including lossless locale selection and focus dismissal.
const elements = new Map();
const listeners = new Map();
let menuDocument;
function menuElement(id, language) {
  const classes = new Set();
  return {
    id, hidden: id === 'language-pop', dataset: language ? {lang: language} : {}, attributes: {}, textContent: '',
    classList: {
      add: value => classes.add(value), remove: value => classes.delete(value), contains: value => classes.has(value),
      toggle(value, force) {
        const on = force === undefined ? !classes.has(value) : force;
        if (on) classes.add(value); else classes.delete(value);
        return on;
      }
    },
    setAttribute(name, value) { this.attributes[name] = value; },
    focus() { menuDocument.activeElement = this; }
  };
}
for (const id of ['language-pop', 'language-btn', 'language-current']) elements.set(id, menuElement(id));
const options = menuCodes.map(code => menuElement(`option-${code}`, code));
const pop = elements.get('language-pop');
pop.querySelectorAll = () => options;
pop.querySelector = () => options.find(option => option.classList.contains('active'));
menuDocument = {
  activeElement: null,
  getElementById: id => elements.get(id) || null,
  querySelectorAll: selector => selector.includes('.language-opt') ? options : [],
  addEventListener(type, callback) { listeners.set(type, callback); }
};
const saved = new Map();
const menuContext = vm.createContext({
  document: menuDocument, currentLang: 'pt', applyLang() {},
  localStorage: {setItem: (key, value) => saved.set(key, value)},
  dashboardLanguage: value => Object.hasOwn(langs, value) ? value : 'es',
  t: key => langs[menuContext.currentLang][key] || langs.en[key],
  closeReadPop() {}, closePowerPop() {}, closePrefsPop() {}, syncPrefsMenuUi() {}
});
const menuHandlers = dashboard.slice(dashboard.indexOf('function setLang('), dashboard.indexOf('let currentTheme='));
vm.runInContext(menuHandlers, menuContext);
const dismissHandlers = dashboard.slice(dashboard.indexOf('// Outside-click + Esc dismissal'), dashboard.indexOf('// ── Touch mode'));
vm.runInContext(dismissHandlers, menuContext);
const key = value => ({key: value, preventDefault() {}, stopPropagation() {}});
const button = elements.get('language-btn');
menuContext.toggleLanguagePop(key('Enter'));
assert.equal(pop.hidden, false);
assert.equal(button.attributes['aria-expanded'], 'true');
assert.equal(menuDocument.activeElement.dataset.lang, 'pt');
assert.equal(options.filter(option => option.attributes['aria-checked'] === 'true').length, 1);
assert.equal(button.attributes['aria-label'], 'Idioma: Português (Portugal)');
assert.equal(elements.get('language-current').textContent, 'PT');
menuContext.languageMenuKeydown(key('End'));
assert.equal(menuDocument.activeElement.dataset.lang, 'zh');
menuContext.languageMenuKeydown(key('ArrowDown'));
assert.equal(menuDocument.activeElement.dataset.lang, 'en', 'Arrow navigation wraps');
menuContext.languageMenuKeydown(key('Home'));
assert.equal(menuDocument.activeElement.dataset.lang, 'en');
menuContext.chooseLanguage('fr');
assert.equal(saved.get('fs_lang'), 'fr');
assert.equal(button.attributes['aria-label'], 'Langue: Français');
assert.equal(elements.get('language-current').textContent, 'FR');
assert.equal(pop.hidden, true);
assert.equal(menuDocument.activeElement, button);
menuContext.languageTriggerKeydown(key('ArrowUp'));
assert.equal(pop.hidden, false);
assert.equal(menuDocument.activeElement.dataset.lang, 'zh');
listeners.get('keydown')(key('Escape'));
assert.equal(pop.hidden, true);
assert.equal(menuDocument.activeElement, button);
menuContext.toggleLanguagePop(key('Enter'));
listeners.get('click')({target: {closest: () => null}});
assert.equal(pop.hidden, true, 'Outside click closes the menu');
menuContext.toggleLanguagePop(key('Enter'));
menuContext.languageMenuKeydown(key('Tab'));
assert.equal(pop.hidden, true, 'Tab closes without trapping focus');
assert.equal(menuDocument.activeElement, button, 'Native Tab traversal resumes from the trigger');
// Exercise the real refresh path with cached live data. No browser or station
// API is needed: changing a display preference must never reload a form or
// issue requests that can overwrite unsaved settings.
function dashboardFunction(name) {
  const start = dashboard.search(new RegExp(`(?:async )?function ${name}\\(`));
  assert.notEqual(start, -1, `Missing dashboard function: ${name}`);
  const tail = dashboard.slice(start);
  for (const end of tail.matchAll(/^}/gm)) {
    const candidate = tail.slice(0, end.index + 1);
    try { new vm.Script(candidate); return candidate; } catch (_) {}
  }
  assert.fail(`Cannot parse dashboard function: ${name}`);
}

function refreshElement(tagName = 'div', attributes = {}) {
  let content = '', markup = '';
  const classes = new Set();
  const el = {
    tagName: tagName.toUpperCase(), attributes: {...attributes}, children: [], style: {}, dataset: {},
    value: '', checked: false, disabled: false, open: false,
    getAttribute(name) { return this.attributes[name] ?? null; },
    setAttribute(name, value) { this.attributes[name] = String(value); },
    removeAttribute(name) { delete this.attributes[name]; },
    appendChild(child) { this.children.push(child); return child; },
    querySelector(selector) { return this.children.find(child => selector === 'span' ? child.tagName === 'SPAN' : child.getAttribute?.('data-i18n') === 'bs_label') || null; },
    querySelectorAll() { return []; },
    reset() { assert.fail('Language selection must not reset a form'); },
    classList: {
      add(...values) { values.forEach(value => classes.add(value)); },
      remove(...values) { values.forEach(value => classes.delete(value)); },
      contains(value) { return classes.has(value); },
      toggle(value, force) {
        const on = force === undefined ? !classes.has(value) : force;
        if (on) classes.add(value); else classes.delete(value);
        return on;
      }
    }
  };
  Object.defineProperties(el, {
    textContent: {get() { return content + this.children.map(child => child.textContent || '').join(''); }, set(value) { content = String(value); markup = ''; this.children = []; }},
    innerHTML: {get() { return markup; }, set(value) { markup = String(value); content = ''; this.children = []; }},
    options: {get() { return this.children; }},
    title: {get() { return this.attributes.title || ''; }, set(value) { this.attributes.title = String(value); }},
    id: {get() { return this.attributes.id; }},
    className: {get() { return [...classes].join(' '); }, set(value) { classes.clear(); String(value).split(/\s+/).filter(Boolean).forEach(part => classes.add(part)); }}
  });
  return el;
}

const refreshElements = new Map();
const translatedElements = [];
// Use every static translation attribute from the actual dashboard, including
// hidden submenus and modal labels, rather than a list mirroring applyLang.
for (const tag of dashboard.matchAll(/<([a-z][a-z\d-]*)\b([^<>]*)>/gi)) {
  const attributes = Object.fromEntries([...tag[2].matchAll(/([\w-]+)="([^"]*)"/g)].map(match => [match[1], match[2]]));
  if (!Object.keys(attributes).some(name => name.startsWith('data-i18n'))) continue;
  const el = refreshElement(tag[1], attributes);
  translatedElements.push(el);
  if (el.id) refreshElements.set(el.id, el);
}
const element = id => {
  if (!refreshElements.has(id)) refreshElements.set(id, refreshElement('div', {id}));
  return refreshElements.get(id);
};
const homePage = element('page-stations');
homePage.classList.add('active');
const networkPage = element('page-network');
networkPage.classList.add('active');
const powerTitle = refreshElement();
const pageTitles = vm.runInNewContext(dashboard.match(/const PAGE_TITLES=([^;]+);/)[0] + ';PAGE_TITLES');
const navLabels = new Map([...Object.keys(pageTitles), 'security'].map(name => [name, refreshElement()]));
for (const id of ['topbar-title', 'bts-tx', 'bts-rx', 'bts-shift', 'bts-mcc', 'bts-mnc', 'bts-carrier', 'bts-neighbor', 'bts-hang', 'bts-access', 'bts-access-sub', 'bts-secondary-wrap', 'cells-list', 'cells-link', 'network-links-list', 'eth-saved-list', 'eth-saved-count', 'wifi-status-grid', 'wifi-radio-btn', 'wifi-saved-list', 'wifi-saved-count', 'wifi-scan-list', 'connLed', 'connText', 'brewLed', 'brewText', 'brewVerBadge', 'stat-brew-val', 'stat-brew-sub', 'stat-brew-card', 'sysBtsStatus', 'sysBrewStatus', 'power-opt-suspend-label', 'power-opt-restart', 'power-menu-btn', 'logout-btn', 'theme-btn', 'theme-pop', 'theme-current', 'sys-auth-badge', 'sys-auth-new-user', 'setup-helper-val', 'setup-device-list', 'wiz-device-list', 'wiz-device', 'ast-configured', 'ast-enabled', 'ast-register', 'ast-dialogs', 'ast-last-error', 'ast-hero-dot', 'ast-hero-pill', 'ast-hero-sub', 'tg-detected', 'dap-hero-pill', 'geo-hero-pill']) element(id);
const draft = element('config-editor');
draft.value = 'unsaved station settings';
const draftToggle = element('vc-brew-enabled');
draftToggle.checked = true;
const draftDetails = element('configuration-submenu');
draftDetails.open = true;
element('wiz-device').value = 'driver=unsaved-device';
element('sys-auth-new-user').value = 'unsaved-user';
const remoteDraftInput = refreshElement('input');
remoteDraftInput.value = '61099';
const remoteDraftAction = refreshElement('select');
remoteDraftAction.value = 'temperature';
const remoteDeleteButton = refreshElement('button');
const remoteDeleteLabel = refreshElement('span');
remoteDeleteButton.appendChild(remoteDeleteLabel);
remoteDeleteButton.querySelector = selector => selector === '.remote-cmd-del-text' ? remoteDeleteLabel : null;
const remoteRows = element('remote-cmd-rows');
remoteRows.appendChild(remoteDraftInput); remoteRows.appendChild(remoteDraftAction); remoteRows.appendChild(remoteDeleteButton);
remoteRows.querySelectorAll = selector => selector === '.remote-cmd-code' ? [remoteDraftInput] : selector === '.remote-cmd-del' ? [remoteDeleteButton] : [];
const remoteDraftNodes = remoteRows.children.slice();

function makeProfileSelect(id, selection) {
  const select = refreshElement('select', {id});
  for (const [value, label] of [['', langs.en.cfg_brew_offline], ['__lst_dispatch__', langs.en.cfg_brew_lst], ['Core Lisbon', 'Core Lisbon ●'], ['French test', 'French test']]) {
    const option = refreshElement('option');
    option.value = value; option.textContent = label; select.appendChild(option);
  }
  select.value = selection;
  refreshElements.set(id, select);
  return select;
}
const homeBrew = makeProfileSelect('home-brew-profile', 'Core Lisbon');
const configBrew = makeProfileSelect('vc-brew-profile', '__lst_dispatch__');
const profileOptions = [...homeBrew.options, ...configBrew.options];
const apiCalls = [];
const refreshWarnings = [];
const forbiddenRequest = name => (...args) => { apiCalls.push([name, ...args]); throw new Error(`Unexpected language refresh request: ${name}`); };
const refreshDocument = {
  documentElement: {},
  getElementById: id => refreshElements.get(id) || null,
  createElement: tag => refreshElement(tag),
  createTextNode: text => ({textContent: String(text)}),
  querySelector(selector) {
    if (selector === '.page.active') return homePage;
    if (selector === '#power-pop .power-pop-title') return powerTitle;
    const nav = selector.match(/^#nav-(\w+) \.nav-label$/);
    return nav ? navLabels.get(nav[1]) || null : null;
  },
  querySelectorAll(selector) {
    const attribute = selector.match(/^\[(data-i18n[\w-]*)\]$/);
    if (attribute) return [...new Set([...translatedElements, ...refreshElements.values()])].filter(el => el.getAttribute(attribute[1]) !== null);
    if (selector === '.page') return [homePage, networkPage];
    return [];
  }
};
const noop = () => {};
const refreshGlobals = {
  document: refreshDocument, currentLang: 'en', currentTheme: 'light', LANGS: langs,
  localStorage: {setItem: noop}, location: {hostname: 'station.test', reload: forbiddenRequest('location.reload')},
  fetch: forbiddenRequest('fetch'), console: {...console, warn: (...args) => refreshWarnings.push(args)},
  PAGE_TITLES: pageTitles,
  state: {ms: {}, calls: {}, emergencies: {}, brewOnline: false, brewVer: 1, cellSec: null},
  serviceStandby: false, dashLinkState: 'online', dcState: {}, tsCarrierInfo: {}, tsCells: [], rfCellsCache: null,
  BTS_TOWER_ICON: '<svg></svg>', BTS_CLOCK_ICON: '<svg></svg>',
  setText(id, text) { const el = refreshDocument.getElementById(id); if (el) el.textContent = text; },
  escHtml: text => String(text).replace(/[&<>"']/g, ch => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[ch])),
  escHtmlAttr: text => String(text),
  btsInfoCache: {tx_freq_hz: 438175000, rx_freq_hz: 430575000, shift_hz: -7600000, mcc: 901, mnc: 9999, main_carrier: 1527, neighbor_count: 2, hangtime_secs: 2, whitelist_restricted: false},
  cellsCache: {cells: [{id: 0, primary: true, carriers: [{carrier_num: 1527, tx_freq_hz: 438175000, rx_freq_hz: 430575000}], registered_radios: 3, colour_code: 1, location_area: 1, rf_state: 'online'}], siteLinked: false},
  sysData: null, systemProfilesCache: null, liveSdsItemsCache: null,
  dashboardAuthCache: {auth_enabled: true},
  setupStateCache: {setup: {setup_complete: true}, config_device: 'driver=saved-device', devices: []}, setupSelectedDevice: 'driver=unsaved-device',
  secpData: null, stationHealthCache: null, systemHealthCache: null, displayedSdrHealthCache: null, lstLastStatus: null, lstGeoOpen: false,
  remoteCmds: [{status_code: 61000, action: 'ip'}],
  asteriskStatusCache: {config: {configured: true, enabled: true}, runtime: {register_status: 'REGISTER 200 OK', active_dialogs: 2}}, asteriskStatusFailed: false,
  tgDetected: [{name: 'HamTetra <test>', id: -123456, kind: 'group'}], tgDetectedLoaded: true,
  otaLastCheck: null, otaPendingCheck: null, otaChannelHintSnapshot: null, otaProgressSnapshot: null, updateLogExpanded: false, updateStartedAt: null,
  networkState: {status: {interfaces: [{name: 'eth0', kind: 'ethernet', is_default: true, state: 'connected', connection: 'Wired connection 1', ipv4: ['192.0.2.10']}]}, ethSavedLoaded: true, ethSaved: [{name: 'Lab ethernet', uuid: 'ethernet-1', active: true}]},
  wifiState: {status: {device_present: true, radio_enabled: true, connected_ssid: 'Station WiFi', signal: 80, ip_address: '192.0.2.20'}, savedLoaded: true, scanLoaded: true, saved: [{name: 'Saved WiFi', uuid: 'wifi-1', active: false}], scan: [{ssid: 'Open WiFi', signal: 75, security: '--', saved: true, active: false}]}
};
for (const name of ['renderStations', 'renderCalls', 'renderLastHeard', 'renderEmergencyBanner', 'renderDgnaPage', 'renderSdsLog', 'renderDapnetLog', 'renderGeoalarmEvents', 'renderRemoteIssis', 'renderRemoteCmds', 'renderTgChips', 'lstRenderRoster', 'lstRenderScan', 'lstRenderActivity', 'lstRenderSdsInbox', 'updateTsBlocksCarrier', 'updateWhitelistBanner', 'renderWhitelist', 'applyServiceStateUi', 'updateHwRfUi', 'installCfgHelp', 'setDcSub', 'tsEnsureCarrierInfo', 'renderTsGridCarrier', 'renderRfCells', 'updateSysHero', 'syncLanguageMenuUi', 'applyTableStackLabels', 'renderHealthIntegrations', 'updateRfStatusBanner']) refreshGlobals[name] = noop;
for (const name of ['loadConfig', 'loadVisualConfig', 'loadBtsInfo', 'loadBtsInfoLegacy', 'loadCells', 'loadDualCarrier', 'refreshProfileSelects', 'networkLoadStatus', 'networkLoadEthernetSaved', 'wifiLoadSaved', 'wifiLoadStatus', 'wifiScan', 'loadSystemInfo']) refreshGlobals[name] = forbiddenRequest(name);
const refreshContext = vm.createContext(refreshGlobals);
for (const name of ['t', 'dashboardLanguage', 'renderBtsInfo', 'renderCells', 'renderRemoteCmds', 'refreshRemoteCommandTranslations', 'refreshProfileSelectTranslations', 'networkKindLabel', 'networkLocalizeState', 'networkLocalizeConn', 'renderNetworkError', 'networkRenderLinks', 'networkRenderEthernetSaved', 'wifiRenderStatus', 'wifiRenderSaved', 'wifiRenderScan', 'wifiSignalBars', 'setDashLinkState', 'setBrewStatus', 'setCellSecurity', 'updateSysBtsPanel', 'syncTopbarChips', 'syncPowerMenuUi', 'syncThemeMenuUi', 'syncPrefsMenuUi', 'renderSystemInfo', 'renderSecurityLabels', 'renderSetupStatus', 'paintRfPill', 'renderDeviceList', 'renderAsteriskStatus', 'setIntegrationHero', 'renderDetectedTelegramChats', 'tgEsc', 'refreshCachedPanelTranslations', 'refreshOtaTranslations', 'applyLang', 'setLang']) vm.runInContext(dashboardFunction(name), refreshContext);
refreshContext.setIntegrationHero('dap', true, true, langs.en.online, '', 'online');
refreshContext.setIntegrationHero('geo', false, false, langs.en.ui_disabled, '', 'ui_disabled');

for (const language of ['en', 'pt', 'fr']) {
  refreshContext.setLang(language);
  const tr = langs[language];
  assert.equal(refreshDocument.documentElement.lang, language === 'pt' ? 'pt-PT' : language);
  assert.equal(element('topbar-title').textContent, tr.stations, `${language}: active page title`);
  for (const [name, label] of navLabels) assert.equal(label.textContent, tr[name], `${language}: ${name} navigation label`);
  for (const el of translatedElements) {
    for (const attribute of ['data-i18n', 'data-i18n-section', 'data-i18n-tab']) {
      const key = el.getAttribute(attribute);
      if (key) assert.equal(el.textContent, tr[key], `${language}: hidden/static ${key}`);
    }
    for (const attribute of ['placeholder', 'title', 'aria-label']) {
      const key = el.getAttribute('data-i18n-' + attribute);
      // The power trigger's current-state hint deliberately supersedes its
      // static accessible name in syncPowerMenuUi.
      if (key) assert.equal(el.getAttribute(attribute), tr[el.id === 'power-menu-btn' ? 'top_power_menu_hint' : key], `${language}: translated ${attribute}`);
    }
  }
  assert.equal(element('bts-access-sub').textContent, tr.bts_wl_open, `${language}: cached BTS access`);
  assert.ok(element('bts-neighbor').innerHTML.includes(tr.ui_neighbor_cell), `${language}: neighbor chip prefix`);
  assert.ok(element('bts-neighbor').innerHTML.includes(tr.ui_neighbors), `${language}: neighbor chip count`);
  assert.ok(element('bts-hang').innerHTML.includes(tr.ui_hangtime.replace('{seconds}', '2')), `${language}: HangTime label and value`);
  assert.equal(homeBrew.options[0].textContent, tr.cfg_brew_offline, `${language}: home offline option`);
  assert.equal(configBrew.options[1].textContent, tr.cfg_brew_lst, `${language}: configuration dispatch option`);
  assert.equal(homeBrew.options[2].textContent, 'Core Lisbon ●', 'User profile names are not translated');
  assert.equal(homeBrew.value, 'Core Lisbon', 'Home profile selection survives');
  assert.equal(configBrew.value, '__lst_dispatch__', 'Unapplied configuration profile selection survives');
  assert.deepEqual([...homeBrew.options, ...configBrew.options], profileOptions, 'Relabelling preserves option nodes');
  assert.ok(element('network-links-list').textContent.includes(tr.network_nm_connected), `${language}: cached interface status`);
  assert.ok(element('eth-saved-list').textContent.includes(tr.wifi_disconnect), `${language}: cached Ethernet actions`);
  assert.ok(element('wifi-saved-list').textContent.includes(tr.wifi_connect), `${language}: cached saved WiFi action`);
  assert.ok(element('wifi-scan-list').textContent.includes(tr.wifi_open), `${language}: cached scan security label`);
  assert.equal(element('connText').textContent, tr.online, `${language}: live dashboard link state`);
  assert.equal(element('brewText').textContent, tr.brew_offline, `${language}: live Brew state`);
  assert.equal(element('power-opt-suspend-label').textContent, tr.svc_suspend, `${language}: power submenu`);
  assert.equal(element('theme-btn').attributes['aria-label'], tr.prefs_theme + ': ' + tr.ui_light, `${language}: appearance submenu`);
  assert.equal(element('theme-current').textContent, tr.ui_light, `${language}: selected theme label`);
  assert.equal(element('cells-link').textContent, tr.cells_single, `${language}: cached cell summary`);
  assert.ok(element('cells-list').innerHTML.includes(tr.cells_radios.replace('{n}', '3')), `${language}: cached cell radio count`);
  assert.equal(draft.value, 'unsaved station settings');
  assert.equal(draftToggle.checked, true);
  assert.equal(draftDetails.open, true);
  assert.equal(element('sys-auth-badge').textContent, tr.sys_account_protected, `${language}: account security badge`);
  assert.equal(element('setup-helper-val').textContent, tr.ui_not_installed, `${language}: cached setup status`);
  assert.equal(element('wiz-device').value, 'driver=unsaved-device', 'Changing language does not restore the configured SDR over a draft');
  assert.equal(refreshContext.setupSelectedDevice, 'driver=unsaved-device');
  assert.equal(element('sys-auth-new-user').value, 'unsaved-user');
  assert.deepEqual(remoteRows.children, remoteDraftNodes, 'Remote commands are relabelled without replacing draft controls');
  assert.equal(remoteDraftInput.value, '61099');
  assert.equal(remoteDraftAction.value, 'temperature');
  assert.equal(remoteDraftInput.placeholder, tr.remote_status_code);
  assert.equal(remoteDeleteLabel.textContent, tr.remote_cmd_del);
  assert.equal(element('ast-configured').textContent, tr.setup_yes.toUpperCase(), `${language}: cached Asterisk configuration status`);
  assert.equal(element('ast-enabled').textContent, tr.ui_enabled, `${language}: cached Asterisk enabled status`);
  assert.equal(element('ast-dialogs').textContent, '2' + tr.ui_active_dialogs, `${language}: cached Asterisk active dialogs`);
  assert.equal(element('ast-register').textContent, 'REGISTER 200 OK', 'SIP status is preserved as backend evidence');
  assert.equal(element('ast-hero-pill').textContent, 'REGISTER 200 OK', 'Backend SIP response is not replaced by a translation key');
  assert.equal(element('ast-hero-pill').getAttribute('data-i18n'), null);
  assert.equal(element('dap-hero-pill').textContent, tr.online, `${language}: generated DAPNET status`);
  assert.equal(element('geo-hero-pill').textContent, tr.ui_disabled, `${language}: generated Geoalarm status`);
  assert.ok(element('tg-detected').innerHTML.includes(tr.tg_detect_found), `${language}: cached Telegram discovery summary`);
  assert.ok(element('tg-detected').innerHTML.includes(tr.tg_add), `${language}: cached Telegram add action`);
  assert.ok(element('tg-detected').innerHTML.includes('HamTetra &lt;test&gt;'), 'Discovered chat names remain escaped and untranslated');
  assert.deepEqual(apiCalls, [], 'Language switching is entirely local and must not call APIs or form loaders');
  assert.deepEqual(refreshWarnings, [], 'All language refreshers complete without silently skipped panels');
}

// A failed refresh must stay visible even when a previous successful response
// is cached. Switching language must not make disconnected data look current.
const networkErrors = [
  [refreshContext.networkState, 'statusError', 'network-links-list'],
  [refreshContext.networkState, 'ethSavedError', 'eth-saved-list'],
  [refreshContext.wifiState, 'statusError', 'wifi-status-grid'],
  [refreshContext.wifiState, 'savedError', 'wifi-saved-list'],
  [refreshContext.wifiState, 'scanError', 'wifi-scan-list']
];
for (const [cache, field] of networkErrors) cache[field] = 'Device <unavailable>';
for (const language of ['pt', 'fr']) {
  refreshContext.setLang(language);
  for (const [, , id] of networkErrors) assert.ok(element(id).innerHTML.includes('Device &lt;unavailable&gt;'), `${language}: preserve and escape ${id} failure`);
}
for (const [cache, field] of networkErrors) cache[field] = null;
refreshContext.wifiState.scanLoading = true;
for (const language of ['pt', 'fr']) {
  refreshContext.setLang(language);
  assert.ok(element('wifi-scan-list').innerHTML.includes(langs[language].wifi_scanning), `${language}: in-progress scan stays in progress`);
}
refreshContext.wifiState.scanLoading = false;
refreshContext.dashLinkState = 'reconnecting';
refreshContext.btsInfoCache.whitelist_restricted = true;
refreshContext.btsInfoCache.whitelist_count = 3;
refreshContext.btsInfoCache.neighbor_count = 0;
for (const language of ['pt', 'fr']) {
  refreshContext.setLang(language);
  assert.equal(element('connText').textContent, langs[language].reconnecting, `${language}: reconnecting stays reconnecting`);
  assert.equal(element('bts-access').textContent, langs[language].ui_restricted, `${language}: restricted registration stays restricted`);
  assert.equal(element('bts-access-sub').textContent, '3 ' + langs[language].bts_wl_entries);
  assert.ok(element('bts-neighbor').innerHTML.includes(langs[language].ui_off), `${language}: disabled neighbors stay disabled`);
}
refreshContext.asteriskStatusFailed = true;
refreshContext.tgDetected = [];
for (const language of ['en', 'pt', 'fr']) {
  refreshContext.setLang(language);
  assert.equal(element('ast-enabled').textContent, langs[language].ui_status_unavailable, `${language}: failed Asterisk status stays unavailable`);
  assert.equal(element('ast-last-error').textContent, langs[language].conn_error, `${language}: failed Asterisk response`);
  assert.equal(element('ast-hero-pill').textContent, langs[language].conn_error, `${language}: unavailable SIP status does not reuse a cached success`);
  assert.ok(element('tg-detected').innerHTML.includes(langs[language].tg_detect_none), `${language}: empty Telegram discovery result`);
}
assert.deepEqual(apiCalls, [], 'Cached failure/status translation must not use the network');
assert.deepEqual(refreshWarnings, [], 'No panel refresh failures were swallowed');
console.log(`Dashboard locales OK: ${Object.keys(langs.en).length} keys, PT/FR placeholders, syntax, login/menu accessibility and cached language refresh without API calls or lost form edits.`);
require('./test-dashboard-language-panels.cjs');
