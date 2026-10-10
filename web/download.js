export function reportFilename(problemId, at) {
  const name = problemId.replace(/[^a-zA-Z0-9_-]/g, "-") || "past-interview";
  // Match the report's local timestamp, including interviews near midnight.
  const date = [at.getFullYear(), at.getMonth() + 1, at.getDate()]
    .map((part) => String(part).padStart(2, "0"))
    .join("-");
  return `interview-report-${name}-${date}.md`;
}

export function downloadMarkdown(markdown, filename) {
  downloadText(markdown, filename, "text/markdown");
}

export function downloadText(content, filename, type) {
  const url = URL.createObjectURL(new Blob([content], { type }));
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = filename;
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
  setTimeout(() => URL.revokeObjectURL(url), 0);
}
