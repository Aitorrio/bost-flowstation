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
const menuMarkup = dashboard.slice(dashboard.indexOf('<div class="language-wrap">'), dashboard.indexOf('<!-- Phone:'));
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
console.log(`Dashboard locales OK: ${Object.keys(langs.en).length} keys, PT/FR placeholders, script syntax, login preferences and accessible language menu.`);
