export const groundingStorageKey = "codetrial.interview-grounding.v1";
export const groundingConsentVersion = 1;
export const maxGroundingFileBytes = 64 * 1024;

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
    requirements: pick(extracted.requirements, selected.requirements),
    skills: pick(extracted.skills, selected.skills),
    anchors: pick(extracted.anchors, selected.anchors),
  };
  const count = packet.requirements.length + packet.skills.length + packet.anchors.length;
  if (!count) return null;
  if (!consent) throw new Error("Agree to send only your selected snippets before starting.");
  return packet;
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
