// The only file in the repo that touches Three.js. It exists so avatar.js can
// stay testable in plain node: everything here is behind the injected
// `loadModel`, and nothing imports this module until an interview is running.
//
// The contract back to avatar.js is two methods, `apply(pose)` and `dispose()`.
// Any failure here -- no WebGL, no jim.vrm, a corrupt file -- throws, and
// avatar.js renders the neutral panel instead.

import {
  AmbientLight,
  Clock,
  DirectionalLight,
  GLTFLoader,
  Object3D,
  PerspectiveCamera,
  Scene,
  Vector3,
  VRMLoaderPlugin,
  VRMUtils,
  WebGLRenderer,
} from "../vendor/avatar/three-vrm.js";
import { EXPRESSION_NAMES, MODEL_URL } from "./avatar.js";

// Upper body only: the camera sits close enough that hips and legs are out of
// frame, which is also why the model budget in docs/avatar-contract.md never
// pays for a lower body.
//
// The distance is set by arithmetic, not taste. At FIELD_OF_VIEW_DEG vertical
// the visible height at the subject plane is 2 * d * tan(fov/2), so d = 1.0
// gives 0.50 m, a 0.25 m half-height. A VRM head bone sits at the top of the
// neck with roughly 0.12 to 0.15 m of skull above it, so the crown lands well
// inside the frame. An earlier 0.62 m gave a 0.155 m half-height and cropped
// the top of the head off.
const FIELD_OF_VIEW_DEG = 28;
const FRAMING_DISTANCE_M = 1.0;
// The subject sits above center, so aim below the head rather than lowering
// the camera, which is what put the crown even closer to the top edge.
const FRAMING_DROP_M = 0.08;

// How far the eyes' look-at target travels for a full-bound gaze. Gaze arrives
// in radians; the target is a point in front of the face, so this converts.
const GAZE_TARGET_DISTANCE_M = 1.5;

// Every VRM loads in its rest pose, which is a T-pose: arms straight out to the
// sides. Nothing in the format ships an idle animation, so an avatar that is
// only ever loaded and rendered stands there like a scarecrow with its arms in
// frame. Dropping the upper arms is what turns the model into a person sitting
// at a desk, and it applies to any model rather than being tuned to one.
const ARM_REST_ROTATION_Z = 1.2;

