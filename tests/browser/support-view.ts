import { createSupportView } from '../../src/supportView';
// Browser-only acceptance with fictional DOM, no source files or network peer.
let count = 0;
const navigate = document.getElementById('navigate')!;
navigate.onclick = () => { document.getElementById('click-count')!.textContent = 'Navigation count: ' + ++count; };
document.getElementById('run')!.onclick = () => {
  const view = createSupportView(document);
  const frame = view.capture();
  const data = JSON.stringify(frame);
  const target = frame.targets.find(t => t.label.includes('Navigate'))!;
  const input = frame.targets.find(t => t.kind === 'input')!;
  const controlWorked = view.apply({ frame: frame.id, target: target?.id, kind: 'click' });
  const inputWorked = view.apply({ frame: frame.id, target: input?.id, kind: 'input', value: 'fictional query' });
  const fake = document.getElementById('fake')!.getBoundingClientRect();
  const untrustedBlocked = !frame.targets.some(t => t.x >= fake.x && t.y >= fake.y && t.x < fake.right && t.y < fake.bottom);
  const canvas = document.getElementById('mirror') as HTMLCanvasElement;
  canvas.width = frame.width; canvas.height = frame.height;
  const context = canvas.getContext('2d')!;
  for (const item of frame.items) {
    if (item.kind === 'box') { context.fillStyle = item.fill; context.fillRect(item.x, item.y, item.w, item.h); }
    else { context.fillStyle = item.color; context.font = `${item.weight} ${item.size}px system-ui`; context.textBaseline = 'top'; context.fillText(item.text, item.x, item.y); }
  }
  view.close();
  const stopped = !view.apply({ frame: frame.id, target: target?.id, kind: 'click' });
  document.getElementById('result')!.textContent = JSON.stringify({ visibleIncluded: data.includes('Fictional visible message'), privateExcluded: !data.includes('PRIVATE_INVITATION_MARKER'), offscreenExcluded: !data.includes('OFFSCREEN_MARKER'), occludedExcluded: !data.includes('OCCLUDED_MARKER'), transparentExcluded: !data.includes('TRANSPARENT_MARKER'), inputValueExcluded: !data.includes('Do not export hidden input values'), untrustedBlocked, controlWorked, inputWorked, stopped, frameBytes: new TextEncoder().encode(data).length, itemCount: frame.items.length }, null, 2);
};
