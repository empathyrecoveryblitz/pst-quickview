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
  const doc = { body, hidden: false, defaultView: { innerWidth: 980, innerHeight: 750 }, hit: null, ranges: [], rangeRect: null,
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
    createRange() { let node, start, end; return { setStart(n, i) { node = n; start = i; }, setEnd(n, i) { assert.equal(n, node); end = i; }, getBoundingClientRect() { doc.ranges.push({ node, start, end }); return doc.rangeRect?.(node, start, end) ?? { x: node.x + start * 8, y: node.y, width: (end - start) * 8, height: 16 }; } }; },
  };
  let clock = 0; const view = createSupportView(doc, () => clock);
  return { body, paragraph, button, doc, view, advance(ms) { clock += ms; } };
}
const texts = f => f.items.filter(item => item.kind === 'text').map(item => item.text).join(' ');
const paragraphText = frame => frame.items.filter(item => item.kind === 'text' && item.y === 20).map(item => item.text).join('');
function assertSafeFrame(frame) {
  validateFrame(frame);
  const values = frame.items.filter(item => item.kind === 'text').map(item => item.text)
    .concat(frame.targets.flatMap(target => [target.label, ...(target.value === undefined ? [] : [target.value])]));
  assert.equal(values.every(value => value.isWellFormed()), true);
  assert.ok(frame.items.filter(item => item.kind === 'text').every(item => item.text.length <= 1000));
  assert.ok(frame.items.filter(item => item.kind === 'text').reduce((total, item) => total + item.text.length, 0) <= 12000);
}
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
test('capture keeps zero, nonfinite and oversized viewport dimensions within the protocol', () => {
  for (const value of [0, -1, NaN, Infinity, -Infinity, undefined, 0.5, 8192]) {
    const f = fixture();
    f.doc.defaultView.innerWidth = value; f.doc.defaultView.innerHeight = value;
    const frame = f.view.capture();
    assertSafeFrame(frame);
    const valid = Number.isFinite(value) && value > 0;
    assert.equal(frame.width, valid ? Math.min(4096, Math.max(1, value)) : 1280);
    assert.equal(frame.height, valid ? Math.min(2160, Math.max(1, value)) : 820);
    if (!valid) { assert.deepEqual(frame.items, []); assert.deepEqual(frame.targets, []); }
  }
});
test('capture omits unsupported colors without exporting potentially invisible text', () => {
  for (const value of ['rgb(20%, 20%, 20%)', '#12345', '#1234567', 'rgb(256, 20, 20)', 'rgb(12.5, 20, 20)', 'rgb(20 20 20)', 'rgba(20, 20, 20, 1.1)', 'rgba(20, 20, 20, 0.123456)', 'color(srgb 0 0 0 / 0)']) {
    const f = fixture('HIDDEN_UNSUPPORTED_COLOR');
    f.body.style.backgroundColor = value; f.paragraph.style.color = value;
    const frame = f.view.capture();
    assertSafeFrame(frame);
    assert.doesNotMatch(texts(frame), /HIDDEN_UNSUPPORTED_COLOR/);
  }
  for (const value of ['rgba(20, 20, 20, 0)', 'rgba(20, 20, 20, .0000)', 'transparent', 'color(srgb 0 0 0 / 0)']) {
    const f = fixture();
    const select = f.body.add(new S('select', 10, 110, 400, 30));
    select.style.color = value; select.selectedOptions = [{ textContent: 'HIDDEN_SELECTED_TEXT' }];
    const frame = f.view.capture();
    assertSafeFrame(frame);
    assert.doesNotMatch(texts(frame), /HIDDEN_SELECTED_TEXT/);
  }
});
test('backward overlapping visible glyphs keep a nonnegative union rectangle', () => {
  const f = fixture('AB'); f.button.style.display = 'none';
  f.doc.rangeRect = (node, start) => ({ x: 30 - start, y: 20, width: 0.5, height: 16 });
  const frame = f.view.capture();
  assertSafeFrame(frame);
  assert.deepEqual(frame.items.filter(item => item.kind === 'text').map(({ x, y, w, h, text }) => ({ x, y, w, h, text })), [{ x: 29, y: 20, w: 1.5, h: 16, text: 'AB' }]);
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
test('control label truncation never splits an astral character at the UTF-16 limit', () => {
  for (const prefix of [98, 99]) {
    const f = fixture();
    f.button.rect.width = 950;
    const source = 'A'.repeat(prefix) + '\u{1f600}';
    f.button.children[0].textContent = source;
    const frame = f.view.capture();
    assert.equal(frame.targets[0].label, 'A'.repeat(prefix) + (prefix === 98 ? '\u{1f600}' : ''));
    assert.equal(f.button.children[0].textContent, source);
    assertSafeFrame(frame);
  }
});
test('DOM ranges use complete code points and preserve visible astral text', () => {
  const source = 'A\u{1f600}B';
  const f = fixture(source), node = f.paragraph.children[0];
  const frame = f.view.capture();
  assert.deepEqual(f.doc.ranges.filter(range => range.node === node).map(({ start, end }) => [start, end]), [[0, 1], [1, 3], [3, 4]]);
  assert.equal(paragraphText(frame), source);
  assert.equal(node.textContent, source);
  assertSafeFrame(frame);
});
test('malformed UTF-16 and prohibited controls are repaired only in outgoing copies', () => {
  const source = '\ud800A\udc00\u{1f600}\u0000\u007fB';
  const f = fixture(source);
  const frame = f.view.capture();
  assert.equal(paragraphText(frame), '\ufffdA\ufffd\u{1f600}\ufffd\ufffdB');
  assert.equal(f.paragraph.children[0].textContent, source);
  assertSafeFrame(frame);
});
test('drawing item and aggregate UTF-16 limits never split astral characters', () => {
  for (const prefix of [998, 999, 1000, 11998, 11999]) {
    const source = 'A'.repeat(prefix) + '\u{1f600}B';
    const f = fixture(source);
    f.button.style.display = 'none';
    f.doc.rangeRect = (node, start, end) => ({ x: node.x + start * 0.01, y: node.y, width: (end - start) * 0.01, height: 16 });
    const frame = f.view.capture();
    const expected = prefix < 11998 ? source : 'A'.repeat(prefix) + (prefix === 11998 ? '\u{1f600}' : '');
    assert.equal(paragraphText(frame), expected);
    assert.equal(f.paragraph.children[0].textContent, source);
    assertSafeFrame(frame);
    if (prefix >= 11998) assert.equal(frame.truncated, true);
  }
});
test('selected option value and display text use safe bounded copies without exposing other choices', () => {
  for (const prefix of [1998, 1999]) {
    const f = fixture();
    const select = f.body.add(new S('select', 10, 110, 950, 30));
    select.dataset.supportControl = 'input';
    const sourceValue = 'V'.repeat(prefix) + '\u{1f600}';
    const textPrefix = prefix === 1998 ? 65 : 66;
    const sourceText = 'T'.repeat(textPrefix) + '\u{1f600}';
    select.value = sourceValue;
    select.selectedOptions = [{ textContent: sourceText }];
    select.options = [{ value: sourceValue }, { value: 'HIDDEN_OTHER_OPTION', textContent: 'HIDDEN_OTHER_OPTION' }];
    const frame = f.view.capture(), target = frame.targets.find(target => target.kind === 'input');
    assert.equal(target.value, 'V'.repeat(prefix) + (prefix === 1998 ? '\u{1f600}' : ''));
    assert.equal(frame.items.find(item => item.kind === 'text' && item.y === 110).text, 'T'.repeat(textPrefix) + (textPrefix === 65 ? '\u{1f600}' : ''));
    assert.equal(select.value, sourceValue);
    assert.equal(select.selectedOptions[0].textContent, sourceText);
    assert.doesNotMatch(JSON.stringify(frame), /HIDDEN_OTHER_OPTION/);
    assertSafeFrame(frame);
  }
  const f = fixture();
  const select = f.body.add(new S('select', 10, 110, 950, 30));
  select.dataset.supportControl = 'input'; select.value = '\ud800'; select.selectedOptions = [{ textContent: 'Option \udc00' }];
  const frame = f.view.capture();
  assert.equal(frame.targets.find(target => target.kind === 'input').value, '\ufffd');
  assert.equal(frame.items.find(item => item.kind === 'text' && item.y === 110).text, 'Option \ufffd');
  assert.equal(select.value, '\ud800');
  assertSafeFrame(frame);
});
test('selected display text contributes to the same aggregate frame text limit', () => {
  const f = fixture();
  f.paragraph.style.display = 'none'; f.button.style.display = 'none';
  f.body.rect.width = 4096; f.doc.defaultView.innerWidth = 4096;
  for (let index = 0; index < 30; index++) {
    const select = f.body.add(new S('select', 0, index * 20, 4096, 20));
    select.style.fontSize = '8px'; select.selectedOptions = [{ textContent: '\u{1f600}'.repeat(300) }];
  }
  const frame = f.view.capture();
  assertSafeFrame(frame);
  assert.equal(frame.items.filter(item => item.kind === 'text').reduce((total, item) => total + item.text.length, 0), 12000);
  assert.equal(frame.truncated, true);
});
test('a partially clipped astral character is omitted whole without disclosing its hidden suffix', () => {
  const source = 'A\u{1f600}HIDDEN';
  const f = fixture(source), node = f.paragraph.children[0];
  f.paragraph.style.overflowX = 'hidden'; f.paragraph.rect.width = 20;
  const frame = f.view.capture();
  assert.equal(paragraphText(frame), 'A');
  assert.deepEqual(f.doc.ranges.filter(range => range.node === node).slice(0, 2).map(({ start, end }) => [start, end]), [[0, 1], [1, 3]]);
  assert.equal(node.textContent, source);
  assertSafeFrame(frame);
});