export async function loadVrm({ mount, url = MODEL_URL }) {
  const canvas = mount.ownerDocument.createElement("canvas");
  canvas.className = "jim-avatar-canvas";
  // A canvas is a graphic with no text alternative, so it gets one. `img` and
  // not `application`: what is on it is a picture of the interviewer, and a
  // screen reader announcing "Jim, the interviewer" once is the whole useful
  // content. It used to be `aria-hidden`, which was the right answer for an
  // unlabeled canvas and the wrong one for a labeled one.
  canvas.setAttribute("role", "img");
  canvas.setAttribute("aria-label", "Jim, the AI interviewer");

  const renderer = new WebGLRenderer({ canvas, alpha: true, antialias: true });
  renderer.setPixelRatio(Math.min(mount.ownerDocument.defaultView?.devicePixelRatio || 1, 2));

  const scene = new Scene();
  scene.add(new AmbientLight(0xffffff, 1.6));
  const key = new DirectionalLight(0xffffff, 1.4);
  key.position.set(0.5, 1.4, 1.0);
  scene.add(key);

  const camera = new PerspectiveCamera(FIELD_OF_VIEW_DEG, 1, 0.05, 20);
  const lookTarget = new Object3D();
  scene.add(lookTarget);

  const loader = new GLTFLoader();
  loader.register((parser) => new VRMLoaderPlugin(parser));

  // Everything from here to the return is inside one try. Only loadAsync used
  // to be, so a VRM that parsed but tripped one of the utils below threw with
  // the WebGL context still open, and a corrupt-enough file is exactly what
  // makes those utils throw.
  let frames = 0;
  let resizeObserver = null;
  try {
    const gltf = await loader.loadAsync(url);
    const vrm = gltf.userData.vrm;
    if (!vrm) throw new Error(`${url} is not a VRM`);

    // Both are pure wins for a head-and-shoulders shot: unused vertices are the
    // lower body the camera never sees, and one skeleton is one draw call.
    VRMUtils.removeUnnecessaryVertices(vrm.scene);
    VRMUtils.combineSkeletons(vrm.scene);
    // No-op on a VRM 1.0 file. VRM 0.x models face away from the camera without
    // it, which reads as a bug rather than as a version difference.
    VRMUtils.rotateVRM0(vrm);
    scene.add(vrm.scene);

    // Arms down before anything measures the model. VRM +X is the character's
    // left, so the left arm rotates negatively about Z to fall and the right
    // arm positively; both are set once here rather than per frame, because
    // vrm.update() syncs normalized bones to raw ones and will carry it.
    // Upper arms only. Posing the forearms as well changed nothing measurable,
    // because a model whose arms are driven by VRMC_node_constraint has those
    // constraints applied inside vrm.update(), after these rotations and on top
    // of them. Bone posing fixes an ordinary T-pose; it cannot fix a rigged
    // prop, and pretending otherwise is two more lines that do nothing.
    for (const [bone, sign] of [["leftUpperArm", -1], ["rightUpperArm", 1]]) {
      const node = vrm.humanoid?.getNormalizedBoneNode(bone);
      if (node) node.rotation.z = sign * ARM_REST_ROTATION_Z;
    }

    const head = vrm.humanoid?.getNormalizedBoneNode("head");
    const headPosition = new Vector3(0, 1.4, 0);
    if (head) {
      vrm.scene.updateMatrixWorld(true);
      head.getWorldPosition(headPosition);
    }
    camera.position.set(headPosition.x, headPosition.y, headPosition.z + FRAMING_DISTANCE_M);
    camera.lookAt(headPosition.x, headPosition.y - FRAMING_DROP_M, headPosition.z);
    if (vrm.lookAt) vrm.lookAt.target = lookTarget;

    const restHeadRotation = head ? { x: head.rotation.x, y: head.rotation.y, z: head.rotation.z } : null;
    const clock = new Clock();
    let width = 0;
    let height = 0;
    // Reading clientWidth flushes pending layout. Doing that once per frame put
    // a forced reflow into the typing path, because the editor rewrites its
    // highlight overlay on every keystroke. The observer marks it instead.
    let needsResize = true;
    resizeObserver = new (mount.ownerDocument.defaultView?.ResizeObserver ?? class {
      observe() {}
      disconnect() {}
    })(() => { needsResize = true; });
    resizeObserver.observe(mount);

    mount.appendChild(canvas);

    function resize() {
      const nextWidth = Math.max(1, Math.round(canvas.clientWidth));
      const nextHeight = Math.max(1, Math.round(canvas.clientHeight));
      if (nextWidth === width && nextHeight === height) return;
      width = nextWidth;
      height = nextHeight;
      renderer.setSize(width, height, false);
      camera.aspect = width / height;
      camera.updateProjectionMatrix();
    }

    function apply(pose) {
      if (needsResize) {
        needsResize = false;
        resize();
      }
      const expressions = vrm.expressionManager;
      if (expressions) {
        expressions.setValue("aa", pose.mouth);
        expressions.setValue("blink", pose.blink);
        // Every preset every frame. The pose carries a weight per name, so the
        // outgoing expression fades on the same curve the incoming one rises
        // on; the previous single-name pose could only cut the old one to zero.
        // Iterating the exported name list instead of Object.entries: the set
        // is fixed and this runs 60 times a second, so the entries array and
        // its pairs were four allocations per frame for nothing.
        for (const name of EXPRESSION_NAMES) {
          expressions.setValue(name, pose.expression[name]);
        }
      }

      lookTarget.position.set(
        headPosition.x + Math.sin(pose.gaze.x) * GAZE_TARGET_DISTANCE_M,
        headPosition.y + Math.sin(pose.gaze.y) * GAZE_TARGET_DISTANCE_M,
        headPosition.z + GAZE_TARGET_DISTANCE_M,
      );

      // Set on the NORMALIZED bone and before vrm.update(), which is the
      // documented direction: VRMHumanoidRig.update() reads the normalized
      // quaternion and writes the raw one. Setting it after update would be
      // overwritten on the next frame.
      if (head && restHeadRotation) {
        head.rotation.x = restHeadRotation.x - pose.headTilt.y + pose.breath;
        head.rotation.y = restHeadRotation.y + pose.headTilt.x;
      }

      vrm.update(clock.getDelta());
      renderer.render(scene, camera);
      frames += 1;
    }

    function dispose() {
      resizeObserver?.disconnect();
      VRMUtils.deepDispose(vrm.scene);
      scene.remove(vrm.scene);
      renderer.dispose();
      // dispose() frees GPU resources but leaves the context for the GC, and a
      // page gets about 16 of them.
      renderer.forceContextLoss?.();
      canvas.remove();
    }

    // frames() exists for the browser check: counting real render calls is the
    // only way to prove from outside that the loop is running, short of reading
    // pixels back, which needs preserveDrawingBuffer and is slower.
    return { apply, dispose, frames: () => frames };
  } catch (error) {
    resizeObserver?.disconnect();
    renderer.dispose();
    renderer.forceContextLoss?.();
    canvas.remove();
    throw error;
  }
}
