// WebGL player model for the play/cosmetics stage. World units are CSS pixels and the camera copies
// the stage's CSS `perspective`, so the model stands exactly on the CSS pedestal that rotates with it.
import * as THREE from 'three';
import { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js';
import { RoomEnvironment } from 'three/addons/environments/RoomEnvironment.js';
import { createAura } from './aura-particles.js';

const DEG = Math.PI / 180;
// Cosmetic .glb files are authored at 1 skin pixel = 1/16 m with their origin on the bone pivot.
const GLB_SCALE = 16;
// Wardrobe slot -> bone socket for a bare url without an attach point. Face items ride on the head so they turn with it.
const SLOT_SOCKET = { hat: 'head', head: 'head', ears: 'head', face: 'head', pet: 'shoulder_left', shoulders: 'shoulder_left' };
// Catalog attach points (attach in src/catalog.items.json); each is a socket on the matching joint.
const ATTACH = ['head', 'body', 'cape', 'shoulder_left', 'arm_left', 'arm_right', 'leg_left', 'leg_right'];

// Box UVs in the 64x64 skin layout. BoxGeometry face order: +x, -x, +y, -y, +z, -z; the player faces +z,
// so +x is the player's left side and -x the right side.
function boxUv(geo, u, v, w, h, d) {
  const rects = [
    [u + d + w, v + d, d, h],     // +x  left
    [u, v + d, d, h],             // -x  right
    [u + d, v, w, d],             // +y  top
    [u + d + w, v, w, d],         // -y  bottom
    [u + d, v + d, w, h],         // +z  front
    [u + 2 * d + w, v + d, w, h]  // -z  back
  ];
  const uv = geo.attributes.uv;
  rects.forEach(([x, y, rw, rh], f) => {
    const x0 = x / 64, x1 = (x + rw) / 64, y0 = 1 - y / 64, y1 = 1 - (y + rh) / 64;
    const i = f * 4; // vertex order per face: top-left, top-right, bottom-left, bottom-right
    uv.setXY(i, x0, y0); uv.setXY(i + 1, x1, y0); uv.setXY(i + 2, x0, y1); uv.setXY(i + 3, x1, y1);
  });
  uv.needsUpdate = true;
  return geo;
}

// pivot: joint position relative to the body centre; offset: box centre relative to the joint (skin px, y up)
function partDefs(slim) {
  const aw = slim ? 3 : 4, ax = slim ? 5.5 : 6;
  return {
    head: { size: [8, 8, 8], uv: [0, 0], ov: [32, 0], inflate: 1, pivot: [0, 6, 0], offset: [0, 4, 0] },
    body: { size: [8, 12, 4], uv: [16, 16], ov: [16, 32], inflate: 0.5, pivot: [0, 0, 0], offset: [0, 0, 0] },
    rarm: { size: [aw, 12, 4], uv: [40, 16], ov: [40, 32], inflate: 0.5, pivot: [-ax, 5, 0], offset: [0, -5, 0] },
    larm: { size: [aw, 12, 4], uv: [32, 48], ov: [48, 48], inflate: 0.5, pivot: [ax, 5, 0], offset: [0, -5, 0] },
    rleg: { size: [4, 12, 4], uv: [0, 16], ov: [0, 32], inflate: 0.5, pivot: [-2, -6, 0], offset: [0, -6, 0] },
    lleg: { size: [4, 12, 4], uv: [16, 48], ov: [0, 48], inflate: 0.5, pivot: [2, -6, 0], offset: [0, -6, 0] }
  };
}

// Cape: 10 x 16 x 1 px, hinge at the origin, hanging down (-y) behind the back (-z). Extra rows and
// columns let it ripple. UVs follow the Minecraft 64x32 cape layout: outer face (1,1), inner (12,1),
// top (1,0), bottom (11,0), side edges (11,1) and (0,1).
function capeGeometry() {
  const geo = new THREE.BoxGeometry(10, 16, 1, 10, 16, 1);
  geo.translate(0, -8, -0.5);
  const pos = geo.attributes.position, nor = geo.attributes.normal, uv = geo.attributes.uv;
  for (let i = 0; i < pos.count; i++) {
    const x = pos.getX(i), y = pos.getY(i), z = pos.getZ(i);
    const nx = nor.getX(i), ny = nor.getY(i), nz = nor.getZ(i);
    let u, v;
    if (nz < -0.5) { u = 1 + (5 - x); v = 1 - y; }          // outer, seen from behind: left = +x
    else if (nz > 0.5) { u = 12 + (x + 5); v = 1 - y; }     // inner, against the back
    else if (ny > 0.5) { u = 1 + (5 - x); v = z + 1; }      // top
    else if (ny < -0.5) { u = 11 + (5 - x); v = z + 1; }    // hem
    else if (nx > 0.5) { u = 11 - z; v = 1 - y; }          // edge beside the outer's first column
    else { u = -z; v = 1 - y; }
    uv.setXY(i, u / 64, 1 - v / 32);
  }
  geo.userData.rest = Float32Array.from(pos.array);
  return geo;
}

// Soft contact shadow lying on the ground under the soles: an ellipse of black fading to nothing.
// The soles are 18 skin px below the body centre (rig origin); the shadow sits a hair lower. The camera
// looks at the ground almost edge-on (6° tilt), so the oval is long front to back to read as ~200×35 px.
function groundShadow() {
  const c = document.createElement('canvas');
  c.width = c.height = 128;
  const g = c.getContext('2d');
  const grad = g.createRadialGradient(64, 64, 0, 64, 64, 64);
  grad.addColorStop(0, 'rgba(0,0,0,0.5)');
  grad.addColorStop(0.45, 'rgba(0,0,0,0.32)');
  grad.addColorStop(1, 'rgba(0,0,0,0)');
  g.fillStyle = grad;
  g.fillRect(0, 0, 128, 128);
  const tex = new THREE.CanvasTexture(c);
  tex.colorSpace = THREE.SRGBColorSpace;
  const mesh = new THREE.Mesh(
    new THREE.PlaneGeometry(18, 30),
    new THREE.MeshBasicMaterial({ map: tex, transparent: true, depthWrite: false, toneMapped: false })
  );
  mesh.rotation.x = -Math.PI / 2;
  mesh.position.y = -18.05;
  mesh.renderOrder = -1;
  return mesh;
}

// Light moods for the backgrounds. `sunset` matches bg.jpg: a warm sun from the upper right (~41° up),
// a cool blue fill from the left so the shadow side keeps its volume, and the sky glowing behind.
// `neutral` is plain daylight for every other background.
const LIGHT_MOODS = {
  neutral: {
    env: 0.55,
    hemi: { sky: 0xf2f5ff, ground: 0x30363f, i: 1.6 },
    key: { c: 0xfff4e8, i: 2.2, p: [-300, 500, 700] },
    rim: { c: 0x9be8ae, i: 1.4, p: [250, 300, -600] },
    fill: { c: 0x4a5a78, i: 0, p: [-500, 120, 250] }
  },
  sunset: {
    env: 0.35,
    hemi: { sky: 0xffcf9a, ground: 0x262b38, i: 0.9 },
    key: { c: 0xffb259, i: 2.8, p: [420, 500, 380] },
    rim: { c: 0xff8a3d, i: 1.3, p: [150, 260, -600] },
    fill: { c: 0x4a5a78, i: 1.5, p: [-500, 120, 250] }
  }
};

function pixelTexture(url) {
  const tex = new THREE.TextureLoader().load(url);
  tex.colorSpace = THREE.SRGBColorSpace;
  tex.magFilter = tex.minFilter = THREE.NearestFilter;
  tex.generateMipmaps = false;
  return tex;
}

// A cloned glTF scene shares materials and textures with the cached original, so each flipbook material
// gets its own copy (and its own offset) before the strip is stepped. Returns [{ tex, frames, fps }].
function flipbooks(obj) {
  const out = [];
  obj.traverse((child) => {
    if (!child.isMesh) return;
    const one = (m) => {
      const frames = m.userData && m.userData.sl_frames;
      if (!(frames > 1)) return m;
      m = m.clone();
      ['map', 'emissiveMap'].forEach((k) => {
        if (!m[k]) return;
        m[k] = m[k].clone();
        out.push({ tex: m[k], frames, fps: m.userData.sl_fps || 12 });
      });
      return m;
    };
    child.material = Array.isArray(child.material) ? child.material.map(one) : one(child.material);
  });
  return out;
}

export class SkinViewer {
  // originY: rig origin (body centre) below the host's top; feetFromBottom instead keeps the soles that far
  // above the host's bottom whatever its height, and a host too short for the player shrinks the model so the
  // head (14 skin px above the origin) still clears the top.
  constructor({ scale = 11, originY = 290, feetFromBottom = null, perspective = 1400 } = {}) {
    this.originY = originY;
    this.feetFromBottom = feetFromBottom;
    this.scale = scale;
    this.maxScale = scale;
    this.perspective = perspective;
    this.t = 0;
    this.pose = 'idle';
    this.animate = true;
    this.view = { angle: 0, tilt: -6, yaw: 0, pitch: 0 };
    this.loader = new GLTFLoader();
    this.glbCache = new Map();
    this.attached = {};   // slot -> { key, objects: [{ socket, object }] }
    this.loadToken = {};

    const r = new THREE.WebGLRenderer({ antialias: true, alpha: true });
    r.setPixelRatio(Math.min(2, window.devicePixelRatio || 1));
    r.outputColorSpace = THREE.SRGBColorSpace;
    r.toneMapping = THREE.NeutralToneMapping;
    r.setClearColor(0x000000, 0);
    r.domElement.style.cssText = 'position:absolute;inset:0;width:100%;height:100%;pointer-events:none';
    this.renderer = r;

    const scene = new THREE.Scene();
    const pmrem = new THREE.PMREMGenerator(r);
    scene.environment = pmrem.fromScene(new RoomEnvironment(), 0.04).texture;
    scene.environmentIntensity = 0.55;
    pmrem.dispose();
    // sky light, key, rim from behind and a fill from the shadow side; setLighting() moves them to a mood
    const hemi = new THREE.HemisphereLight(0xf2f5ff, 0x30363f, 1.6); scene.add(hemi);
    const key = new THREE.DirectionalLight(0xfff4e8, 2.2); key.position.set(-300, 500, 700); scene.add(key);
    const rim = new THREE.DirectionalLight(0x9be8ae, 1.4); rim.position.set(250, 300, -600); scene.add(rim);
    const fill = new THREE.DirectionalLight(0x4a5a78, 0); fill.position.set(-500, 120, 250); scene.add(fill);
    this.lights = { hemi, key, rim, fill };
    this.lightMood = null;
    this.lightTarget = null;
    this.scene = scene;
    this.camera = new THREE.PerspectiveCamera(30, 1, 10, 6000);

    this.root = new THREE.Group();
    this.root.scale.setScalar(scale);
    scene.add(this.root);
    // on the rig root, not the body: it stays on the ground while the player breathes and walks
    this.shadow = groundShadow();
    this.root.add(this.shadow);
    this.skinTex = null;
    this.baseMat = new THREE.MeshLambertMaterial();
    this.layerMat = new THREE.MeshLambertMaterial({ alphaTest: 0.5, side: THREE.DoubleSide });
    this.slim = null;
    this.buildBody(false);
  }

  buildBody(slim) {
    if (this.slim === slim) return;
    this.slim = slim;
    const old = this.joints;
    if (old) {
      old.body.traverse((o) => { if (o.isMesh && o.userData.skinPart) o.geometry.dispose(); });
      this.root.remove(old.body);
    }
    const joints = {}, meshes = {}, layers = {};
    Object.entries(partDefs(slim)).forEach(([name, p]) => {
      const j = new THREE.Group();
      j.position.fromArray(p.pivot);
      const [w, h, d] = p.size;
      const k = p.inflate;
      const base = new THREE.Mesh(boxUv(new THREE.BoxGeometry(w, h, d), p.uv[0], p.uv[1], w, h, d), this.baseMat);
      const over = new THREE.Mesh(boxUv(new THREE.BoxGeometry(w + k, h + k, d + k), p.ov[0], p.ov[1], w, h, d), this.layerMat);
      base.position.fromArray(p.offset); over.position.fromArray(p.offset);
      base.userData.skinPart = over.userData.skinPart = true;
      j.add(base, over);
      joints[name] = j; meshes[name] = base; layers[name] = over;
    });
    // everything hangs off the body so the whole rig moves as one
    Object.keys(joints).forEach((n) => { if (n !== 'body') joints.body.add(joints[n]); });
    this.root.add(joints.body);
    // sockets survive a Classic/Slim rebuild together with whatever is attached to them
    if (!this.sockets) this.sockets = Object.fromEntries(ATTACH.map((a) => [a, new THREE.Group()]));
    const k = this.sockets;
    joints.head.add(k.head);                                            // neck pivot
    k.body.position.set(0, 6, 0); joints.body.add(k.body);              // neck, torso bone
    k.cape.position.set(0, 6, -2); joints.body.add(k.cape);             // top of the back
    // left shoulder perch on the torso (not the arm, so pets don't swing with it); SOCKET_shoulder_left in Blender
    k.shoulder_left.position.set(7, 6, 0); joints.body.add(k.shoulder_left);
    // Limb sockets sit on the vanilla rig pivots, which differ from this model's joints: arms (+-5, 22) for
    // classic and (+-5, 21.5) for slim, legs (+-1.9, 12); the body centre is at rig height 18.
    const ax = slim ? 0.5 : 1, ay = slim ? -1.5 : -1;
    k.arm_left.position.set(-ax, ay, 0); joints.larm.add(k.arm_left);
    k.arm_right.position.set(ax, ay, 0); joints.rarm.add(k.arm_right);
    k.leg_left.position.set(-0.1, 0, 0); joints.lleg.add(k.leg_left);
    k.leg_right.position.set(0.1, 0, 0); joints.rleg.add(k.leg_right);
    this.joints = joints; this.meshes = meshes; this.layers = layers;
    this.setLayerVisible(this.showLayer !== false);
  }

  mount(host) {
    if (this.host === host) return;
    if (this.ro) this.ro.disconnect();
    this.host = host;
    host.appendChild(this.renderer.domElement);
    this.ro = new ResizeObserver(() => this.resize());
    this.ro.observe(host);
    this.resize();
  }

  resize() {
    if (!this.host) return;
    const w = this.host.clientWidth, h = this.host.clientHeight;
    if (!w || !h) return;
    this.renderer.setSize(w, h, false);
    const cam = this.camera;
    cam.aspect = w / h;
    cam.fov = 2 * Math.atan(h / 2 / this.perspective) / DEG;
    if (this.feetFromBottom != null) {
      this.scale = Math.max(4, Math.min(this.maxScale, (h - this.feetFromBottom - 24) / 32));
      this.root.scale.setScalar(this.scale);
      this.originY = h - this.feetFromBottom - 18 * this.scale;
    }
    // CSS projects toward the stage centre; the model origin sits originY px below the stage top
    cam.position.set(0, this.originY - h / 2, this.perspective);
    cam.updateProjectionMatrix();
    // screen pixels per world unit at distance 1, for the aura's gl_PointSize
    this.pointScale = this.renderer.getPixelRatio() * h / 2 / Math.tan(cam.fov * DEG / 2);
  }

  setSkin(url, slim) {
    this.buildBody(!!slim);
    if (url === this.skinUrl) return;
    this.skinUrl = url;
    new THREE.TextureLoader().load(url, (tex) => {
      if (url !== this.skinUrl) { tex.dispose(); return; }
      tex.colorSpace = THREE.SRGBColorSpace;
      tex.magFilter = tex.minFilter = THREE.NearestFilter;
      tex.generateMipmaps = false;
      if (this.skinTex) this.skinTex.dispose();
      this.skinTex = tex;
      this.baseMat.map = tex; this.layerMat.map = tex;
      this.baseMat.needsUpdate = this.layerMat.needsUpdate = true;
    });
  }

  setLayerVisible(on) {
    this.showLayer = on;
    if (this.layers) Object.values(this.layers).forEach((m) => { m.visible = on; });
  }

  loadGlb(url) {
    if (!this.glbCache.has(url)) {
      this.glbCache.set(url, new Promise((resolve, reject) => this.loader.load(url, resolve, undefined, reject)));
    }
    return this.glbCache.get(url);
  }

  // Put an item into a wardrobe slot. parts: [{ attach, url }] (one .glb per bone, e.g. boots on both legs),
  // or a bare url that goes on the slot's default socket; null empties the slot.
  setAccessory(slot, parts) {
    if (typeof parts === 'string') parts = [{ attach: SLOT_SOCKET[slot] || 'body', url: parts }];
    parts = (parts || []).filter((p) => p && p.url);
    const key = parts.map((p) => p.attach + '=' + p.url).join('|') || null;
    const cur = this.attached[slot];
    if ((cur ? cur.key : null) === key) return;
    if (cur) cur.objects.forEach(({ socket, object }) => socket.remove(object));
    delete this.attached[slot];

    const token = (this.loadToken[slot] || 0) + 1;
    this.loadToken[slot] = token;
    if (!key) return;
    this.attached[slot] = { key, objects: [] };
    // all parts appear together, so a pair of boots never shows up one leg at a time
    Promise.all(parts.map((p) => this.loadGlb(p.url))).then((gltfs) => {
      if (this.loadToken[slot] !== token) return;
      const objects = gltfs.map((gltf, i) => {
        const socket = this.sockets[parts[i].attach] || this.sockets.body;
        const object = gltf.scene.clone(true);
        object.scale.setScalar(GLB_SCALE);
        object.userData.flipbooks = flipbooks(object);
        socket.add(object);
        return { socket, object };
      });
      this.attached[slot] = { key, objects };
    }).catch((e) => console.warn('[skin-viewer] cannot load', key, e));
  }

  // Flipbook strips (material extras sl_frames/sl_fps written by the cosmetics exporter): UVs sit on the first frame,
  // the texture offset steps one frame down the strip. glTF textures are not flipped, so +v goes down the image.
  stepFlipbooks(t) {
    for (const slot in this.attached) {
      this.attached[slot].objects.forEach(({ object }) => {
        object.userData.flipbooks.forEach(({ tex, frames, fps }) => {
          tex.offset.y = (Math.floor(t * fps) % frames) / frames;
        });
      });
    }
  }

  // spec: { key, texture: 64x32 cape png, emissive: matching glow map or null } or null
  setCape(spec) {
    const key = spec ? spec.key : null;
    if (key === this.capeKey) return;
    this.capeKey = key;
    if (this.capeHinge) {
      this.sockets.cape.remove(this.capeHinge);
      const mesh = this.capeHinge.children[0];
      mesh.geometry.dispose();
      ['map', 'emissiveMap'].forEach((k) => { if (mesh.material[k]) mesh.material[k].dispose(); });
      mesh.material.dispose();
      this.capeHinge = null;
    }
    if (!spec) return;
    const mat = new THREE.MeshStandardMaterial({ map: pixelTexture(spec.texture), roughness: 0.85, alphaTest: 0.5 });
    if (spec.emissive) {
      mat.emissiveMap = pixelTexture(spec.emissive);
      mat.emissive = new THREE.Color(0xffffff);
      mat.emissiveIntensity = 1.2;
    }
    this.capeHinge = new THREE.Group();
    this.capeHinge.add(new THREE.Mesh(capeGeometry(), mat));
    this.sockets.cape.add(this.capeHinge);
  }

  // Travelling wave down the cloth: stronger toward the hem, paced by the breath or the walk cycle.
  rippleCape(t, walking) {
    const geo = this.capeHinge.children[0].geometry;
    const pos = geo.attributes.position, rest = geo.userData.rest;
    const amp = walking ? 1.3 : 0.75;
    const w = Math.PI * 2 / (walking ? 0.55 : 1.8);
    for (let i = 0; i < pos.count; i++) {
      const x = rest[i * 3], y = rest[i * 3 + 1], z = rest[i * 3 + 2];
      const d = -y / 16;
      const wave = Math.sin(t * w - d * 4.2 + x * 0.35);
      pos.setXYZ(i, x + 0.15 * d * Math.sin(t * w * 0.5), y, z - amp * Math.pow(d, 1.5) * wave);
    }
    pos.needsUpdate = true;
    geo.computeVertexNormals();
  }

  // Aura id from aura-particles.js, or null.
  setAura(id) {
    if (id === this.auraId) return;
    this.auraId = id;
    if (this.aura) {
      this.root.remove(this.aura);
      this.aura.geometry.dispose();
      this.aura.material.dispose();
    }
    this.aura = id ? createAura(id) : null;
    if (this.aura) this.root.add(this.aura);   // child of the rig root: it turns with the player, not with the breath
  }

  // 'sunset' | 'neutral' (LIGHT_MOODS); the lights glide there over a few frames, the first one is set at once.
  setLighting(mood) {
    const m = LIGHT_MOODS[mood] || LIGHT_MOODS.neutral;
    if (m === this.lightTarget) return;
    const first = !this.lightTarget;
    this.lightTarget = m;
    this.lightMood = mood;
    if (first) this.stepLights(1);
  }

  stepLights(k) {
    const m = this.lightTarget, L = this.lights;
    if (!m) return;
    const tmp = new THREE.Color();
    L.hemi.color.lerp(tmp.set(m.hemi.sky), k);
    L.hemi.groundColor.lerp(tmp.set(m.hemi.ground), k);
    L.hemi.intensity += (m.hemi.i - L.hemi.intensity) * k;
    ['key', 'rim', 'fill'].forEach((n) => {
      const l = L[n], t = m[n];
      l.color.lerp(tmp.set(t.c), k);
      l.intensity += (t.i - l.intensity) * k;
      l.position.x += (t.p[0] - l.position.x) * k;
      l.position.y += (t.p[1] - l.position.y) * k;
      l.position.z += (t.p[2] - l.position.z) * k;
    });
    this.scene.environmentIntensity += (m.env - this.scene.environmentIntensity) * k;
  }

  setView(angle, tilt, yaw, pitch) {
    this.view = { angle, tilt, yaw, pitch };
  }

  setPose(pose) { this.pose = pose === 'walk' ? 'walk' : 'idle'; }

  setAnimate(on) { this.animate = !!on; }

  render(dt) {
    if (!this.host || !this.host.isConnected) return;
    if (this.animate) this.t += dt / 1000;
    const t = this.t, v = this.view, j = this.joints;
    // CSS rotateX(a) is three's rotation.x = -a (y points the other way); rotateY keeps its sign
    this.root.rotation.set(-v.tilt * DEG, v.angle * DEG, 0, 'XYZ');
    j.head.rotation.set(-v.pitch * DEG, v.yaw * DEG, 0, 'YXZ');
    // same 3.6 s ease-in-out cycle as the old CSS idle: 0 → 1 → 0
    const breath = (1 - Math.cos(t * Math.PI * 2 / 3.6)) / 2;
    this.meshes.body.scale.set(1 + 0.012 * breath, 1 + 0.01 * breath, 1 + 0.07 * breath);
    let swing = 0, capeLift = 10;
    if (this.pose === 'walk') {
      swing = Math.sin(t * Math.PI * 2 / 1.1) * 30;
      capeLift = 24 + Math.abs(swing) * 0.25;
    }
    j.rarm.rotation.set((swing + 2.5 * breath) * DEG, 0, -3.5 * breath * DEG);
    j.larm.rotation.set((-swing - 2.5 * breath) * DEG, 0, 3.5 * breath * DEG);
    j.rleg.rotation.set(-swing * DEG, 0, 0);
    j.lleg.rotation.set(swing * DEG, 0, 0);
    if (this.capeHinge) {
      this.capeHinge.rotation.x = (capeLift + 6 * breath) * DEG;
      if (this.animate) this.rippleCape(t, this.pose === 'walk');
    }
    if (this.aura) {
      const u = this.aura.material.uniforms;
      u.uTime.value = t;
      u.uScale.value = this.pointScale || 1;
    }
    this.stepFlipbooks(t);
    this.stepLights(1 - Math.exp(-(dt || 16) / 250));
    this.renderer.render(this.scene, this.camera);
  }

  dispose() {
    if (this.ro) this.ro.disconnect();
    this.setCape(null);
    this.setAura(null);
    if (this.skinTex) this.skinTex.dispose();
    this.root.traverse((o) => { if (o.isMesh && o.userData.skinPart) o.geometry.dispose(); });
    this.baseMat.dispose(); this.layerMat.dispose();
    this.shadow.geometry.dispose(); this.shadow.material.map.dispose(); this.shadow.material.dispose();
    if (this.scene.environment) this.scene.environment.dispose();
    this.renderer.dispose();
    this.renderer.domElement.remove();
  }
}
