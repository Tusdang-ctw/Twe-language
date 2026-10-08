import { defineConfig } from 'vite'

// The game is a single page served from index.html; assets in public/ are
// served at the site root (/assets/hero.glb). `base: './'` makes the built
// bundle work from any sub-path (parity with Twe's hashed web build).
export default defineConfig({
  base: './',
  build: { target: 'esnext', outDir: 'dist' },
})
