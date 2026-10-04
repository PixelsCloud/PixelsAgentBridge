import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { readFileSync } from 'node:fs';

// TLS certificates are supplied locally, never copied into the production bundle.
export default defineConfig({
  plugins: [react()],
  server: {
    port: 1440,
    strictPort: true,
    https: process.env.PAB_WEB_DEV_CERT && process.env.PAB_WEB_DEV_KEY ? {
      cert: readFileSync(process.env.PAB_WEB_DEV_CERT), key: readFileSync(process.env.PAB_WEB_DEV_KEY),
    } : undefined,
    proxy: {
      '/api': {
        target: process.env.PAB_WEB_BACKEND ?? 'https://localhost:8443',
        ws: true,
        // Only explicit local development opt-in allows a self-signed backend.
        secure: process.env.PAB_WEB_DEV_SELF_SIGNED !== '1',
      },
    },
  },
});
