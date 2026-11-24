// A defense's submission: one ZIP for the learner to hand in, holding the
// result, what the learner wrote (code.txt), the transcript of the
// conversation and a manifest that names each file by its SHA-256. The
// recording is not in it: it goes on YouTube, and the manifest names it by
// its link, so a recording and a submission of one attempt are told from
// those of another.
import { zipStore } from "./zip.js";

const encoder = new TextEncoder();

/// The SHA-256 of `bytes`, in lower-case hex.
export async function sha256Hex(bytes) {
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return [...new Uint8Array(digest)]
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
}

const VIDEO_ID = /^[A-Za-z0-9_-]{11}$/;
const PLAYLIST_ID = /^[A-Za-z0-9_-]{10,64}$/;

/// The YouTube address in `text` for the manifest, or null when there is none;
/// throws when what was typed is not a YouTube video or playlist link.
export function youtubeUrl(text) {
  const value = text.trim();
  if (!value) return null;
  const refused = new Error(
    "Paste the YouTube link of your recording: a youtu.be or youtube.com video or playlist address.",
  );
  let url;
  try {
    url = new URL(value);
  } catch {
    throw refused;
  }
  const host = url.hostname.toLowerCase().replace(/^(www|m)\./, "");
  let video = null;
  let playlist = null;
  if (host === "youtu.be") video = url.pathname.slice(1);
  else if (host === "youtube.com") {
    if (url.pathname === "/watch") video = url.searchParams.get("v");
    else if (url.pathname.startsWith("/live/")) video = url.pathname.slice(6);
    else if (url.pathname === "/playlist")
      playlist = url.searchParams.get("list");
  }
  if (
    url.protocol !== "https:" ||
    url.username ||
    url.password ||
    // Only the standard HTTPS port: another is some other service.
    url.port ||
    !(VIDEO_ID.test(video ?? "") || PLAYLIST_ID.test(playlist ?? ""))
  )
    throw refused;
  return url.href;
}

const isoSeconds = (seconds) =>
  new Date(seconds * 1000).toISOString().replace(/\.\d+Z$/, "Z");

/// The conversation as lines the learner and the instructors can read. Times
/// are UTC: when the learner began a turn, when Jim's reply ended.
export function transcriptText(rows, heading) {
  const lines = rows.map(({ at, role, turnId, text }) => {
    const who =
      role === "jim" ? "Jim" : `Learner${turnId ? ` (${turnId})` : ""}`;
    return `[${isoSeconds(at)}] ${who}: ${text}`;
  });
  return `${heading}\n\n${lines.join("\n\n")}\n`;
}

/// What the learner wrote in the defense, read from its result: the line each
/// item asked about, the change asked, what was typed and predicted, and the
/// bug claim. The code itself stays in the learner's own checkout.
export function codeText(defense, heading) {
  const out = [heading, ""];
  for (const item of defense?.items ?? []) {
    out.push(
      `${item.id}  ${item.file}:${item.line}${item.exercise ? ` (Exercise ${item.exercise})` : ""}`,
      `  Original line: ${item.quote}`,
      `  Change asked:  ${item.change}${item.changedLine === null ? " (the line is removed)" : ` => ${item.changedLine || "(an empty line)"}`}`,
    );
    const allRuns = item.predict?.runs ?? [];
    const runs = allRuns.filter((run) => run.edited !== undefined);
    if (!allRuns.length) out.push("  Typed:         (nothing was run)");
    if (runs.length < allRuns.length)
      out.push("  Some run details were not retained in the result.");
    for (const [at, run] of runs.entries()) {
      out.push(
        `  Typed (run ${at + 1}): ${run.edited || "(the line deleted)"}`,
        `  Prediction:    fails ${run.prediction?.fails?.join(", ") || "nothing"}; ${run.prediction?.compiles === false ? "does not compile" : "compiles"}. ${run.prediction?.how ?? ""}`.trimEnd(),
      );
    }
    out.push("");
  }
  const claim = defense?.bug?.claim;
  out.push(
    "Bug",
    claim
      ? `  Claim: program ${claim.program}, cycle ${claim.cycle}, signal "${claim.signal}", rule "${claim.rule}", exercise ${claim.exercise}`
      : "  No claim was stated.",
  );
  return `${out.join("\n")}\n`;
}

const MANIFEST_NOTICE =
  "Made in the learner's browser: the hashes tell which files belong together and catch a mix-up of files. They do not prove who made the attempt, that nothing was edited, or what the video at the link shows.";

/// A file name from what the server stamped on the attempt, plain ASCII on
/// any system; the time when there is nothing to name it by.
function submissionName(result, when) {
  const assessment = result.taskAssessment ?? {};
  const word = (value) => String(value ?? "").replace(/[^A-Za-z0-9_.-]/g, "");
  const parts = [
    "defense-submission",
    word(assessment.setId),
    assessment.setVersion === undefined
      ? ""
      : `v${word(assessment.setVersion)}`,
    word(assessment.sessionId) ||
      new Date(when).toISOString().replace(/[:.]/g, "-"),
  ].filter(Boolean);
  return `${parts.join("-")}.zip`;
}

/// `{name, bytes, manifest}` for `result`, the exported result object,
/// `transcript`, the lines the page kept, and `recordingUrl`, the YouTube
/// address of the learner's recording or null.
export async function defenseSubmission({
  result,
  transcript,
  recordingUrl,
  when = Date.now(),
}) {
  const assessment = result.taskAssessment ?? {};
  const stamp = result.defense?.stamp ?? {};
  const heading = `CodeTrial defense of set ${assessment.setId ?? "?"} version ${assessment.setVersion ?? "?"}, student ${stamp.studentId ?? "?"}, attempt ${assessment.sessionId ?? "?"}.`;
  const files = [
    {
      name: "result.json",
      bytes: encoder.encode(JSON.stringify(result, null, 2)),
    },
    {
      name: "code.txt",
      bytes: encoder.encode(codeText(result.defense, heading)),
    },
    {
      name: "transcript.txt",
      bytes: encoder.encode(
        transcriptText(
          transcript,
          `${heading} Times are UTC. This transcript is incomplete: turns may be missing or shortened, including all turns captured before a reload or crash.`,
        ),
      ),
    },
  ];
  const manifest = {
    format: "codetrial-defense-submission-2",
    notice: MANIFEST_NOTICE,
    createdAt: new Date(when).toISOString(),
    setId: assessment.setId ?? null,
    setVersion: assessment.setVersion ?? null,
    taskId: assessment.taskId ?? null,
    sessionId: assessment.sessionId ?? null,
    studentId: stamp.studentId ?? null,
    head: stamp.head ?? null,
    recordingUrl,
    files: await Promise.all(
      files.map(async ({ name, bytes }) => ({
        name,
        bytes: bytes.length,
        sha256: await sha256Hex(bytes),
      })),
    ),
  };
  files.push({
    name: "manifest.json",
    bytes: encoder.encode(JSON.stringify(manifest, null, 2)),
  });
  return {
    name: submissionName(result, when),
    bytes: zipStore(files, when),
    manifest,
  };
}
