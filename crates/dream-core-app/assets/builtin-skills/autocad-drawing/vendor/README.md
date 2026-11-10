# vendor

Third-party code shipped with this skill so `scripts/model3d.cjs` works offline with Node only.

| File | Source | License |
|------|--------|---------|
| `three-viewer.iife.min.js` | three.js 0.170.0 (only the classes listed in `three-viewer.entry.js`) + OrbitControls, bundled as a classic script that sets `window.THREE` | MIT — `LICENSE.three.txt` |
| `occt/occt-import-js.{js,wasm}` | occt-import-js 0.0.23 (OpenCascade compiled to WASM), unmodified | LGPL-2.1 — `occt/license.*.txt` |

occt-import-js is loaded at runtime as a separate, unmodified file (dynamic linking), which
keeps the LGPL obligations to shipping it with its license text.

Rebuild the three.js bundle (when the viewer needs another class, add it to the entry file):

```bash
npm i three@0.170.0 esbuild@0.24.0
npx esbuild three-viewer.entry.js --bundle --minify --format=iife --legal-comments=eof --outfile=three-viewer.iife.min.js
```
