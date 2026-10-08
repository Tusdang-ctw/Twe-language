// action3d, in Three.js. A faithful re-implementation of
// examples/action3d/main.twe for the web3d-M12 same-game case study:
// a 3rd-person hero with blend-space locomotion + a one-shot sword
// attack, 8 chasing enemies, a spherical-joint rope bridge, IBL + sun
// shadows + AO + bloom, and a chase camera. Same engine under the
// physics (rapier) as Twe, so that half is a like-for-like comparison.
import * as THREE from 'three'
import { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js'
import { RGBELoader } from 'three/addons/loaders/RGBELoader.js'
import { EffectComposer } from 'three/addons/postprocessing/EffectComposer.js'
import { RenderPass } from 'three/addons/postprocessing/RenderPass.js'
import { GTAOPass } from 'three/addons/postprocessing/GTAOPass.js'
import { UnrealBloomPass } from 'three/addons/postprocessing/UnrealBloomPass.js'
import { OutputPass } from 'three/addons/postprocessing/OutputPass.js'
import RAPIER from '@dimforge/rapier3d-compat'
import { makeGroundMaterial, makeEnemyMaterial, updateEnemyPulse } from './materials.js'

const WALK_SPEED = 3.2
const RUN_SPEED = 6.5

// ---- renderer / scene / camera ----
const app = document.getElementById('app')
const renderer = new THREE.WebGLRenderer({ antialias: true })
renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2))
renderer.setSize(window.innerWidth, window.innerHeight)
renderer.shadowMap.enabled = true
renderer.shadowMap.type = THREE.PCFSoftShadowMap
renderer.toneMapping = THREE.ACESFilmicToneMapping
renderer.toneMappingExposure = 1.0
app.appendChild(renderer.domElement)

const scene = new THREE.Scene()
const camera = new THREE.PerspectiveCamera(55, window.innerWidth / window.innerHeight, 0.1, 500)
camera.position.set(0, 4, 12)

// ---- IBL from sky.hdr (ambient + backdrop at 0.55) ----
const pmrem = new THREE.PMREMGenerator(renderer)
new RGBELoader().load('assets/sky.hdr', (hdr) => {
  hdr.mapping = THREE.EquirectangularReflectionMapping
  const env = pmrem.fromEquirectangular(hdr).texture
  scene.environment = env
  scene.background = env
  scene.environmentIntensity = 0.55
  scene.backgroundIntensity = 0.55
  hdr.dispose()
  pmrem.dispose()
})

// ---- sun + shadows (direction toward the sun = (-0.4, 0.85, 0.3), extent 20) ----
const sun = new THREE.DirectionalLight(0xfff4e6, 3.0)
sun.position.set(-0.4, 0.85, 0.3).normalize().multiplyScalar(30)
sun.castShadow = true
sun.shadow.mapSize.set(2048, 2048)
Object.assign(sun.shadow.camera, { left: -20, right: 20, top: 20, bottom: -20, near: 1, far: 80 })
sun.shadow.camera.updateProjectionMatrix()
sun.shadow.bias = -0.0005
scene.add(sun)
scene.add(sun.target)

// ---- ground: a 96x96 plane at y=-0.5 with the procedural grass material ----
const ground = new THREE.Mesh(new THREE.PlaneGeometry(96, 96), makeGroundMaterial())
ground.rotation.x = -Math.PI / 2
ground.position.y = -0.5
ground.receiveShadow = true
scene.add(ground)

// ---- hero state (mirrors the Twe globals) ----
let heroX = 0
let heroZ = 6
let heroHeading = 0
let speedNow = 0
let attacking = 0
let attackHit = false
let kills = 0
let camX = 0
let camY = 4
let camZ = 12

// ---- hero mesh + animation ----
let heroRoot = null
let mixer = null
const actions = {}
const LOCO = ['idle', 'walk', 'run']
new GLTFLoader().load('assets/hero.glb', (gltf) => {
  heroRoot = gltf.scene
  heroRoot.traverse((o) => {
    if (o.isMesh) {
      o.castShadow = true
      o.frustumCulled = false
    }
  })
  scene.add(heroRoot)
  mixer = new THREE.AnimationMixer(heroRoot)
  for (const clip of gltf.animations) actions[clip.name] = mixer.clipAction(clip)
  // Keep the locomotion clips playing; the blend space drives their weights.
  for (const name of LOCO) {
    const a = actions[name]
    if (a) {
      a.play()
      a.setEffectiveWeight(name === 'idle' ? 1 : 0)
    }
  }
})

