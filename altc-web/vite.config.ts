import { svelte } from '@sveltejs/vite-plugin-svelte'
import { defineConfig } from 'vite'

export default defineConfig({
  plugins: [svelte()],
  build: {
    // Emitted into the crate so the Rust binary can embed it: the UI has to be
    // available at first boot with no network and nothing to fetch.
    outDir: '../altc/web-dist',
    emptyOutDir: true,
  },
  server: {
    // `npm run dev` talks to a real agent instead of needing a rebuild to see
    // a change. The agent serves the API on 47990 over a self-signed cert.
    proxy: {
      '/api': { target: 'https://127.0.0.1:47990', changeOrigin: true, secure: false },
    },
  },
})
