import { resolve } from 'node:path';
const root=resolve(import.meta.dirname);
export default {root,resolve:{alias:{'@xterm/xterm':resolve(root,'../../../../apps/desktop/node_modules/@xterm/xterm'),'@xterm/addon-fit':resolve(root,'../../../../apps/desktop/node_modules/@xterm/addon-fit'),'@tauri-apps/api':resolve(root,'../../../../apps/desktop/node_modules/@tauri-apps/api')}},build:{outDir:'dist',emptyOutDir:true}};
