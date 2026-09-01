import { escapeHtml } from "./lib.js";

export const STAR_FIELDS = ["Situation", "Task", "Action", "Result"];

export function candidateTranscriptText(segments) {
  return Array.from(segments || [])
    .filter((segment) => segment?.speaker !== "interviewer")
    .filter((segment) => segment?.final === true)
    .map((segment) => String(segment?.text || ""))
    .filter((text) => text.trim())
    .join("\n");
}

/// This is deliberately mechanical. Every non-label character comes from a
/// field the candidate controls, so the app cannot manufacture a fact while
/// making prose sound smoother.
export function starRewrite(fields) {
  return STAR_FIELDS
    .map((name) => [name, String(fields?.[name] || "").trim()])
    .filter(([, value]) => value)
    .map(([name, value]) => `${name}: ${value}`)
    .join("\n");
}

export function behavioralReviewMarkup(original) {
  if (!String(original || "").trim()) return "";
  const fields = STAR_FIELDS.map((name) => `
    <label class="star-field">${name}
      <textarea data-star-field="${name}" rows="2" placeholder="Paste or edit only what you actually said."></textarea>
    </label>`).join("");
  return `<section id="behavioral-review" aria-labelledby="behavioral-review-title">
    <h3 id="behavioral-review-title">Behavioral answer review</h3>
    <p class="muted small">Local self-review only. These edits do not change your score, source transcript, or replay evidence.</p>
    <h4>Before · original candidate transcript</h4>
    <pre id="star-original">${escapeHtml(original)}</pre>
    <h4>Tag your answer</h4>
    <div class="star-fields">${fields}</div>
    <div id="star-suggestion">
      <label for="star-rewrite">After · generated from your fields</label>
      <textarea id="star-rewrite" rows="6" aria-describedby="star-grounding"></textarea>
      <p id="star-grounding" class="muted small">The initial draft uses only your four fields plus fixed STAR labels.</p>
      <button id="star-reset" class="pill-button" type="button">Reset draft</button>
      <button id="star-dismiss" class="pill-button" type="button">Dismiss suggestion</button>
    </div>
  </section>`;
}

export function mountBehavioralReview(root, segments) {
  const original = candidateTranscriptText(segments);
  const markup = behavioralReviewMarkup(original);
  if (!markup) return false;
  const actions = root.querySelector(".report-actions");
  actions?.insertAdjacentHTML("beforebegin", markup);
  const section = root.querySelector("#behavioral-review");
  if (!section) return false;
  const rewrite = section.querySelector("#star-rewrite");
  const fields = () => Object.fromEntries(STAR_FIELDS.map((name) => [
    name,
    section.querySelector(`[data-star-field="${name}"]`)?.value || "",
  ]));
  const reset = () => { rewrite.value = starRewrite(fields()); };
  for (const input of section.querySelectorAll("[data-star-field]")) input.addEventListener("input", reset);
  section.querySelector("#star-reset")?.addEventListener("click", reset);
  section.querySelector("#star-dismiss")?.addEventListener("click", () => {
    section.querySelector("#star-suggestion")?.remove();
  });
  return true;
}
