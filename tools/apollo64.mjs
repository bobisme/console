#!/usr/bin/env node

// Reproducibly expand AdamCYounis' 46-color Apollo palette into the console's
// 64-color ramp grid. The original 46 swatches remain byte-for-byte anchors.
// Each chromatic ramp gains two OKLCH midpoints; the neutral ramp gains six
// OKLab midpoints. At every insertion we split the largest remaining
// perceptual gap (Euclidean delta in OKLab).

"use strict";

const APOLLO = [
  ["172038", "253a5e", "3c5e8b", "4f8fba", "73bed3", "a4dddb"],
  ["19332d", "25562e", "468232", "75a743", "a8ca58", "d0da91"],
  ["4d2b32", "7a4841", "ad7757", "c09473", "d7b594", "e7d5b3"],
  ["341c27", "602c2c", "884b2b", "be772b", "de9e41", "e8c170"],
  ["241527", "411d31", "752438", "a53030", "cf573c", "da863e"],
  ["1e1d39", "402751", "7a367b", "a23e8c", "c65197", "df84a5"],
];

const NEUTRALS = [
  "090a14", "10141f", "151d28", "202e37", "394a50",
  "577277", "819796", "a8b5b2", "c7cfcc", "ebede9",
];

function srgbToLinear(c) {
  c /= 255;
  return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
}

function linearToSrgb(c) {
  const v = c <= 0.0031308 ? 12.92 * c : 1.055 * c ** (1 / 2.4) - 0.055;
  return v * 255;
}

function hexToLab(hex) {
  const rgb = [0, 2, 4].map((i) => srgbToLinear(Number.parseInt(hex.slice(i, i + 2), 16)));
  const l = 0.4122214708 * rgb[0] + 0.5363325363 * rgb[1] + 0.0514459929 * rgb[2];
  const m = 0.2119034982 * rgb[0] + 0.6806995451 * rgb[1] + 0.1073969566 * rgb[2];
  const s = 0.0883024619 * rgb[0] + 0.2817188376 * rgb[1] + 0.6299787005 * rgb[2];
  const l3 = Math.cbrt(l), m3 = Math.cbrt(m), s3 = Math.cbrt(s);
  return [
    0.2104542553 * l3 + 0.793617785 * m3 - 0.0040720468 * s3,
    1.9779984951 * l3 - 2.428592205 * m3 + 0.4505937099 * s3,
    0.0259040371 * l3 + 0.7827717662 * m3 - 0.808675766 * s3,
  ];
}

function labToLinear([L, a, b]) {
  const l3 = L + 0.3963377774 * a + 0.2158037573 * b;
  const m3 = L - 0.1055613458 * a - 0.0638541728 * b;
  const s3 = L - 0.0894841775 * a - 1.291485548 * b;
  const l = l3 ** 3, m = m3 ** 3, s = s3 ** 3;
  return [
    4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
  ];
}

function labToHex(lab) {
  const rgb = labToLinear(lab).map(linearToSrgb);
  return rgb.map((v) => Math.round(Math.max(0, Math.min(255, v))).toString(16).padStart(2, "0")).join("");
}

function labToLch([L, a, b]) {
  return [L, Math.hypot(a, b), Math.atan2(b, a)];
}

function lchToLab([L, C, h]) {
  return [L, C * Math.cos(h), C * Math.sin(h)];
}

function midpointLab(a, b) {
  return a.map((v, i) => (v + b[i]) / 2);
}

function midpointLch(a, b) {
  const [L0, C0, h0] = labToLch(a);
  const [L1, C1, h1] = labToLch(b);
  const tau = Math.PI * 2;
  let dh = ((h1 - h0 + Math.PI) % tau + tau) % tau - Math.PI;
  return lchToLab([(L0 + L1) / 2, (C0 + C1) / 2, h0 + dh / 2]);
}

function inGamut(lab) {
  return labToLinear(lab).every((v) => v >= 0 && v <= 1);
}

function gamutMap(lab) {
  if (inGamut(lab)) return lab;
  const [L, C, h] = labToLch(lab);
  let lo = 0, hi = C;
  for (let i = 0; i < 24; i++) {
    const mid = (lo + hi) / 2;
    if (inGamut(lchToLab([L, mid, h]))) lo = mid;
    else hi = mid;
  }
  return lchToLab([L, lo, h]);
}

function delta(a, b) {
  return Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]);
}

function expand(hexes, target, chromatic) {
  const colors = hexes.map((hex) => ({ hex, lab: hexToLab(hex), original: true }));
  while (colors.length < target) {
    let split = 0, gap = -1;
    for (let i = 0; i + 1 < colors.length; i++) {
      const d = delta(colors[i].lab, colors[i + 1].lab);
      if (d > gap) [split, gap] = [i, d];
    }
    const raw = chromatic
      ? midpointLch(colors[split].lab, colors[split + 1].lab)
      : midpointLab(colors[split].lab, colors[split + 1].lab);
    const lab = gamutMap(raw);
    colors.splice(split + 1, 0, { hex: labToHex(lab), lab, original: false });
  }
  return colors;
}

const ramps = APOLLO.map((ramp) => expand(ramp, 8, true));
ramps.push(expand(NEUTRALS, 16, false));
const palette = ramps.flat();

if (palette.length !== 64) throw new Error(`internal error: generated ${palette.length} colors`);

for (let family = 0; family < ramps.length; family++) {
  console.log(`# family ${family}`);
  for (let shade = 0; shade < ramps[family].length; shade++) {
    const color = ramps[family][shade];
    const index = ramps.slice(0, family).reduce((n, ramp) => n + ramp.length, 0) + shade;
    console.log(`${index.toString().padStart(2)} #${color.hex}${color.original ? "" : "  generated"}`);
  }
}
