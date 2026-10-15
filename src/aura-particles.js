// 3D aura particles around the player. One THREE.Points per aura; every particle moves in the vertex
// shader from its random seeds and the time uniform, so a frame costs one uniform update on the CPU.
// Positions are in skin pixels around the body centre (feet at y = -18, top of the head at +14).
import * as THREE from 'three';

const MODE = { rise: 0, spiral: 1, fall: 2, blink: 3, float: 4 };
const SHAPE = { pixel: 0, wisp: 1, snow: 2, glyph: 3, petal: 4 };

// 7x7 glyphs: 0-3 portal runes, 4-7 neon shapes.
const GLYPHS = [
  ['#.....#', '.#...#.', '..#.#..', '...#...', '..#.#..', '.#.....', '#......'],
  ['###.###', '#.....#', '#.###.#', '..#.#..', '#.###.#', '#.....#', '###.###'],
  ['...#...', '..###..', '.#.#.#.', '...#...', '...#...', '.#####.', '...#...'],
  ['#.#.#.#', '...#...', '.#####.', '...#...', '..#.#..', '.#...#.', '#.....#'],
  ['...#...', '..#.#..', '..#.#..', '.#...#.', '.#...#.', '#.....#', '#######'],
  ['#######', '#.....#', '#.....#', '#..#..#', '#.....#', '#.....#', '#######'],
  ['...#...', '...#...', '...#...', '#######', '...#...', '...#...', '...#...'],
  ['#.....#', '.#...#.', '..#.#..', '...#...', '..#.#..', '.#...#.', '#.....#']
];

// Aura ids match the wardrobe (launcher.logic.js cosDefs().aura).
export const AURAS = {
  flame:   { mode: 'rise',   shape: 'pixel', count: 180, size: [7, 13],  period: 3.2, radius: [6, 12], swirl: 1.6,
             colors: ['#FFB13B', '#FF7A1F', '#FF4A1C', '#C2260E', '#FFD27A', '#6A6470'] },
  emerald: { mode: 'rise',   shape: 'pixel', count: 140, size: [6, 11],   period: 4.5, radius: [7, 13], swirl: 0.6,
             colors: ['#5BD07A', '#9BE8AE', '#2A8040', '#C8FFD6'] },
  portal:  { mode: 'spiral', shape: 'glyph', count: 110, size: [14, 20], period: 14,  radius: [9, 13], glyphs: [0, 4],
             colors: ['#C04CFF', '#E58CFF', '#7A2FD0', '#B88CFF', '#2FA88A'] },
  snow:    { mode: 'fall',   shape: 'snow',  count: 150, size: [10, 15],  period: 7,   radius: [6, 15],
             colors: ['#FFFFFF', '#DDF2FF', '#9FD8FF', '#B8FFFF'] },
  neon:    { mode: 'blink',  shape: 'glyph', count: 110, size: [15, 22], period: 2.4, radius: [9, 15], glyphs: [4, 4],
             colors: ['#4FF0FF', '#FF3FD2', '#6FF6FF', '#FF7AE0'] },
  soul:    { mode: 'float',  shape: 'wisp',  count: 36,  size: [24, 40], period: 9,   radius: [9, 14],
             colors: ['#B3F0FF', '#7FE3FF', '#4FC3F7', '#E0FAFF'] },
  sakura:  { mode: 'fall',   shape: 'petal', count: 90,  size: [10, 15],  period: 9,   radius: [6, 15], normalBlend: true,
             colors: ['#FFB7D5', '#F48FB1', '#FFD6E7', '#FF8FB8'] }
};

