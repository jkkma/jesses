import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import tailwindcss from '@tailwindcss/vite';
import { fileURLToPath, URL } from 'node:url';

export default defineConfig({
  plugins: [tailwindcss(), svelte()],
  resolve: { alias: { $lib: fileURLToPath(new URL('./src/lib', import.meta.url)) } },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: [
        '**/src-tauri/**',
        '**/crates/**',
        '**/target/**',
        '**/.validation/**',
        '**/.jesses-job-*/**',
      ],
    },
  },
  build: { target: ['es2022', 'chrome111', 'safari16.4'] },
});