// Twe does this with ONE call: mesh_anim.blend_space(hero, clips, stops, value).
// Three.js has no blend space, so we weight the AnimationActions by hand,
// keep the walk/run clips phase-synced (the mixer does not), and juggle the
// one-shot attack against the loco weights.
function driveBlendSpace(value) {
  const stops = [0, WALK_SPEED, RUN_SPEED]
  let lo
  let hi
  let t
  if (value <= stops[0]) {
    lo = 0
    hi = 0
    t = 0
  } else if (value >= stops[2]) {
    lo = 2
    hi = 2
    t = 0
  } else {
    const i = value > stops[1] ? 1 : 0
    const span = stops[i + 1] - stops[i]
    t = span > 0 ? (value - stops[i]) / span : 0
    lo = i
    hi = i + 1
  }
  const w = [0, 0, 0]
  if (lo === hi) w[lo] = 1
  else {
    w[lo] = 1 - t
    w[hi] = t
  }
  for (let i = 0; i < 3; i++) actions[LOCO[i]]?.setEffectiveWeight(w[i])
  // Phase-sync walk & run so the gaits do not foot-slide against each other.
  if (actions.walk && actions.run) actions.run.time = actions.walk.time
}

function startAttack() {
  const a = actions.attack
  if (!a) return
  a.reset()
  a.setLoop(THREE.LoopOnce, 1)
  a.clampWhenFinished = true
  a.setEffectiveWeight(1)
  a.play()
  for (const n of LOCO) actions[n]?.setEffectiveWeight(0)
}

// ---- enemies: 8 blobs in a ring that chase the hero ----
const enemies = []
const enemyGeo = new THREE.SphereGeometry(0.8, 24, 16)
for (let i = 0; i < 8; i++) {
  const a = (i / 8) * Math.PI * 2
  const m = new THREE.Mesh(enemyGeo, makeEnemyMaterial())
  m.position.set(Math.sin(a) * 12, 0, Math.cos(a) * 12)
  m.castShadow = true
  m.userData = { hp: 2, hurt: 0 }
  scene.add(m)
  enemies.push(m)
}

function dist2ToHero(x, z) {
  const dx = x - heroX
  const dz = z - heroZ
  return dx * dx + dz * dz
}

// ---- physics rope bridge (rapier — the same engine Twe uses) ----
let world = null
const planks = []
let midPlank = null
await RAPIER.init()
{
  world = new RAPIER.World({ x: 0, y: -9.81, z: 0 })
  const n = 9
  const span = 11.0
  const by = 3.4
  const bz = -8.0
  const seg = span / n
  const edge = seg / 2
  const post = (x) => {
    const b = world.createRigidBody(RAPIER.RigidBodyDesc.fixed().setTranslation(x, by, bz))
    world.createCollider(RAPIER.ColliderDesc.cuboid(0.3, 0.6, 1.4), b)
    return b
  }
  const left = post(-span / 2 - 0.3)
  const right = post(span / 2 + 0.3)
  const plankMat = new THREE.MeshStandardMaterial({ color: 0x8c6138, roughness: 0.85 })
  let prev = left
  for (let i = 0; i < n; i++) {
    const px = -span / 2 + (i + 0.5) * seg
    const body = world.createRigidBody(RAPIER.RigidBodyDesc.dynamic().setTranslation(px, by, bz))
    world.createCollider(RAPIER.ColliderDesc.cuboid(edge * 0.9, 0.1, 0.7).setDensity(1.0), body)
    world.createImpulseJoint(
      RAPIER.JointData.spherical({ x: edge, y: 0, z: 0 }, { x: -edge, y: 0, z: 0 }),
      prev,
      body,
      true
    )
    const mesh = new THREE.Mesh(new THREE.BoxGeometry(seg * 0.9, 0.2, 1.4), plankMat)
    mesh.castShadow = true
    mesh.receiveShadow = true
    scene.add(mesh)
    planks.push({ body, mesh })
    prev = body
  }
  world.createImpulseJoint(
    RAPIER.JointData.spherical({ x: edge, y: 0, z: 0 }, { x: -edge, y: 0, z: 0 }),
    prev,
    right,
    true
  )
  midPlank = planks[planks.length >> 1].body
}

// ---- input (held + edge-triggered, as the Twe key.* / key_pressed split) ----
const held = new Set()
const pressed = new Set()
let mousePressLeft = false
window.addEventListener('keydown', (e) => {
  const k = e.key.toLowerCase()
  if (!e.repeat) pressed.add(k)
  held.add(k)
})
window.addEventListener('keyup', (e) => held.delete(e.key.toLowerCase()))
window.addEventListener('mousedown', (e) => {
  if (e.button === 0) mousePressLeft = true
})
const down = (...keys) => keys.some((k) => held.has(k))

