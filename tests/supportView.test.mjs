import test from 'node:test';
import assert from 'node:assert/strict';
import { createSupportView, parseSupportAction } from '../src/supportView.ts';
import { validateFrame } from '../public/support/protocol.js';

// A deliberately tiny deterministic layout fixture. Actual browser acceptance
// lives in tests/browser/support-view.html; these are real policy executions.
class E {
  nodeType = 1; children = []; parentElement = null; dataset = {}; attributes = {}; isConnected = true;
  disabled = false; readOnly = false; scrollHeight = 300; clientHeight = 100; scrollTop = 0;
  style = { display: 'block', visibility: 'visible', opacity: '1', pointerEvents: 'auto', backgroundColor: 'rgba(0, 0, 0, 0)', color: 'rgb(20, 20, 20)', fontSize: '14px', fontWeight: '400', overflow: 'visible', overflowX: 'visible', overflowY: 'visible', getPropertyValue: () => '' };
  constructor(tag = 'div', x = 0, y = 0, w = 400, h = 50) { this.tag = tag; this.rect = { x, y, width: w, height: h }; }
  get childNodes() { return this.children; }
  get textContent() { return this.children.map(n => n.textContent).join(''); }
  getBoundingClientRect() { return this.rect; }
  getAttribute(name) { return this.attributes[name] ?? null; }
  matches(selector) { return selector.split(',').some(s => {
    if (s === ':disabled') return this.disabled;
    const a = /^\[([^=\]]+)(?:=([^\]]+))?\]$/.exec(s);
    if (a) return a[1] in this.attributes && (a[2] === undefined || this.attributes[a[1]] === a[2]);
    if (s === 'input[type=password]') return this.tag === 'input' && this.type === 'password';
    if (s === 'input[type=file]') return this.tag === 'input' && this.type === 'file';
    return s === this.tag;
  }); }
  closest(selector) { for (let p = this; p; p = p.parentElement) if (p.matches(selector)) return p; return null; }
  contains(e) { return e === this || this.children.some(child => child.nodeType === 1 && child.contains(e)); }
  add(child) { child.parentElement = this; this.children.push(child); return child; }
}
class B extends E { clicks = 0; constructor(...args) { super('button', ...args); } click() { this.clicks++; } }
class I extends E { type = 'search'; _value = ''; placeholder = ''; events = []; constructor(...args) { super('input', ...args); } get value() { return this._value; } set value(v) { this._value = v; } dispatchEvent(e) { this.events.push(e.type); } }
class S extends E { options = []; selectedOptions = []; value = ''; }
function fixture(text = 'Fictional visible message') {
  Object.assign(globalThis, { HTMLElement: E, HTMLButtonElement: B, HTMLInputElement: I, HTMLSelectElement: S, Node: { ELEMENT_NODE: 1, TEXT_NODE: 3 }, getComputedStyle: e => e.style });
  const body = new E('body', 0, 0, 980, 750);
  const paragraph = body.add(new E('p', 10, 20, 800, 20));
  paragraph.add({ nodeType: 3, textContent: text, x: 10, y: 20 });
  const button = body.add(new B(10, 60, 100, 30)); button.dataset.supportControl = 'click';
  button.add({ nodeType: 3, textContent: 'Navigate', x: 10, y: 60 });
  const doc = { body, hidden: false, defaultView: { innerWidth: 980, innerHeight: 750 }, hit: null,
    elementFromPoint(x, y) {
      if (this.hit) return this.hit;
      const descend = e => {
        for (const child of [...e.children].reverse()) {
          if (child.nodeType !== 1 || child.style.display === 'none') continue;
          const r = child.rect;
          if (x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height) return descend(child);
        } return e;
      }; return descend(body);
    },
    createRange() { let node, offset; return { setStart(n, i) { node = n; offset = i; }, setEnd() {}, getBoundingClientRect() { return { x: node.x + offset * 8, y: node.y, width: 8, height: 16 }; } }; },
  };
  let clock = 0; const view = createSupportView(doc, () => clock);
  return { body, paragraph, button, doc, view, advance(ms) { clock += ms; } };
}
const texts = f => f.items.filter(item => item.kind === 'text').map(item => item.text).join(' ');
test('actions reject extra authority, malformed identifiers, controls and bounds', () => {
  const base = { frame: 'a'.repeat(32), target: 'b'.repeat(32), kind: 'click' };
  assert.deepEqual(parseSupportAction(base), base);
  for (const input of [null, [], {}, { ...base, selector: 'body' }, { ...base, command: 'open_pst' }, { ...base, kind: 'input', value: '\0' }, { ...base, kind: 'input', value: 'x'.repeat(2001) }, { ...base, kind: 'scroll', deltaY: 701 }, { ...base, kind: 'scroll', deltaY: NaN }]) assert.equal(parseSupportAction(input), null);
});
test('visible-only inert frame is accepted by the wire validator', () => {
  const f = fixture(); const frame = f.view.capture();
  assert.match(texts(frame), /Fictional visible message/); validateFrame(frame);
  assert.equal(frame.targets.length, 1); assert.equal(frame.targets[0].kind, 'click');
  assert.equal(f.view.apply({ frame: frame.id, target: frame.targets[0].id, kind: 'click' }), true); assert.equal(f.button.clicks, 1);
});
test('private, offscreen, transparent and occluded text is not exported', () => {
  const f = fixture('Visible');
  const add = (marker, y) => { const el = f.body.add(new E('p', 10, y, 400, 20)); el.add({ nodeType: 3, textContent: marker, x: 10, y }); return el; };
  add('HIDDEN_PRIVATE', 100).attributes['data-support-private'] = 'true';
  add('HIDDEN_OFFSCREEN', 900);
  add('HIDDEN_TRANSPARENT', 130).style.color = 'rgba(0, 0, 0, 0)';
  add('HIDDEN_COLOR_FOUR', 150).style.color = 'color(srgb 0 0 0 / 0)';
  const parent = add('HIDDEN_COVERED', 170); parent.add(new E('div', 10, 170, 400, 20));
  const frame = f.view.capture(); assert.doesNotMatch(texts(frame), /HIDDEN_/); validateFrame(frame);
});
test('mail HTML cannot mint actionable controls and input values never leak', () => {
  const f = fixture();
  const mail = f.body.add(new E('section', 10, 100, 400, 100)); mail.attributes['data-support-untrusted'] = 'true';
  const fake = mail.add(new B(10, 110, 100, 20)); fake.dataset.supportControl = 'click';
  const input = f.body.add(new I(10, 230, 300, 30)); input.dataset.supportControl = 'input'; input.value = 'HIDDEN_INPUT_VALUE';
  const frame = f.view.capture(); assert.doesNotMatch(JSON.stringify(frame), /HIDDEN_INPUT_VALUE/);
  assert.equal(frame.targets.length, 2); const target = frame.targets.find(t => t.kind === 'input');
  assert.equal(f.view.apply({ frame: frame.id, target: target.id, kind: 'input', value: 'fictional query' }), true);
  assert.equal(input.value, 'fictional query'); assert.deepEqual(input.events, ['input', 'change']);
});
test('Stop, stale frame, detached, disabled, hidden and covered controls fail closed', () => {
  for (const mutate of [f => f.advance(2001), f => { f.button.isConnected = false; }, f => { f.button.disabled = true; }, f => { f.button.style.opacity = '0'; }, f => { f.doc.hidden = true; }, f => { f.doc.hit = f.paragraph; }, f => f.view.close()]) {
    const f = fixture(), frame = f.view.capture(); mutate(f);
    assert.equal(f.view.apply({ frame: frame.id, target: frame.targets[0].id, kind: 'click' }), false); assert.equal(f.button.clicks, 0);
  }
  const f = fixture(); f.view.close(); assert.throws(() => f.view.capture());
});
test('only a bounded recent generation of target capabilities is retained', () => {
  const f = fixture(), first = f.view.capture();
  f.view.capture(); f.view.capture(); f.view.capture();
  assert.equal(f.view.apply({ frame: first.id, target: first.targets[0].id, kind: 'click' }), false);
});
