import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { supportSharingFailureNotice } from '../src/supportFailureNotice.ts';

test('sharing failure codes distinguish capture, invalid frame, and ended session without exposing errors', () => {
  assert.match(supportSharingFailureNotice('capture', new Error('fictional private diagnostic')), /S01: view capture failed/);
  assert.match(supportSharingFailureNotice('publish', 'The shared application frame exceeds its permitted limits.'), /S02: frame rejected/);
  assert.match(supportSharingFailureNotice('publish', 'This support session is unavailable or has stopped.'), /S03: session ended/);
  for (const value of ['fictional private diagnostic', 'The shared application frame exceeds its permitted limits. fictional private diagnostic', new Error('fictional private diagnostic'), { message: 'fictional private diagnostic' }, { get message() { throw new Error('Must not inspect arbitrary error properties'); } }, null, undefined]) {
    const notice = supportSharingFailureNotice('publish', value);
    assert.match(notice, /S04: frame publication failed/);
    assert.doesNotMatch(notice, /fictional private diagnostic/);
  }
});

const styles = readFileSync(new URL('../src/styles.css', import.meta.url), 'utf8');
const rule = selector => styles.slice(styles.indexOf(selector + ' {')).split('}')[0];
const tokens = body => Object.fromEntries([...body.matchAll(/(--[\w-]+):\s*([^;]+);/g)].map(([, key, value]) => [key, value.trim()]));
const light = tokens(styles.match(/:root \{([\s\S]*?)\n\}/)[1]);
const dark = { ...light, ...tokens(styles.match(/:root\[data-appearance="dark"\] \{([\s\S]*?)\n\}/)[1]) };
function property(body, name, theme, fallback) {
  const value = new RegExp('(?:[;{]\\s*)' + name + ':\\s*([^;]+)').exec(body)?.[1] ?? fallback;
  return value.replace(/var\((--[\w-]+)(?:,\s*([^)]*))?\)/g, (_, key, alternate) => theme[key] ?? alternate ?? '');
}
function luminance(hex) {
  let value = hex.trim().slice(1);
  if (value.length === 3) value = [...value].map(c => c + c).join('');
  assert.match(value, /^[a-f\d]{6}$/i);
  const rgb = [0, 2, 4].map(offset => parseInt(value.slice(offset, offset + 2), 16) / 255)
    .map(channel => channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4);
  return rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722;
}
for (const selector of ['.support-live-banner', '.support-stop-notice']) {
  test(selector + ' uses defined theme colors and stays readable in both themes', () => {
    const body = rule(selector);
    for (const [, key] of body.matchAll(/var\((--[\w-]+)/g)) assert.ok(key in light, 'Undefined support theme token: ' + key);
    for (const theme of [light, dark]) {
      const foreground = luminance(property(body, 'color', theme, theme['--color-text-primary']));
      const background = luminance(property(body, 'background', theme, theme['--color-panel-background']));
      assert.ok((Math.max(foreground, background) + 0.05) / (Math.min(foreground, background) + 0.05) >= 4.5);
    }
  });
}
