// Split out of task.js so a node test can import it: task.js reads the LiveKit
// global and the DOM when it loads, and this does neither. It holds the text a
// run's cases are shown in, and takes the task's required cases as an argument
// instead of reading the page's state.

/// Public case `index` as the workspace names it, marked when the task
/// requires it to pass. By index, since a variant rename can make two cases'
/// shown labels equal.
export function caseLabel(row, index, requiredCases) {
  const label = row.label ?? `Case ${index + 1}`;
  return (requiredCases ?? []).includes(index) ? `${label} (required)` : label;
}

/// One case of a run: a failure says what went in, what was expected and what
/// came back, or the error, so the learner has something to debug from.
export function caseLine(row, index, requiredCases) {
  const label = caseLabel(row, index, requiredCases);
  if (row.pass) return `OK: ${label}`;
  const detail = [
    row.input !== undefined && `input ${row.input}`,
    row.expected !== undefined && `expected ${row.expected}`,
    row.error
      ? `error ${row.error}`
      : row.got !== undefined && `got ${row.got}`,
  ].filter(Boolean);
  return `FAIL: ${label}${detail.length ? ` (${detail.join("; ")})` : ""}`;
}
