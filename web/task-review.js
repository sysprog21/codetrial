// Split out of task.js so a node test can import it: task.js reads the LiveKit
// global and the DOM when it loads, and this does neither. It holds the words of
// the result area: what it says when there is no feedback, and the headed
// paragraphs an assessment is shown as. task.js builds the elements.

export const DIMENSION_LABELS = {
  reasoningParticipation: "Reasoning participation",
  implementationOwnership: "Implementation ownership",
  testingAndDiagnosis: "Testing and diagnosis",
  revisionAndImprovement: "Revision and improvement",
};

/// What the result area says when there is no feedback to show: whether the
/// result is still being finalized, and whether the learner's code can still be
/// had.
export function feedbackUnavailableText(awaitingResult, codeLost) {
  return awaitingResult
    ? codeLost
      ? "The result is still being finalized. Your code cannot be retrieved yet; try reloading later or ask your instructor."
      : "The result is still being finalized. Download your code now and reload later to retrieve the result."
    : codeLost
      ? "No retained code is available. You can start a new attempt or ask your instructor."
      : "Feedback unavailable. Download your code and ask your instructor for a review.";
}

/// The `[heading, text]` pairs an assessment is shown as: one per dimension,
/// then the gaps and the next actions. A generator, so that a malformed tail
/// throws after the head has been handed over, as the page always did.
export function* reviewSections(assessment) {
  for (const [name, dimension] of Object.entries(assessment.dimensions)) {
    yield [
      DIMENSION_LABELS[name] ?? name,
      `${dimension.rating ?? "Insufficient evidence"}. ${dimension.reason}`,
    ];
  }
  for (const [field, label] of [
    ["gaps", "Gaps"],
    ["nextActions", "Next actions"],
  ]) {
    yield [label, assessment[field].join(" ")];
  }
}
