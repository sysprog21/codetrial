import { test } from "node:test";
import assert from "node:assert/strict";
import {
  editorOptionsFromParams,
  INDENT_UNITS,
  indentFromParams,
  loadEditorOptions,
  loadIndent,
  saveEditorOptions,
  saveIndent,
  setEditorOptions,
  setIndent,
} from "../../web/editor-options.js";
import { memoryStorage } from "./source.js";

const defaults = {
  highlight: true,
  execution: true,
  autoIndent: true,
  autoClose: true,
  monospace: true,
};

test("old interview URLs retain every editor feature", () => {
  assert.deepEqual(
    editorOptionsFromParams(new URLSearchParams("problem=example")),
    defaults,
  );
});

test("each editor option can be disabled without changing the others", () => {
  for (const key of Object.keys(defaults)) {
    const options = { ...defaults, [key]: false };
    const params = new URLSearchParams("problem=example&duration=45");
    setEditorOptions(params, options);
    assert.deepEqual(editorOptionsFromParams(params), options, key);
    assert.equal(params.get("problem"), "example");
    assert.equal(params.get("duration"), "45");
  }
});

test("only a literal zero disables a known option", () => {
  for (const value of ["", "false", "no", "2", "null"]) {
    const params = new URLSearchParams({
      editorHighlight: value,
      unknown: "0",
    });
    assert.deepEqual(editorOptionsFromParams(params), defaults, value);
  }
  assert.deepEqual(
    editorOptionsFromParams(new URLSearchParams("editorHighlight=0")),
    { ...defaults, highlight: false },
  );
});

test("reenabling options removes their URL overrides", () => {
  const params = new URLSearchParams();
  setEditorOptions(
    params,
    Object.fromEntries(Object.keys(defaults).map((key) => [key, false])),
  );
  assert.deepEqual(Object.values(editorOptionsFromParams(params)), [
    false,
    false,
    false,
    false,
    false,
  ]);
  setEditorOptions(params, defaults);
  assert.equal(params.toString(), "");
});

test("preferences survive a reload without changing an existing interview URL", () => {
  const storage = memoryStorage();
  assert.deepEqual(loadEditorOptions(storage), defaults);
  const options = { ...defaults, highlight: false, execution: false };
  saveEditorOptions(options, storage);
  assert.deepEqual(loadEditorOptions(storage), options);
  const params = new URLSearchParams();
  setEditorOptions(params, loadEditorOptions(storage));
  saveEditorOptions(defaults, storage);
  assert.deepEqual(loadEditorOptions(storage), defaults);
  assert.deepEqual(editorOptionsFromParams(params), options);
  assert.deepEqual(editorOptionsFromParams(new URLSearchParams()), defaults);
});

test("refused preference storage preserves defaults and does not throw", () => {
  const storage = {
    getItem() {
      throw new Error("storage blocked");
    },
    setItem() {
      throw new Error("storage blocked");
    },
    removeItem() {
      throw new Error("storage blocked");
    },
  };
  assert.deepEqual(loadEditorOptions(storage), defaults);
  assert.doesNotThrow(() =>
    saveEditorOptions({ ...defaults, autoClose: false }, storage),
  );
  assert.doesNotThrow(() => saveEditorOptions(defaults, storage));
});

test("the indentation choice round-trips and four spaces leaves no parameter", () => {
  assert.equal(indentFromParams(new URLSearchParams("problem=example")), "4");
  for (const name of ["2", "4", "tab"]) {
    const params = new URLSearchParams("problem=example");
    setIndent(params, name);
    assert.equal(indentFromParams(params), name);
    assert.equal(params.get("problem"), "example");
    assert.equal(params.has("editorIndent"), name !== "4", name);
  }
  assert.deepEqual(INDENT_UNITS, { 2: "  ", 4: "    ", tab: "\t" });
});

test("an unknown indentation falls back to four spaces", () => {
  for (const value of ["", "3", "8", "Tab", "\t", "toString", "__proto__"])
    assert.equal(
      indentFromParams(new URLSearchParams({ editorIndent: value })),
      "4",
      value,
    );
  const params = new URLSearchParams("editorIndent=2");
  setIndent(params, "3");
  assert.equal(params.has("editorIndent"), false);
});

test("the indentation choice survives a reload apart from the other options", () => {
  const storage = memoryStorage();
  assert.equal(loadIndent(storage), "4");
  saveIndent("tab", storage);
  saveEditorOptions(
    { ...loadEditorOptions(storage), highlight: false },
    storage,
  );
  assert.equal(loadIndent(storage), "tab");
  saveIndent("4", storage);
  assert.equal(loadIndent(storage), "4");
  assert.equal(loadEditorOptions(storage).highlight, false);
});

test("only a known indentation is stored, and refused storage keeps four", () => {
  const storage = memoryStorage();
  saveIndent("tab", storage);
  for (const value of ["3", "Tab", "\t", "__proto__", undefined]) {
    saveIndent(value, storage);
    assert.equal(
      storage.getItem("codetrial:editorIndent"),
      null,
      String(value),
    );
  }
  const refused = {
    getItem() {
      throw new Error("storage blocked");
    },
    setItem() {
      throw new Error("storage blocked");
    },
    removeItem() {
      throw new Error("storage blocked");
    },
  };
  assert.equal(loadIndent(refused), "4");
  assert.doesNotThrow(() => saveIndent("2", refused));
  assert.doesNotThrow(() => saveIndent("4", refused));
});