// ---- post: AO + bloom (TAA off), matching postfx.ao(0.9)/bloom(0.3) ----
const composer = new EffectComposer(renderer)
composer.addPass(new RenderPass(scene, camera))
const gtao = new GTAOPass(scene, camera, window.innerWidth, window.innerHeight)
gtao.output = GTAOPass.OUTPUT.Default
composer.addPass(gtao)
composer.addPass(new UnrealBloomPass(new THREE.Vector2(window.innerWidth, window.innerHeight), 0.3, 0.4, 0.85))
composer.addPass(new OutputPass())

const killsEl = document.getElementById('kills')
const statusEl = document.getElementById('status')
statusEl.textContent = 'ready'

// ---- the game loop ----
const _q = new THREE.Quaternion()
function update(dt) {
  // Movement input (world XZ).
  let mx = 0
  let mz = 0
  if (down('w', 'arrowup')) mz -= 1
  if (down('s', 'arrowdown')) mz += 1
  if (down('a', 'arrowleft')) mx -= 1
  if (down('d', 'arrowright')) mx += 1
  const running = down('shift')
  const mag = Math.hypot(mx, mz)

  // Attack: one-shot, locks locomotion while it plays.
  if (attacking <= 0 && (pressed.has(' ') || mousePressLeft)) {
    attacking = 0.45
    startAttack()
  }
  if (attacking > 0) attacking -= dt
  attackHit = attacking > 0.12 && attacking < 0.3

  // Move + face.
  let targetSpeed = 0
  if (mag > 0.01 && attacking <= 0) {
    const spd = running ? RUN_SPEED : WALK_SPEED
    heroX += (mx / mag) * spd * dt
    heroZ += (mz / mag) * spd * dt
    heroHeading = Math.atan2(mx, mz)
    targetSpeed = spd
  }
  speedNow += (targetSpeed - speedNow) * Math.min(1, dt * 10)

  // Locomotion blend space (unless an attack is playing).
  if (attacking <= 0) {
    if (actions.attack) actions.attack.setEffectiveWeight(0)
    driveBlendSpace(speedNow)
  }

  if (heroRoot) {
    heroRoot.position.set(heroX, -0.5, heroZ)
    heroRoot.quaternion.setFromAxisAngle(new THREE.Vector3(0, 1, 0), heroHeading)
  }

  // Enemies chase; a swing in range kills.
  for (let i = enemies.length - 1; i >= 0; i--) {
    const e = enemies[i]
    const d = e.userData
    const dx = heroX - e.position.x
    const dz = heroZ - e.position.z
    const m = Math.hypot(dx, dz)
    if (m > 0.9) {
      const s = (2.0 * dt) / m
      e.position.set(e.position.x + dx * s, 0, e.position.z + dz * s)
    }
    if (d.hurt > 0) d.hurt -= dt
    if (attackHit && dist2ToHero(e.position.x, e.position.z) < 3.2 && d.hurt <= 0) {
      d.hp -= 1
      d.hurt = 0.3
      if (d.hp <= 0) {
        scene.remove(e)
        enemies.splice(i, 1)
        kills += 1
        killsEl.textContent = String(kills)
      }
    }
  }
  for (const e of enemies) updateEnemyPulse(e, performance.now() / 1000)

  // Chase camera (smoothed, behind the heading).
  const dist = 7.0
  const wantX = heroX - Math.sin(heroHeading) * dist
  const wantZ = heroZ - Math.cos(heroHeading) * dist
  const k = Math.min(1, dt * 5)
  camX += (wantX - camX) * k
  camZ += (wantZ - camZ) * k
  camY += (4.5 - camY) * k
  camera.position.set(camX, camY, camZ)
  camera.lookAt(heroX, 1.0, heroZ)

  // Shove the bridge.
  if (pressed.has('e') && midPlank) midPlank.applyImpulse({ x: 0, y: 4, z: 3 }, true)

  // Step physics and sync the plank meshes.
  if (world) {
    world.step()
    for (const p of planks) {
      const t = p.body.translation()
      const r = p.body.rotation()
      p.mesh.position.set(t.x, t.y, t.z)
      p.mesh.quaternion.copy(_q.set(r.x, r.y, r.z, r.w))
    }
  }
}

let last = performance.now() / 1000
renderer.setAnimationLoop(() => {
  const now = performance.now() / 1000
  const dt = Math.min(now - last, 1 / 20)
  last = now
  update(dt)
  if (mixer) mixer.update(dt)
  composer.render()
  pressed.clear()
  mousePressLeft = false
  if (window.__captureReady) window.__captureReady()
})

window.addEventListener('resize', () => {
  camera.aspect = window.innerWidth / window.innerHeight
  camera.updateProjectionMatrix()
  renderer.setSize(window.innerWidth, window.innerHeight)
  composer.setSize(window.innerWidth, window.innerHeight)
})
