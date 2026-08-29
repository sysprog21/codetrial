# Avatar Vendor Assets

Three.js, its glTF loader, and `@pixiv/three-vrm`, bundled into one ES module so
Jim's VRM renderer can load without a CDN, a lockfile, or an import map.

## Files Overview

- `three-vrm.js`: the bundle. One ES module, no bare imports, no external
  requests at import time.
- `LICENSE-three.txt`: Three.js, MIT, Copyright (c) 2010-2026 three.js authors.
- `LICENSE-three-vrm.txt`: `@pixiv/three-vrm`, MIT, Copyright (c) 2019-2026
  pixiv Inc.
- `SHA256SUMS`: bare filenames, checked by `scripts/verify-vendor.sh`.

- `LICENSE-jim-vrm.txt`: the interviewer model's grant, read out of its own
  `VRMC_vrm.meta`.

The VRM model is not here at all, and is not fetched here. The browser
downloads it from the pinned URL in `web/avatar/model.js` and checks it
against the SHA-256 beside it; `docs/avatar-contract.md` records
why, and the neutral panel is what a candidate sees when it cannot be had.

## Sources

| Package | Version | Tarball |
|---|---|---|
| `three` | 0.185.1 | `https://registry.npmjs.org/three/-/three-0.185.1.tgz` |
| `@pixiv/three-vrm` | 3.5.5 | `https://registry.npmjs.org/@pixiv/three-vrm/-/three-vrm-3.5.5.tgz` |

Both are MIT. `@pixiv/three-vrm` declares `three: >=0.137` as a peer dependency,
so the pinned pair satisfies it.

## Why a bundle and not an import map

`@pixiv/three-vrm` and `GLTFLoader.js` both import `three` by bare specifier,
which a browser resolves only through an import map. Import maps cannot be
loaded from a `src` URL, so one would have to be inlined into
`web/interview.html`, and `src/web/policy.rs` sets `script-src 'self'` with no
`'unsafe-inline'` and no nonce. Bundling resolves the specifiers at vendor time
instead of loosening the policy at runtime.

## Regenerating

Reproduces `three-vrm.js` byte for byte with the pinned esbuild:

```sh
mkdir three-vrm-build && cd three-vrm-build
npm init -y
npm install three@0.185.1 @pixiv/three-vrm@3.5.5 esbuild@0.25.12
cat > entry.js <<'EOF'
export {
  AmbientLight,
  Clock,
  DirectionalLight,
  Object3D,
  PerspectiveCamera,
  Scene,
  Vector3,
  WebGLRenderer,
} from "three";
export { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
export { VRMLoaderPlugin, VRMUtils } from "@pixiv/three-vrm";
EOF
npx esbuild entry.js --bundle --format=esm --minify --legal-comments=eof \
  --target=es2022 --outfile=three-vrm.js
```

`--legal-comments=eof` appends both MIT notices to the end of the bundle.
`LICENSE-three.txt` and `LICENSE-three-vrm.txt` beside it already satisfy MIT
for this directory, but the bundle is served on its own URL, and anyone who
fetches only that URL should still receive the notice with the code.

The entry list is the whole API `web/avatar/vrm.js` uses. Adding an import there
means adding it here and re-running the bundle, because esbuild tree-shakes
everything the entry does not name. A different esbuild version produces
different bytes and a failing `SHA256SUMS`; bump the pin in this file when that
is deliberate.
