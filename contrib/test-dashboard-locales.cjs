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
  assert.equal([...dashboard.matchAll(new RegExp(`data-lang="${language}"`, 'g'))].length, 2, `${language}: desktop/mobile selectors`);
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
console.log(`Dashboard locales OK: ${Object.keys(langs.en).length} keys, PT/FR placeholders, commands, selectors, script syntax and login preferences.`);
