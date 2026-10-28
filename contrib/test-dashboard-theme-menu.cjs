#!/usr/bin/env node
// Run with: node contrib/test-dashboard-theme-menu.cjs
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const source = fs.readFileSync(path.join(__dirname, '../crates/tetra-entities/src/net_dashboard/html.rs'), 'utf8');
const dashboard = source.split('pub const DASHBOARD_HTML: &str = r#"')[1].split('"#;')[0];
const dictionary = dashboard.slice(dashboard.indexOf('const LANGS={'), dashboard.indexOf('// Fill gaps from English'));
const langs = vm.runInNewContext(dictionary + ';LANGS');
const markup = dashboard.slice(dashboard.indexOf('<div class="theme-wrap">'), dashboard.indexOf('<div class="language-wrap">'));
const themes = [...markup.matchAll(/data-t="([^"]+)"/g)].map(match => match[1]);
assert.deepEqual(themes, ['dark', 'light', 'blue'], 'One shared menu, in Dark/Light/Blue order');
assert.equal([...dashboard.matchAll(/id="theme-btn"/g)].length, 1, 'One trigger on desktop and mobile');
assert.equal([...dashboard.matchAll(/class="theme-btn theme-opt/g)].length, 3, 'No duplicate mobile choices');
assert.equal([...markup.matchAll(/role="menuitemradio"/g)].length, 3);
assert.ok(markup.includes('aria-controls="theme-pop"'));
assert.ok(markup.includes('onkeydown="themeMenuKeydown(event)" hidden>'), 'Closed menu is hidden');
assert.ok(!dashboard.includes('id="prefs-pop"'), 'Old horizontal mobile menu is removed');
assert.match(dashboard, /\.theme-opt\{[^}]*display:flex;[^}]*width:100%;/s, 'Choices fill separate vertical rows');
assert.match(dashboard, /@media\(pointer:coarse\)\{\.theme-trigger,\.theme-opt\{min-height:44px;/, 'Touch targets remain usable');

const elements = new Map();
const listeners = new Map();
const closed = [];
let menuDocument;
function element(id, theme) {
  const classes = new Set();
  return {
    id, hidden: id === 'theme-pop', dataset: theme ? {t: theme} : {}, attributes: {}, textContent: '',
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
for (const id of ['theme-pop', 'theme-btn', 'theme-current']) elements.set(id, element(id));
const options = themes.map(theme => element('option-' + theme, theme));
const pop = elements.get('theme-pop');
const button = elements.get('theme-btn');
const current = elements.get('theme-current');
pop.querySelectorAll = () => options;
pop.querySelector = () => options.find(option => option.classList.contains('active'));
menuDocument = {
  activeElement: null,
  documentElement: element('html'),
  getElementById: id => elements.get(id) || null,
  querySelectorAll: selector => selector.includes('.theme-opt') ? options : [],
  addEventListener(type, callback) { listeners.set(type, callback); }
};
const saved = new Map([['fs_theme', 'blue'], ['fs_lang', 'pt'], ['fs_uisize', 'h']]);
const context = vm.createContext({
  document: menuDocument, currentLang: 'pt',
  localStorage: {getItem: key => saved.get(key) ?? null, setItem: (key, value) => saved.set(key, value)},
  t: key => langs[context.currentLang][key] || langs.en[key],
  closeLanguagePop() { closed.push('language'); }, closeReadPop() { closed.push('readability'); },
  closePowerPop() { closed.push('power'); }
});
const handlers = dashboard.slice(dashboard.indexOf("let currentTheme="), dashboard.indexOf('// ── Readability'));
const compatibility = dashboard.slice(dashboard.indexOf('function togglePrefsPop('), dashboard.indexOf('function syncPowerMenuUi('));
const dismissal = dashboard.slice(dashboard.indexOf('// Outside-click + Esc dismissal'), dashboard.indexOf('// ── Touch mode'));
vm.runInContext(handlers + compatibility + dismissal, context);
vm.runInContext('setTheme(currentTheme)', context);
assert.equal(current.textContent, langs.pt.ui_blue, 'Persisted theme is restored');
assert.equal(menuDocument.documentElement.attributes['data-theme'], 'blue');

const key = value => ({key: value, preventDefault() {}, stopPropagation() {}});
context.toggleThemePop(key('Enter'));
assert.equal(pop.hidden, false);
assert.equal(button.attributes['aria-expanded'], 'true');
assert.equal(menuDocument.activeElement.dataset.t, 'blue', 'Opening focuses the current choice');
assert.deepEqual(closed, ['language', 'readability', 'power'], 'Other toolbar menus close');
assert.equal(options.filter(option => option.attributes['aria-checked'] === 'true').length, 1);
context.themeMenuKeydown(key('ArrowDown'));
assert.equal(menuDocument.activeElement.dataset.t, 'dark', 'Arrow navigation wraps');
context.themeMenuKeydown(key('End'));
assert.equal(menuDocument.activeElement.dataset.t, 'blue');
context.themeMenuKeydown(key('Home'));
assert.equal(menuDocument.activeElement.dataset.t, 'dark');
context.themeMenuKeydown(key('ArrowUp'));
assert.equal(menuDocument.activeElement.dataset.t, 'blue');
context.chooseTheme('dark');
assert.equal(saved.get('fs_theme'), 'dark');
assert.equal(menuDocument.documentElement.attributes['data-theme'], '', 'Dark keeps the existing root attribute');
assert.equal(current.attributes['data-i18n'], 'ui_dark');
assert.equal(pop.hidden, true);
assert.equal(menuDocument.activeElement, button, 'Choosing restores trigger focus');

for (const language of ['en', 'es', 'fr', 'pt', 'ro', 'de', 'hu', 'zh']) {
  context.currentLang = language;
  for (const theme of themes) {
    context.setTheme(theme);
    assert.equal(current.textContent, langs[language]['ui_' + theme] || langs.en['ui_' + theme], `${language}: selected label`);
    assert.equal(button.attributes['aria-label'], (langs[language].prefs_theme || langs.en.prefs_theme) + ': ' + current.textContent);
    assert.equal(pop.attributes['aria-label'], langs[language].prefs_theme || langs.en.prefs_theme);
  }
}
context.themeTriggerKeydown(key('ArrowDown'));
assert.equal(menuDocument.activeElement.dataset.t, 'dark');
context.themeMenuKeydown(key('Escape'));
assert.equal(pop.hidden, true);
assert.equal(menuDocument.activeElement, button);
context.themeTriggerKeydown(key('ArrowUp'));
assert.equal(menuDocument.activeElement.dataset.t, 'blue');
listeners.get('keydown')(key('Escape'));
assert.equal(pop.hidden, true, 'Global Escape closes and restores focus');
assert.equal(menuDocument.activeElement, button);
context.toggleThemePop(key('Enter'));
listeners.get('click')({target: {closest: selector => selector === '.theme-wrap' ? {} : null}});
assert.equal(pop.hidden, false, 'Clicking inside the theme menu keeps it open');
listeners.get('click')({target: {closest: () => null}});
assert.equal(pop.hidden, true, 'Outside click closes the menu');
context.toggleThemePop(key('Enter'));
context.themeMenuKeydown(key('Tab'));
assert.equal(pop.hidden, true, 'Tab closes without trapping focus');
assert.equal(menuDocument.activeElement, button, 'Native Tab traversal resumes from the trigger');
context.setTheme('unsupported');
assert.equal(saved.get('fs_theme'), 'light', 'Invalid persisted choices fall back to Light');
assert.equal(saved.get('fs_lang'), 'pt', 'Theme selection preserves language');
assert.equal(saved.get('fs_uisize'), 'h', 'Theme selection preserves readability');
console.log('Dashboard theme menu checks passed (layout, 8 locales, persistence, keyboard and dismissal).');
