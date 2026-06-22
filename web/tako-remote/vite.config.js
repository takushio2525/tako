import { defineConfig } from 'vite';
import preact from '@preact/preset-vite';

export default defineConfig({
  plugins: [preact()],
  server: {
    port: 5174,
    host: true,
  },
  build: {
    outDir: 'dist',
  },
});
