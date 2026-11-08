// Draws the change as three debts (technical, cognitive, intent) and turns the
// contributor's intent record into text for the pull request. What each signal
// says is decided on the server (intentdebt.py); this only shows it. All text
// goes in through textContent. There is no score anywhere: a signal is an
// observation or a question.
import { citeButton, el } from "/explainer.js";

const LAYERS = [
  ["code", "Code", "Technical debt lives in the code."],
  ["people", "People", "Cognitive debt lives in people."],
  ["intent", "Intent", "Intent debt lives in artifacts."],
];

export function renderDebt(root, debt, onCite) {
  root.replaceChildren();
  for (const [key, title, where] of LAYERS) {
    const layer = debt?.[key];
    if (!layer) continue;
    const card = el("section", "debt-card");
    const heading = el("h3", "", title);
    heading.id = `debt-${key}`;
    card.setAttribute("aria-labelledby", heading.id);
    const list = el("ul", "debt-signals");
    for (const signal of layer.signals) {
      const item = el("li");
      item.append(signal.text);
      for (const cite of signal.cites ?? [])
        item.append(" ", citeButton(cite, onCite));
      list.append(item);
    }
    card.append(
      heading,
      el("p", "debt-where", where),
      el("p", "debt-asks", layer.asks),
      list,
    );
    root.append(card);
  }
}

const LABELS = [
  ["goal", "Goal"],
  ["constraints", "Must stay true"],
  ["alternatives", "Not done, and why"],
  ["check", "Checked by"],
  ["backup", "Also understood by"],
];

// The record as a section for a pull request description: only what was written,
// in the contributor's words, under plain labels.
export function intentMarkdown(fields) {
  const lines = LABELS.filter(([key]) => (fields[key] ?? "").trim()).map(
    ([key, label]) =>
      `- **${label}:** ${fields[key].trim().replace(/\s*\n\s*/g, " ")}`,
  );
  return lines.length
    ? ["### Why this change exists", "", ...lines, ""].join("\n")
    : "";
}
