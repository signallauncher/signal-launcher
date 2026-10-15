// WebGL player model for the play/cosmetics stage. World units are CSS pixels and the camera copies
// the stage's CSS `perspective`, so the model stands exactly on the CSS pedestal that rotates with it.
import * as THREE from 'three';
import { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js';
import { RoomEnvironment } from 'three/addons/environments/RoomEnvironment.js';
import { createAura } from './aura-particles.js';

const DEG = Math.PI / 180;
// Cosmetic .glb files are authored at 1 skin pixel = 1/16 m with their origin on the bone pivot.
const GLB_SCALE = 16;
// Wardrobe slot -> bone socket. Face items ride on the head so they turn with it.
const SLOT_SOCKET = { hat: 'head', face: 'head', back: 'body', pet: 'shoulderL' };

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

function pixelTexture(url) {
  const tex = new THREE.TextureLoader().load(url);
  tex.colorSpace = THREE.SRGBColorSpace;
  tex.magFilter = tex.minFilter = THREE.NearestFilter;
  tex.generateMipmaps = false;
  return tex;
}

export class SkinViewer {
  constructor({ scale = 11, originY = 290, perspective = 1400 } = {}) {
    this.originY = originY;
    this.perspective = perspective;
    this.t = 0;
    this.pose = 'idle';
    this.animate = true;
    this.view = { angle: 0, tilt: -6, yaw: 0, pitch: 0 };
    this.loader = new GLTFLoader();
    this.glbCache = new Map();
    this.attached = {};   // slot -> { url, object }
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
    scene.add(new THREE.HemisphereLight(0xf2f5ff, 0x30363f, 1.6));
    const key = new THREE.DirectionalLight(0xfff4e8, 2.2); key.position.set(-300, 500, 700); scene.add(key);
    const rim = new THREE.DirectionalLight(0x9be8ae, 1.4); rim.position.set(250, 300, -600); scene.add(rim);
    this.scene = scene;
    this.camera = new THREE.PerspectiveCamera(30, 1, 10, 6000);

    this.root = new THREE.Group();
    this.root.scale.setScalar(scale);
    scene.add(this.root);
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
    if (!this.sockets) this.sockets = { head: new THREE.Group(), body: new THREE.Group(), cape: new THREE.Group(), shoulderL: new THREE.Group() };
    joints.head.add(this.sockets.head);                                            // neck pivot
    this.sockets.body.position.set(0, 6, 0); joints.body.add(this.sockets.body);   // neck, torso bone
    this.sockets.cape.position.set(0, 6, -2); joints.body.add(this.sockets.cape);  // top of the back
    // left shoulder perch on the torso (not the arm, so pets don't swing with it); SOCKET_shoulder_left in Blender
    this.sockets.shoulderL.position.set(7, 6, 0); joints.body.add(this.sockets.shoulderL);
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

  // Put a .glb into a wardrobe slot ('hat' | 'face' | 'back' | 'pet'); url null empties the slot.
  setAccessory(slot, url) {
    const cur = this.attached[slot];
    if ((cur ? cur.url : null) === url) return;
    const socket = this.sockets[SLOT_SOCKET[slot]];
    if (cur) { socket.remove(cur.object); delete this.attached[slot]; }
    const token = (this.loadToken[slot] || 0) + 1;
    this.loadToken[slot] = token;
    if (!url) return;
    this.attached[slot] = { url, object: new THREE.Group() };
    this.loadGlb(url).then((gltf) => {
      if (this.loadToken[slot] !== token) return;
      const obj = gltf.scene.clone(true);
      obj.scale.setScalar(GLB_SCALE);
      socket.add(obj);
      this.attached[slot] = { url, object: obj };
    }).catch((e) => console.warn('[skin-viewer] cannot load', url, e));
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
    this.renderer.render(this.scene, this.camera);
  }

  dispose() {
    if (this.ro) this.ro.disconnect();
    this.setCape(null);
    this.setAura(null);
    if (this.skinTex) this.skinTex.dispose();
    this.root.traverse((o) => { if (o.isMesh && o.userData.skinPart) o.geometry.dispose(); });
    this.baseMat.dispose(); this.layerMat.dispose();
    if (this.scene.environment) this.scene.environment.dispose();
    this.renderer.dispose();
    this.renderer.domElement.remove();
  }
}
