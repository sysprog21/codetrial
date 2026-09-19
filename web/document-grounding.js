export const groundingStorageKey = "codetrial.interview-grounding.v1";
export const groundingConsentVersion = 1;
export const maxGroundingFileBytes = 64 * 1024;
// MAX_GROUNDING_TEXT_BYTES in src/agent.rs, and that is not a coincidence to
// be maintained by memory: the server drops over-budget grounding silently.
export const maxGroundingPacketBytes = 6 * 1024;

const encoder = new TextEncoder();
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

export function retainedSelection(selected, kind) {
  const replaced = kind === "jd" ? ["requirements"] : ["skills", "anchors"];
  const retained = { requirements: [], skills: [], anchors: [] };
  for (const group of Object.keys(retained)) {
    if (!replaced.includes(group)) retained[group] = [...(selected[group] || [])];
  }
  return retained;
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
    .reduce((total, text) => total + encoder.encode(text).length, 0);
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
  return String(text)
    .replace(/\p{Cc}/gu, " ")
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

// A bullet glyph is stripped on its own, because it is a list glyph and not
// part of any name, so it needs no space after it to say so. A run of dashes
// or asterisks stripped that freely would eat into real content, so those
// still only count as a marker once whitespace after them confirms it. A
// run, not one character: "-- " and "** " are what pasted markdown leaves.
//
// A numbered marker is stripped without a following space, because "1.Must"
// and "1.) Must" are common paste shapes and the "1." should not reach the
// interviewer. Its digits must be followed by "." or ")", which keeps "5G"
// and "3D" whole, and then by whitespace or two letters. A "." alone is not
// enough, since it follows a digit inside real tokens: "9.x Java" has one
// letter after it and stays whole, while "1.Must" and "1.Go" lose their
// marker. That is a heuristic, not a rule: "1.C experience" keeps its
// marker and "3.js experience" loses its "3.".
// \p{Nd} rather than \d, so a full-width digit is a marker digit like an
// ASCII one.
function clean(line) {
  return line.trimStart()
    .replace(/^(?:(?:-+|\*+)\s+|\p{Nd}+[.)]+(?=\s|\p{L}{2})\s*|•\s*)+/u, "")
    .slice(0, textLimit).trim();
}

// A token with no letter is digits and symbols only, wearing a list item's
// clothes, and that is not a skill on its own.
//
// A letter is what makes a token legible as a named thing: "ISO 27001" and
// "IEEE 754" keep the org name that scopes their number, "5G" and "3D" carry
// their own label, and a plain "5" or "27001" or "2015" carries no such
// scope, whether it arrived alone -- "Skills: 2025" -- or split off a shared
// prefix by parseResume's "," / ";" / "|" split -- "Skills: ISO 27001,
// 124141, 2015". Either way, there is nothing left to tell whether it is
// still part of a standard, a separate one, or an unrelated year. Rather
// than guess, every letterless token is dropped, with no exception: "24/7"
// and "100%" are the same digit-plus-symbol shape as "-50", "1-2", and
// "2020-2024", and none of them carry a letter to claim a meaning others
// would have to guess at.
//
// That costs bare part numbers -- 6502, 8051, 68000 -- which are real skills
// on a systems resume and have no letter either. The loss is accepted, not
// overlooked: "8051" and "2025" are the same shape, four bare digits, and
// nothing in the token says which is a part number and which is a year, so
// a filter that goes by shape and keeps one keeps the other. Both are
// dropped. A part number that names itself, "MOS 6502" or "Z80", is kept.
function unique(values, max) {
  return [...new Set(values.map(clean).filter((value) => /\p{L}/u.test(value)))].slice(0, max);
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