const vertexShader = /* glsl */ `
  uniform float uTime, uScale, uPeriod, uSwirl, uGlyphBase, uGlyphCount;
  uniform int uMode;
  uniform vec2 uRadius;
  attribute vec4 aSeed;
  attribute vec3 aColor;
  attribute float aSize;
  varying vec3 vColor;
  varying float vAlpha, vGlyph, vSpin;
  const float TAU = 6.2831853;

  float hash(float n) { return fract(sin(n) * 43758.5453); }

  void main() {
    float t = fract(uTime / uPeriod + aSeed.x);
    float r = mix(uRadius.x, uRadius.y, aSeed.z);
    float a = aSeed.y * TAU;
    float size = aSize;
    float alpha = 1.0;
    vec3 p;
    if (uMode == 0) {            // rise: feet to head, swirling inward, fading out
      a += t * uSwirl * 3.14159 + uTime * 0.2;
      r *= 1.0 - 0.35 * t;
      p = vec3(cos(a) * r, mix(-18.0, 18.0, t), sin(a) * r);
      alpha = smoothstep(0.0, 0.12, t) * (1.0 - smoothstep(0.6, 1.0, t));
      size *= 1.0 - 0.5 * t;
    } else if (uMode == 1) {     // spiral: slow climb round the body
      float y = mix(-18.0, 18.0, t);
      a += uTime * (0.35 + 0.25 * aSeed.w) + y * 0.18;
      p = vec3(cos(a) * r, y, sin(a) * r);
      alpha = smoothstep(0.0, 0.15, t) * (1.0 - smoothstep(0.8, 1.0, t)) * (0.6 + 0.4 * sin(uTime * 3.0 + aSeed.w * TAU));
    } else if (uMode == 2) {     // fall: drifting down, swaying, twinkling
      a += uTime * 0.5 * (aSeed.w - 0.5);
      p = vec3(cos(a) * r + sin(uTime * 0.9 + aSeed.w * TAU) * 1.5,
               mix(22.0, -18.0, t),
               sin(a) * r + cos(uTime * 0.7 + aSeed.y * TAU) * 1.5);
      alpha = smoothstep(0.0, 0.1, t) * (1.0 - smoothstep(0.85, 1.0, t));
      alpha *= 0.7 + 0.3 * sin(uTime * 5.0 + aSeed.w * 40.0);
    } else if (uMode == 3) {     // blink: flash somewhere new every cycle
      float cycle = floor(uTime / uPeriod + aSeed.x);
      a += hash(cycle * 12.9898 + aSeed.y * 78.233) * TAU;
      float y = mix(-18.0, 18.0, hash(cycle * 4.1414 + aSeed.w * 19.19)) + t * 3.0;
      p = vec3(cos(a) * r, y, sin(a) * r);
      alpha = smoothstep(0.0, 0.04, t) * (1.0 - smoothstep(0.08, 0.4, t));
    } else {                     // float: wisps hovering round the body
      a += uTime * 0.25 * (0.6 + aSeed.w * 0.8);
      r += sin(uTime * 0.5 + aSeed.y * TAU) * 2.0;
      p = vec3(cos(a) * r, mix(-14.0, 14.0, aSeed.x) + sin(uTime * 0.7 + aSeed.w * TAU) * 3.0, sin(a) * r);
      alpha = 0.55 + 0.45 * sin(uTime * 1.3 + aSeed.w * TAU);
    }
    vColor = aColor;
    vAlpha = alpha;
    vGlyph = uGlyphBase + floor(aSeed.w * uGlyphCount);
    vSpin = aSeed.y * TAU + uTime * (aSeed.w - 0.5) * 3.0;
    vec4 mv = modelViewMatrix * vec4(p, 1.0);
    gl_PointSize = size * uScale / -mv.z;
    gl_Position = projectionMatrix * mv;
  }
`;

