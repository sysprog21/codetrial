import test from "node:test";
import assert from "node:assert/strict";
import {
  DIMENSION_LABELS,
  feedbackUnavailableText,
  reviewSections,
} from "../../web/task-review.js";

// The strings below are what the task workspace shows in its result area.

test("the four rated dimensions have their own names", () => {
  assert.deepEqual(DIMENSION_LABELS, {
    reasoningParticipation: "Reasoning participation",
    implementationOwnership: "Implementation ownership",
    testingAndDiagnosis: "Testing and diagnosis",
    revisionAndImprovement: "Revision and improvement",
  });
});

test("an unavailable result says whether the code can still be had", () => {
  assert.equal(
    feedbackUnavailableText(true, true),
    "The result is still being finalized. Your code cannot be retrieved yet; try reloading later or ask your instructor.",
  );
  assert.equal(
    feedbackUnavailableText(true, false),
    "The result is still being finalized. Download your code now and reload later to retrieve the result.",
  );
  assert.equal(
    feedbackUnavailableText(false, true),
    "No retained code is available. You can start a new attempt or ask your instructor.",
  );
  assert.equal(
    feedbackUnavailableText(false, false),
    "Feedback unavailable. Download your code and ask your instructor for a review.",
  );
});

test("a result is one section per dimension, then the gaps and next actions", () => {
  const sections = [
    ...reviewSections({
      dimensions: {
        reasoningParticipation: { rating: "Strong", reason: "Explained it." },
        testingAndDiagnosis: { rating: "Weak", reason: "Skipped tests." },
      },
      gaps: ["No edge cases", "No retry"],
      nextActions: ["Add tests"],
    }),
  ];
  assert.deepEqual(sections, [
    ["Reasoning participation", "Strong. Explained it."],
    ["Testing and diagnosis", "Weak. Skipped tests."],
    ["Gaps", "No edge cases No retry"],
    ["Next actions", "Add tests"],
  ]);
});

test("an unnamed dimension keeps its key, and a missing rating says so", () => {
  const [first] = reviewSections({
    dimensions: { somethingNew: { reason: "Because." } },
    gaps: [],
    nextActions: [],
  });
  assert.deepEqual(first, ["somethingNew", "Insufficient evidence. Because."]);
  const [empty] = reviewSections({
    dimensions: { somethingNew: { rating: "", reason: "Because." } },
    gaps: [],
    nextActions: [],
  });
  assert.deepEqual(
    empty,
    ["somethingNew", ". Because."],
    "an empty rating is shown",
  );
});

test("empty gaps and next actions are still sections", () => {
  const sections = [
    ...reviewSections({ dimensions: {}, gaps: [], nextActions: [] }),
  ];
  assert.deepEqual(sections, [
    ["Gaps", ""],
    ["Next actions", ""],
  ]);
});

test("sections arrive one at a time, so a broken tail leaves the head shown", () => {
  const sections = reviewSections({
    dimensions: { testingAndDiagnosis: { rating: "Good", reason: "Fine." } },
    nextActions: [],
  });
  assert.deepEqual(sections.next().value, [
    "Testing and diagnosis",
    "Good. Fine.",
  ]);
  assert.throws(() => sections.next(), TypeError);
});
