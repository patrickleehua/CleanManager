import { defineConfig } from 'vite';

// Tauri serves the built files from ../dist; dev uses a fixed port the host expects.
export default defineConfig({
  clearScreen: false,
  server: { port: 1420, strictPort: true, host: '127.0.0.1', watch: { ignored: ['**/src-tauri/**'] } },
  envPrefix: ['VITE_'],
  build: {
    target: 'es2022',
    outDir: 'dist',
    emptyOutDir: true,
    sourcemap: false,
    // Keep everything as files so the strict CSP (script-src/style-src 'self') holds.
    assetsInlineLimit: 0,
  },
});
