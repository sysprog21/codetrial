import { test } from "node:test";
import assert from "node:assert/strict";
import { mountWorkspace } from "../../web/workspace.js";

class Node extends EventTarget {
  value = "";
  readOnly = false;
  selectionStart = 0;
  selectionEnd = 0;
  textContent = "";
  setSelectionRange(start, end) {
    this.selectionStart = start;
    this.selectionEnd = end;
  }
}

test("read-only workspace ignores editor actions and paints incoming text", () => {
  const editor = new Node();
  const editorLines = new Node();
  const code = { innerHTML: "" };
  const editorHighlight = { firstElementChild: code };
  let edits = 0;
  const workspace = mountWorkspace({
    nodes: { editor, editorLines, editorHighlight },
    language: "javascript",
    readOnly: true,
    onEdit: () => edits++,
  });
  workspace.setValue("const n = 1;\nreturn n;");
  const key = new Event("keydown", { cancelable: true });
  Object.assign(key, { key: "Tab" });
  editor.dispatchEvent(key);
  assert.equal(editor.value, "const n = 1;\nreturn n;");
  assert.equal(editor.readOnly, true);
  assert.equal(edits, 0);
  assert.equal(editorLines.textContent, "1\n2");
  assert.match(code.innerHTML, /const/);
  workspace.destroy();
});

test("workspace indentation calls onEdit and read-only run cannot execute", () => {
  const editor = new Node();
  const run = new Node();
  let edits = 0;
  let runs = 0;
  const workspace = mountWorkspace({
    nodes: { editor, run },
    onEdit: () => edits++,
    onRunTests: () => runs++,
  });
  const key = new Event("keydown", { cancelable: true });
  Object.assign(key, { key: "Tab" });
  editor.dispatchEvent(key);
  assert.equal(editor.value, "    ");
  assert.equal(edits, 1);
  run.dispatchEvent(new Event("click"));
  assert.equal(runs, 1);
  workspace.setReadOnly(true);
  run.dispatchEvent(new Event("click"));
  assert.equal(runs, 1);
  workspace.destroy();
});

test("shared results panel renders runner progress and filters passing cases", async () => {
  const {
    renderWorkspaceResults,
    renderWorkspaceStatus,
    setWorkspaceResultsOpen,
  } = await import("../../web/workspace.js");
  const nodes = {
    resultsContent: { innerHTML: "" },
    resultsLabel: { textContent: "" },
    resultsFilter: { hidden: false },
    resultsBody: { hidden: true },
    resultsToggle: {
      setAttribute(name, value) {
        this[name] = value;
      },
    },
  };
  setWorkspaceResultsOpen(nodes, true);
  assert.equal(nodes.resultsBody.hidden, false);
  assert.equal(nodes.resultsToggle["aria-expanded"], "true");
  renderWorkspaceStatus(nodes, "compiling");
  assert.match(
    nodes.resultsContent.innerHTML,
    /Compiling with Compiler Explorer/,
  );
  assert.equal(nodes.resultsFilter.hidden, true);
  const summary = {
    passed: 1,
    total: 2,
    cases: [
      { label: "passing case", pass: true, timeMs: 1 },
      {
        label: "failing case",
        pass: false,
        input: "a",
        expected: "b",
        got: "c",
        timeMs: 2,
      },
    ],
  };
  renderWorkspaceResults(nodes, summary, { status: "done", hidePassed: true });
  assert.match(nodes.resultsLabel.textContent, /1.*2/);
  assert.match(nodes.resultsContent.innerHTML, /failing case/);
  assert.doesNotMatch(nodes.resultsContent.innerHTML, /passing case/);
  assert.match(nodes.resultsContent.innerHTML, /expected b/);
  assert.equal(nodes.resultsFilter.hidden, false);
  renderWorkspaceResults(nodes, { setupError: "compile failed", cases: [] });
  assert.equal(nodes.resultsFilter.hidden, true);
  assert.match(nodes.resultsContent.innerHTML, /compile failed/);
});
