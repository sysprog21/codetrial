export const groundingStorageKey = "codetrial.interview-grounding.v1";
export const groundingConsentVersion = 1;
export const maxGroundingFileBytes = 64 * 1024;
// MAX_GROUNDING_TEXT_BYTES in src/agent.rs, and that is not a coincidence to
// be maintained by memory: the server drops over-budget grounding silently.
export const maxGroundingPacketBytes = 6 * 1024;

const limits = { requirements: 8, skills: 8, anchors: 6 };
const textLimit = 240;

export async function parseGroundingFile(file, kind) {
  if (!file) throw new Error("Choose a .txt file.");
  if (!/\.txt$/i.test(file.name || "") || file.type !== "text/plain") {
    throw new Error("Use a UTF-8 .txt file with text/plain type.");
  }
  if (file.size === 0) throw new Error("The file is empty.");
  if (file.size > maxGroundingFileBytes) throw new Error("The file must be 64 KiB or smaller.");
  let text;
  try {
    text = new TextDecoder("utf-8", { fatal: true }).decode(await file.arrayBuffer());
  } catch {
    throw new Error("The file is not valid UTF-8.");
  }
  const lines = normalizeLines(text);
  if (!lines.length) throw new Error("The file contains no usable text.");
  return kind === "jd" ? parseJd(lines) : parseResume(lines);
}

export function selectedGroundingPacket(extracted, selected, consent) {
  const packet = {
    consentVersion: groundingConsentVersion,
    requirements: pick(extracted.requirements, selected.requirements).map(normalizeSnippet),
    skills: pick(extracted.skills, selected.skills).map(normalizeSnippet),
    anchors: pick(extracted.anchors, selected.anchors).map(normalizeSnippet),
  };
  const count = packet.requirements.length + packet.skills.length + packet.anchors.length;
  if (!count) return null;
  if (!consent) throw new Error("Agree to send only your selected snippets before starting.");

  // The server rejects a list holding the same snippet twice, and rejecting it
  // means dropping every snippet in the packet, not just the repeat. `pick`
  // only rules out choosing one index twice, so two lines that read alike in
  // the document still arrive as a pair. Said here, because the alternative is
  // an interview that quietly runs with no grounding at all.
  for (const field of ["requirements", "skills", "anchors"]) {
    if (new Set(packet[field]).size !== packet[field].length) {
      throw new Error("Two selected snippets are identical. Remove the repeat before starting.");
    }
  }

  // Counted over the same text the server counts, which is why the snippets
  // are normalized above rather than at the point of use: the server measures
  // what it stores, and measuring the raw selection here would be counting a
  // different string and calling it the same budget.
  const bytes = [...packet.requirements, ...packet.skills, ...packet.anchors]
    .reduce((total, text) => total + new TextEncoder().encode(text).length, 0);
  if (bytes > maxGroundingPacketBytes) {
    throw new Error("Selected snippets are too long. Select fewer or shorter snippets.");
  }
  return packet;
}

/// The normalization `grounding_array` applies in src/agent.rs, using the same
/// two Unicode properties it does: `char::is_control` is the Cc category, and
/// `split_whitespace` is the White_Space property. Spelling either as a
/// hand-written character class would agree with Rust today and drift at the
/// next edition of the tables.
function normalizeSnippet(text) {
  return [...String(text)]
    .map((character) => (/\p{Cc}/u.test(character) ? " " : character))
    .join("")
    .split(/\p{White_Space}+/u)
    .filter(Boolean)
    .join(" ");
}

export function storeGroundingPacket(storage, packet) {
  if (!packet) {
    try { storage.removeItem(groundingStorageKey); } catch { /* no grounding must remain usable */ }
    return;
  }
  try {
    storage.setItem(groundingStorageKey, JSON.stringify(packet));
  } catch {
    throw new Error("Selected snippets could not be stored temporarily. Clear grounding to start normally.");
  }
}

export function consumeGroundingPacket(storage) {
  let raw = null;
  try {
    raw = storage.getItem(groundingStorageKey);
  } catch { return null; }
  try { storage.removeItem(groundingStorageKey); } catch { return null; }
  if (!raw) return null;
  try {
    const value = JSON.parse(raw);
    return value?.consentVersion === groundingConsentVersion ? value : null;
  } catch {
    return null;
  }
}

function normalizeLines(text) {
  return text.replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/g, " ")
    .split(/\r?\n/).map((line) => line.replace(/\s+/g, " ").trim()).filter(Boolean);
}

function clean(line) {
  return line.replace(/^[-*•\d.)\s]+/, "").slice(0, textLimit).trim();
}

function unique(values, max) {
  return [...new Set(values.map(clean).filter((value) => value.length >= 2))].slice(0, max);
}

function parseJd(lines) {
  const marked = lines.filter((line) => /\b(required?|requirements?|must|should|experience|proficien|knowledge|ability)\b/i.test(line));
  return { requirements: unique(marked, limits.requirements), skills: [], anchors: [] };
}

function parseResume(lines) {
  const skillLines = lines.filter((line) => /^(skills?|technologies|stack)\s*:/i.test(line));
  const skills = skillLines.flatMap((line) => line.replace(/^[^:]+:/, "").split(/[,;|]/));
  const anchors = lines.filter((line) => /\b(project|experience|built|led|created|implemented|delivered|improved|reduced|increased|developed)\b/i.test(line));
  return { requirements: [], skills: unique(skills, limits.skills), anchors: unique(anchors, limits.anchors) };
}

function pick(values = [], indexes = []) {
  return [...new Set(indexes)].filter((index) => Number.isInteger(index) && index >= 0 && index < values.length)
    .map((index) => values[index]);
}
