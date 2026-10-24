import { test } from "node:test";
import assert from "node:assert/strict";
import { loadInterview } from "./source.js";
import {
  createEditorHistory,
  editorSnapshot,
} from "../../web/editor-history.js";

const snapshot = (
  value,
  start = value.length,
  end = start,
  direction = "none",
) => ({ value, start, end, direction });

test("interleaved deletions undo and redo within their own language", () => {
  const history = createEditorHistory();
  const python = snapshot("AAA\nBBB");
  const javascript = snapshot("111\n222");
  const pythonDeleted = snapshot("AAA\n");
  const javascriptDeleted = snapshot("111\n");
  history.select("python", python);
  history.select("javascript", javascript, python);
  history.record(javascript, javascriptDeleted);
  history.select("python", python, javascriptDeleted);
  history.record(python, pythonDeleted);
  history.select("javascript", javascript, pythonDeleted);
  assert.deepEqual(history.undo(), javascript);
  history.select("python", python, javascript);
  assert.deepEqual(history.undo(), python);
  history.select("javascript", javascript, python);
  assert.deepEqual(history.redo(), javascriptDeleted);
  history.select("python", python, javascriptDeleted);
  assert.deepEqual(history.redo(), pythonDeleted);
});

test("switching without editing neither creates an entry nor loses redo", () => {
  const history = createEditorHistory();
  const before = snapshot("before");
  const after = snapshot("after");
  history.select("python", before);
  history.record(before, after);
  assert.deepEqual(history.undo(), before);
  history.select("javascript", snapshot("js"), before);
  assert.equal(history.undo(), null);
  history.select("python", before, snapshot("js"));
  assert.deepEqual(history.redo(), after);
});

test("a new edit discards only that language's redo branch", () => {
  const history = createEditorHistory();
  history.select("python", snapshot("a"));
  history.record(snapshot("a"), snapshot("b"));
  history.undo();
  history.select("javascript", snapshot("1"), snapshot("a"));
  history.record(snapshot("1"), snapshot("2"));
  history.undo();
  history.select("python", snapshot("a"), snapshot("1"));
  history.record(snapshot("a"), snapshot("c"));
  assert.equal(history.redo(), null);
  history.select("javascript", snapshot("1"), snapshot("c"));
  assert.deepEqual(history.redo(), snapshot("2"));
});

test("undo and redo restore distinct selection bounds and direction", () => {
  const history = createEditorHistory();
  const before = snapshot("one\ntwo", 0, 7, "backward");
  const after = snapshot("# one\n# two", 2, 11, "backward");
  history.select("python", before);
  history.record(before, after);
  assert.deepEqual(history.undo(), before);
  assert.deepEqual(history.redo(), after);
});

test("each language has its own bounded history", () => {
  const history = createEditorHistory({ limit: 2 });
  history.select("python", snapshot("0"));
  for (let i = 1; i <= 3; i++)
    history.record(snapshot(String(i - 1)), snapshot(String(i)));
  history.select("javascript", snapshot("js"), snapshot("3"));
  history.record(snapshot("js"), snapshot("changed"));
  history.select("python", snapshot("3"), snapshot("changed"));
  assert.deepEqual(history.undo(), snapshot("2"));
  assert.deepEqual(history.undo(), snapshot("1"));
  assert.equal(history.undo(), null);
  history.select("javascript", snapshot("changed"), snapshot("1"));
  assert.deepEqual(history.undo(), snapshot("js"));
});

test("adjacent typing groups but cursor movement and pauses split entries", () => {
  const history = createEditorHistory({ groupDelay: 100 });
  history.select("python", snapshot(""));
  history.record(snapshot(""), snapshot("a"), "insertText", 0);
  history.record(snapshot("a"), snapshot("ab"), "insertText", 50);
  history.record(snapshot("ab"), snapshot("abc"), "insertText", 200);
  assert.deepEqual(history.undo(), snapshot("ab"));
  assert.deepEqual(history.undo(), snapshot(""));
  history.redo();
  history.breakGroup();
  history.record(snapshot("ab"), snapshot("abd"), "insertText", 220);
  assert.deepEqual(history.undo(), snapshot("ab"));
});

test("selection-only edits preserve redo and create no text history", () => {
  const history = createEditorHistory();
  history.select("python", snapshot("a"));
  history.record(snapshot("a"), snapshot("b"));
  history.undo();
  history.record(snapshot("a"), snapshot("a", 0));
  assert.deepEqual(history.redo(), snapshot("b"));
});

test("snapshot objects cannot mutate stored history", () => {
  const history = createEditorHistory();
  const before = snapshot("before");
  history.select("python", before);
  history.record(before, snapshot("after"));
  before.value = "mutated";
  const restored = history.undo();
  assert.equal(restored.value, "before");
  restored.value = "mutated again";
  history.redo();
  assert.equal(history.undo().value, "before");
});

test("starter buffer replacement through applyEditorEdit is one undoable edit", () => {
  const before = snapshot("candidate solution\n", 0, 9, "backward");
  const starter = "starter\n";
  const inputs = [];
  const editor = {
    value: before.value,
    selectionStart: before.start,
    selectionEnd: before.end,
    selectionDirection: before.direction,
    setSelectionRange(start, end, direction = "none") {
      this.selectionStart = start;
      this.selectionEnd = end;
      this.selectionDirection = direction;
    },
    dispatchEvent() {
      inputs.push(this.value);
    },
  };
  const editorHistory = createEditorHistory();
  editorHistory.select("python", before);
  const scope = {
    nodes: { editor },
    editorHistory,
    editorSnapshot,
    editorComposition: null,
    editorDrag: null,
    compositionTimer: null,
    pendingEditorEdit: null,
    observedEditor: before,
    applyingProgrammaticEdit: false,
    clearTimeout,
    Event,
  };
  const { applyEditorEdit } = loadInterview(
    ["applyEditorEdit", "finishEditorComposition", "finishEditorDrag"],
    scope,
  );
  applyEditorEdit({ value: starter, start: 0, end: 0 });
  assert.deepEqual(inputs, [starter]);
  assert.deepEqual(editorHistory.undo(), before);
  assert.equal(editorHistory.undo(), null);
  assert.deepEqual(editorHistory.redo(), snapshot(starter, 0));
});