const fragmentShader = /* glsl */ `
  uniform int uShape;
  uniform sampler2D uGlyphs;
  varying vec3 vColor;
  varying float vAlpha, vGlyph, vSpin;

  void main() {
    vec2 pc = gl_PointCoord;
    float m = 1.0;
    if (uShape == 1) {           // soft wisp with a hot core
      float d = length(pc - 0.5) * 2.0;
      if (d > 1.0) discard;
      m = pow(1.0 - d, 1.6) + 0.6 * smoothstep(0.35, 0.0, d);
    } else if (uShape == 2) {    // 7x7 pixel snow crystal
      vec2 q = floor(pc * 7.0) - 3.0;
      if (!(q.x == 0.0 || q.y == 0.0 || (abs(q.x) == abs(q.y) && abs(q.x) <= 2.0))) discard;
    } else if (uShape == 3) {    // glyph from the atlas
      if (texture2D(uGlyphs, vec2((vGlyph + pc.x) / 8.0, 1.0 - pc.y)).r < 0.5) discard;
    } else if (uShape == 4) {    // tumbling petal
      vec2 q = pc - 0.5;
      float c = cos(vSpin), s = sin(vSpin);
      q = mat2(c, -s, s, c) * q;
      q.x *= 1.8;
      if (length(q) > 0.42) discard;
    }
    gl_FragColor = vec4(vColor * m, vAlpha * min(m, 1.0));
  }
`;

let glyphAtlas = null;
function glyphTexture() {
  if (glyphAtlas) return glyphAtlas;
  const c = document.createElement('canvas');
  c.width = 8 * GLYPHS.length; c.height = 8;
  const g = c.getContext('2d');
  g.fillStyle = '#fff';
  GLYPHS.forEach((rows, k) => rows.forEach((row, y) => [...row].forEach((ch, x) => {
    if (ch === '#') g.fillRect(k * 8 + x, y, 1, 1);
  })));
  glyphAtlas = new THREE.CanvasTexture(c);
  glyphAtlas.magFilter = glyphAtlas.minFilter = THREE.NearestFilter;
  glyphAtlas.generateMipmaps = false;
  return glyphAtlas;
}

// Points for one aura, or null for an unknown id.
export function createAura(id) {
  const def = AURAS[id];
  if (!def) return null;
  const n = def.count;
  const seed = new Float32Array(n * 4), color = new Float32Array(n * 3), size = new Float32Array(n);
  const col = new THREE.Color();
  for (let i = 0; i < n; i++) {
    for (let k = 0; k < 4; k++) seed[i * 4 + k] = Math.random();
    col.set(def.colors[Math.floor(Math.random() * def.colors.length)]);
    color.set([col.r, col.g, col.b], i * 3);
    size[i] = def.size[0] + Math.random() * (def.size[1] - def.size[0]);
  }
  const geo = new THREE.BufferGeometry();
  geo.setAttribute('position', new THREE.BufferAttribute(new Float32Array(n * 3), 3)); // unused, sets the draw count
  geo.setAttribute('aSeed', new THREE.BufferAttribute(seed, 4));
  geo.setAttribute('aColor', new THREE.BufferAttribute(color, 3));
  geo.setAttribute('aSize', new THREE.BufferAttribute(size, 1));
  const glyphs = def.glyphs || [0, 1];
  const mat = new THREE.ShaderMaterial({
    vertexShader, fragmentShader,
    uniforms: {
      uTime: { value: 0 }, uScale: { value: 1 }, uPeriod: { value: def.period }, uSwirl: { value: def.swirl || 0 },
      uMode: { value: MODE[def.mode] }, uShape: { value: SHAPE[def.shape] }, uRadius: { value: new THREE.Vector2(...def.radius) },
      uGlyphBase: { value: glyphs[0] }, uGlyphCount: { value: glyphs[1] }, uGlyphs: { value: glyphTexture() }
    },
    transparent: true,
    depthWrite: false,                // particles behind the player stay hidden by it, none hide each other
    blending: def.normalBlend ? THREE.NormalBlending : THREE.AdditiveBlending
  });
  const points = new THREE.Points(geo, mat);
  points.frustumCulled = false;       // positions only exist in the shader
  return points;
}
