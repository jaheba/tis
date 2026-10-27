// DOM logic smoke test; this does not replace checking the rendered page in a browser.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const filename = process.argv[2];
if (!filename) throw new Error('Usage: node tests/viewer.mjs /path/to/generated-ir.html');
const source = readFileSync(filename, 'utf8');
const payload = source.match(/<script type="application\/json" id="ir-data">([\s\S]*?)<\/script>/)?.[1];
const script = source.match(/<script>\n([\s\S]*?)<\/script>/)?.[1];
assert.ok(payload && script, 'expected standalone viewer data and script');
const data = JSON.parse(payload);

class Element {
  constructor(name) { this.name = name; this.attributes = new Map(); this.children = []; this.listeners = new Map(); this.textContent = ''; }
  setAttribute(name, value) { this.attributes.set(name, String(value)); }
  getAttribute(name) { return this.attributes.get(name); }
  set className(value) { this.setAttribute('class', value); }
  get classList() {
    const element = this;
    return {
      contains(name) { return (element.getAttribute('class') ?? '').split(/\s+/).includes(name); },
      add(...names) { element.className = [...new Set((element.getAttribute('class') ?? '').split(/\s+/).concat(names))].join(' '); },
      remove(...names) { element.className = (element.getAttribute('class') ?? '').split(/\s+/).filter(name => !names.includes(name)).join(' '); },
    };
  }
  appendChild(child) { child.parent = this; this.children.push(child); return child; }
  replaceChildren() { this.children = []; }
  addEventListener(name, listener) { if (!this.listeners.has(name)) this.listeners.set(name, []); this.listeners.get(name).push(listener); }
  emit(name, event = {}) { for (const listener of this.listeners.get(name) ?? []) listener({ target: this, stopPropagation() {}, preventDefault() {}, ...event }); }
  matches(selector) {
    if (selector.startsWith('.')) return this.classList.contains(selector.slice(1));
    const attribute = selector.match(/^\[([^=\]]+)(?:="([^"]*)")?\]$/);
    return attribute ? this.attributes.has(attribute[1]) && (attribute[2] === undefined || this.getAttribute(attribute[1]) === attribute[2]) : false;
  }
  querySelectorAll(selector) {
    const result = [];
    for (const child of this.children) {
      if (selector.split(',').some(selector => child.matches(selector))) result.push(child);
      result.push(...child.querySelectorAll(selector));
    }
    return result;
  }
  querySelector(selector) { return this.querySelectorAll(selector)[0]; }
  closest(selector) { return this.matches(selector) ? this : this.parent?.closest(selector); }
  getScreenCTM() { return { a: 1, inverse() { return this; } }; }
  setPointerCapture() {}
}

const elements = new Map(['ir-data', 'graph', 'details', 'selection', 'summary', 'fit', 'zoom-in', 'zoom-out', 'download'].map(id => [id, new Element(id)]));
elements.get('ir-data').textContent = payload;
const document = {
  getElementById(id) { assert.ok(elements.has(id), `unknown element ${id}`); return elements.get(id); },
  createElement(name) { return new Element(name); },
  createElementNS(_namespace, name) { return new Element(name); },
};
const context = { document, console, Blob, URL, setTimeout, DOMPoint: class { constructor(x, y) { this.x = x; this.y = y; } matrixTransform() { return this; } } };
vm.runInNewContext(script, context, { filename: 'viewer.js' });

const graph = elements.get('graph');
assert.equal(graph.querySelectorAll('[data-block]').length, data.nodes.length);
assert.equal(graph.querySelectorAll('[data-value]').length, data.nodes.reduce((n, node) => n + node.instructions.length, 0));
assert.match(elements.get('summary').textContent, new RegExp(`${data.nodes.length} blocks`));

const addition = data.nodes.flatMap(node => node.instructions).find(instruction => instruction.feedback);
assert.ok(addition, 'use the sum or add demo for this smoke test');
graph.querySelector(`[data-value="${addition.id}"]`).emit('click');
assert.equal(elements.get('selection').textContent, `Value %${addition.id}`);
assert.equal(graph.querySelectorAll('.definition').length, 1);
function content(element) { return element.textContent + element.children.map(content).join(' '); }
assert.match(content(elements.get('details')), /Runtime feedback/);
assert.ok(content(elements.get('details')).includes(addition.feedback.strategy));

const initial = graph.getAttribute('viewBox');
elements.get('zoom-in').onclick();
assert.notEqual(graph.getAttribute('viewBox'), initial);
elements.get('fit').onclick();
assert.equal(graph.getAttribute('viewBox'), initial);
graph.emit('pointerdown', { clientX: 20, clientY: 20, pointerId: 1 });
graph.emit('pointermove', { clientX: 50, clientY: 60 });
assert.notEqual(graph.getAttribute('viewBox'), initial);
graph.emit('pointerup');
elements.get('fit').onclick();

graph.querySelector('[data-block="0"]').emit('click');
assert.equal(elements.get('selection').textContent, data.nodes[0].header);
assert.equal(graph.querySelectorAll('.definition').length, 0);
console.log('Viewer DOM smoke checks passed: graph, selection, feedback, zoom, pan and reset.');
