# Third-Party Notices

CodeTrial redistributes the following browser assets and model. Verify their
pinned files with `make verify-vendor`.

| Component | Version | License | Purpose |
|---|---:|---|---|
| [MediaPipe Face Detection](https://www.npmjs.com/package/@mediapipe/face_detection) | 0.4.1646425229 | Apache-2.0 | Local face-presence analysis |
| [LiveKit client](https://www.npmjs.com/package/livekit-client) | 2.20.0 | Apache-2.0 | Browser room client, `dist/livekit-client.umd.js` |
| [Pyodide](https://github.com/pyodide/pyodide) | 0.26.4 | MPL-2.0 | In-browser Python runner |
| CPython standard library | 3.12 | PSF-2.0 | Shipped inside Pyodide as `python_stdlib.zip` |
| [Three.js](https://threejs.org/) | 0.185.1 | MIT | Avatar renderer |
| [@pixiv/three-vrm](https://github.com/pixiv/three-vrm) | 3.5.5 | MIT | VRM support |
| Seed-san, VirtualCast, Inc. | n/a | [VRM Public License 1.0](https://vrm.dev/licenses/1.0/) | Jim's avatar model, fetched by the browser, not redistributed |

The MediaPipe binaries and the Pyodide binaries are fetched at build time from
the version or commit named in their `FETCH` manifests, and accepted only when
they match the adjacent `SHA256SUMS`. The LiveKit client and the three-vrm
bundle are committed and pinned in place.

The avatar model is the one entry above that CodeTrial does not ship. It is not
committed, not fetched into the tree, and not embedded in the release binary:
the candidate's browser downloads it from a pinned upstream URL and checks it
against a pinned SHA-256 before rendering it. The license terms below still
apply, because the model is displayed.

License texts and source details are shipped with the relevant assets:

- `web/vendor/LICENSE-apache-2.0.txt` (LiveKit client, MediaPipe)
- `web/vendor/avatar/LICENSE-three.txt`
- `web/vendor/avatar/LICENSE-three-vrm.txt`
- `web/vendor/avatar/LICENSE-jim-vrm.txt`
- `web/vendor/avatar/NOTICE`
- `web/vendor/avatar/README.md`
- `web/vendor/pyodide/README.md`

## Avatar model

Jim's avatar is Seed-san, from
[`vrm-c/vrm-specification`](https://github.com/vrm-c/vrm-specification/tree/master/samples/Seed-san/vrm),
fetched at runtime from a commit-pinned URL rather than shipped. The pin is
`MODEL_URL` and `MODEL_SHA256`, both in `web/avatar/model.js`, and bytes that
miss it are never rendered.

Its embedded `VRMC_vrm.meta` grant, read out of the pinned bytes themselves,
permits redistribution, corporate commercial use, and modification
redistribution. It requires attribution (`creditNotation: "required"`), and
that obligation attaches to displaying the model, so it is met whether or not
the bytes are redistributed: the credit is in the lobby footer of
`web/index.html`, in `README.md`, and in `web/vendor/avatar/NOTICE`. The full
grant and its source checksum are in `web/vendor/avatar/LICENSE-jim-vrm.txt`.

Seed-san is a specification sample, not a dependency sample. It was selected
because its named publisher and embedded grant permit this use. Future avatar
assets must document their source, embedded license, attribution requirements,
and redistribution rights here, and the grant must be read out of the file's own
`VRMC_vrm.meta`: a license claim on a download page is not evidence about the
bytes, and in survey it was wrong more often than right. Do not use a real person's likeness without a
signed release or redistribute a dependency sample without an explicit product
grant.

Compiler Explorer is a remote service, not bundled software. C, C++, and Java
source is sent to it only when remote test runs are enabled.
