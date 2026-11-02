import { execFileSync } from 'node:child_process';
import { rmSync } from 'node:fs';
import { join, relative } from 'node:path';
import { defineConfig } from 'vite';

// A release ships what is committed. Local cosmetic packs (git-ignored files in public/, such as bb_*) and
// local edits of server/catalog.items.json stay on this computer; `npm run dev` still shows them.
function committedOnly() {
  let outDir = 'dist';
  const git = (...args) => execFileSync('git', args, { encoding: 'utf8', maxBuffer: 64 << 20 });
  return {
    name: 'signal-committed-only',
    apply: 'build',
    enforce: 'pre',
    configResolved(config) {
      outDir = config.build.outDir;
    },
    load(id) {
      if (!id.replace(/\\/g, '/').endsWith('/server/catalog.items.json')) return null;
      try {
        return git('show', 'HEAD:server/catalog.items.json');
      } catch (e) {
        return null; // no git (a source archive): the file as it is
      }
    },
    closeBundle() {
      let ignored;
      try {
        ignored = git('ls-files', '--others', '--ignored', '--exclude-standard', '-z', 'public').split('\0').filter(Boolean);
      } catch (e) {
        return;
      }
      ignored.forEach((f) => rmSync(join(outDir, relative('public', f)), { force: true }));
      if (ignored.length) console.log(`signal: ${ignored.length} git-ignored files from public/ left out of the build`);
    }
  };
}

// Settings recommended by Tauri: fixed port, no screen clearing, ignore the Rust folder.
export default defineConfig({
  plugins: [committedOnly()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ['**/src-tauri/**'] }
  },
  envPrefix: ['VITE_', 'TAURI_ENV_*'],
  build: {
    // WebView2 on Windows is Chromium-based.
    target: 'chrome105',
    minify: !process.env.TAURI_ENV_DEBUG ? 'esbuild' : false,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
    chunkSizeWarningLimit: 1500
  }
});
