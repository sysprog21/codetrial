# MediaPipe Face Detection Vendor Assets

This directory contains vendored assets for Google MediaPipe Face Detection (WASM and model binaries).

## Files Overview
- `face_detection.js`: MediaPipe JS API bundle for initializing and controlling the FaceDetection pipeline.
- `face_detection_short.binarypb`: Short-range MediaPipe SSD anchor / graph specification buffer.
- `face_detection_short_range.tflite`: Quantized TFLite model for short-range face detection (~2m).
- `face_detection_solution_wasm_bin.js` & `.wasm`: Standard WebAssembly binary and loader for CPU face detection.
- `face_detection_solution_simd_wasm_bin.js` & `.wasm`: SIMD-accelerated WebAssembly binary and loader for high-performance CPU inference.

Do not rename these. MediaPipe passes its own filenames to `locateFile`, and the
emscripten loaders ask for their own `.wasm` the same way, so a renamed file is
a 404 and a detector that never starts. The last rename cost exactly that: it
needed a name table in `face-presence.js` and another in `face-worker.js`, and
both were missing the `.wasm` entries.

## Browser Usage
The browser client consumes these files via Web Worker in [`face-worker.js`](../../face-worker.js) and [`face-presence.js`](../../face-presence.js).

The Web Worker overrides `locateFile` to point to the vendored directory to ensure local asset loading without external CDN dependencies.

## Maintenance & Upgrades
To update these binaries from upstream MediaPipe:
1. Download `@mediapipe/face_detection` package from npm or Google CDN (`https://cdn.jsdelivr.net/npm/@mediapipe/face_detection/`).
2. Copy `.js`, `.wasm`, `.tflite`, and `.binarypb` files into this directory under
   their upstream names.
3. If upstream adds or renames an asset, update the list in
   [`tests/browser/face-presence.test.js`](../../../tests/browser/face-presence.test.js),
   which fails when a name MediaPipe asks for is not vendored.
4. Regenerate the pins: `shasum -a 256 face_detection.js face_detection_short.binarypb
   face_detection_short_range.tflite face_detection_solution_*.js
   face_detection_solution_*.wasm > SHA256SUMS`, and say in the commit message
   where the bytes came from. `make verify-vendor` checks them and `make check`
   runs it.

These files are overwritten in place, under the same names, so the server sends
them with a one-day cache rather than an immutable one: an upgrade reaches a
browser holding the old bytes within a day instead of a year.
