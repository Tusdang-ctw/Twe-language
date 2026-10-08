// The two procedural materials from action3d's `visual` blocks, rebuilt
// in Three.js. In Twe these are `surface(uv, time, pos) -> material`
// blocks compiled to type-checked WGSL. Three.js has no first-class
// procedural-surface hook, so the idiomatic route is to patch a
// MeshStandardMaterial via onBeforeCompile with raw GLSL strings — which
// keeps the PBR + IBL + shadow chunks but is stringly-typed and tied to
// three's internal #include names. That friction is part of the case study.
import * as THREE from 'three'

// Shared value-noise GLSL (Twe's `noise(vec2)` builtin has no Three.js
// equivalent, so we supply one).
const NOISE_GLSL = /* glsl */ `
  float hash21(vec2 p) {
    p = fract(p * vec2(123.34, 456.21));
    p += dot(p, p + 45.32);
    return fract(p.x * p.y);
  }
  float valueNoise(vec2 p) {
    vec2 i = floor(p); vec2 f = fract(p);
    float a = hash21(i);
    float b = hash21(i + vec2(1.0, 0.0));
    float c = hash21(i + vec2(0.0, 1.0));
    float d = hash21(i + vec2(1.0, 1.0));
    vec2 u = f * f * (3.0 - 2.0 * f);
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
  }
`

// Ground: noise-blended grass with a worn dirt path down the middle.
// Twe original:
//   blade = noise(pos.xz*1.3)*0.5 + noise(pos.xz*5)*0.25
//   path  = 1 - smoothstep(1.5, 3.0, abs(pos.x))
//   albedo = mix(mix(green0, green1, blade), dirt, path*0.7); roughness 0.9
export function makeGroundMaterial() {
  const mat = new THREE.MeshStandardMaterial({ roughness: 0.9, metalness: 0.0 })
  mat.onBeforeCompile = (shader) => {
    shader.vertexShader = shader.vertexShader
      .replace('#include <common>', '#include <common>\nvarying vec3 vWorldPos;')
      .replace(
        '#include <worldpos_vertex>',
        '#include <worldpos_vertex>\n  vWorldPos = (modelMatrix * vec4(transformed, 1.0)).xyz;'
      )
    shader.fragmentShader = shader.fragmentShader
      .replace('#include <common>', '#include <common>\nvarying vec3 vWorldPos;\n' + NOISE_GLSL)
      .replace(
        '#include <map_fragment>',
        /* glsl */ `
        vec2 gp = vWorldPos.xz;
        float blade = valueNoise(gp * 1.3) * 0.5 + valueNoise(gp * 5.0) * 0.25;
        float path = 1.0 - smoothstep(1.5, 3.0, abs(vWorldPos.x));
        vec3 green = mix(vec3(0.13, 0.28, 0.12), vec3(0.18, 0.36, 0.15), blade);
        vec3 dirt = vec3(0.32, 0.26, 0.18);
        diffuseColor.rgb = mix(green, dirt, path * 0.7);
        `
      )
  }
  return mat
}

// Enemy: a red blob with a pulsing emissive core. Twe computes the pulse
// in-shader from `time` + position; here the per-frame pulse is driven
// from JS (see updateEnemyPulse) because each blob needs its own phase and
// MeshStandardMaterial already exposes emissiveIntensity. The noise mottle
// is injected in GLSL to keep the surface procedural.
export function makeEnemyMaterial() {
  const mat = new THREE.MeshStandardMaterial({
    color: new THREE.Color(0.7, 0.12, 0.14),
    roughness: 0.3,
    emissive: new THREE.Color(0.5, 0.04, 0.05),
    emissiveIntensity: 0.5,
  })
  mat.onBeforeCompile = (shader) => {
    shader.vertexShader = shader.vertexShader
      .replace('#include <common>', '#include <common>\nvarying vec3 vLocalPos;')
      .replace('#include <begin_vertex>', '#include <begin_vertex>\n  vLocalPos = position;')
    shader.fragmentShader = shader.fragmentShader
      .replace('#include <common>', '#include <common>\nvarying vec3 vLocalPos;\n' + NOISE_GLSL)
      .replace(
        '#include <map_fragment>',
        /* glsl */ `
        #include <map_fragment>
        float mottle = valueNoise(vLocalPos.xy * 7.0) * 0.12;
        diffuseColor.rgb += vec3(mottle, 0.0, 0.0);
        `
      )
  }
  return mat
}

// Advance an enemy's emissive pulse (Twe: 0.5 + 0.5*sin(time*4 + x + z)).
export function updateEnemyPulse(mesh, time) {
  const p = mesh.position
  mesh.material.emissiveIntensity = 0.5 + 0.5 * Math.sin(time * 4.0 + p.x + p.z)
}
