export function parseHexColor(value) {
  const match = /^#([0-9a-f]{6})$/i.exec(value.trim());
  if (!match) {
    throw new Error(`Expected a six-digit hex color, received: ${value}`);
  }
  const numeric = Number.parseInt(match[1], 16);
  return [numeric >> 16, (numeric >> 8) & 0xff, numeric & 0xff];
}

function linearChannel(channel) {
  const normalized = channel / 255;
  return normalized <= 0.04045
    ? normalized / 12.92
    : ((normalized + 0.055) / 1.055) ** 2.4;
}

export function contrastRatio(foreground, background) {
  const luminance = (color) => {
    const [red, green, blue] = parseHexColor(color).map(linearChannel);
    return (0.2126 * red) + (0.7152 * green) + (0.0722 * blue);
  };
  const lighter = Math.max(luminance(foreground), luminance(background));
  const darker = Math.min(luminance(foreground), luminance(background));
  return (lighter + 0.05) / (darker + 0.05);
}

export function extractCssBlock(css, selector) {
  const start = css.indexOf(`${selector} {`);
  if (start < 0) {
    throw new Error(`CSS selector not found: ${selector}`);
  }
  const openingBrace = css.indexOf("{", start);
  let depth = 0;
  for (let index = openingBrace; index < css.length; index += 1) {
    if (css[index] === "{") depth += 1;
    if (css[index] === "}") depth -= 1;
    if (depth === 0) return css.slice(openingBrace + 1, index);
  }
  throw new Error(`Unclosed CSS block: ${selector}`);
}

export function extractColorTokens(block) {
  return Object.fromEntries(
    [...block.matchAll(/(--[a-z0-9-]+)\s*:\s*(#[0-9a-f]{6})\s*;/gi)]
      .map(([, name, value]) => [name, value.toLowerCase()]),
  );
}
