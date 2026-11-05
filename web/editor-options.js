import { readStored, writeStored } from "./audio-output.js";

const STORAGE_KEY = "codetrial:editorOptions";
const PARAMETERS = {
  highlight: "editorHighlight",
  execution: "editorExecution",
  autoIndent: "editorAutoIndent",
  autoClose: "editorAutoClose",
  monospace: "editorMonospace",
};

export function editorOptionsFromParams(params) {
  return Object.fromEntries(
    Object.entries(PARAMETERS).map(([key, name]) => [
      key,
      params.get(name) !== "0",
    ]),
  );
}

export function setEditorOptions(params, options) {
  for (const [key, name] of Object.entries(PARAMETERS)) {
    if (options[key] === false) params.set(name, "0");
    else params.delete(name);
  }
}

export function loadEditorOptions(storage) {
  return editorOptionsFromParams(
    new URLSearchParams(readStored(STORAGE_KEY, storage) || ""),
  );
}

export function saveEditorOptions(options, storage) {
  const params = new URLSearchParams();
  setEditorOptions(params, options);
  writeStored(STORAGE_KEY, params.toString(), storage);
}

// Indentation is a choice of three rather than a switch, so it keeps a
// parameter and a stored value of its own. Four spaces, the default, leaves
// both unset, so an existing interview URL keeps the editor it had.
const INDENT_PARAMETER = "editorIndent";
const INDENT_STORAGE_KEY = "codetrial:editorIndent";
export const DEFAULT_INDENT = "4";

/// What one level of indentation types, by the name the lobby and URL use.
export const INDENT_UNITS = { 2: "  ", 4: "    ", tab: "\t" };

export function indentFromParams(params) {
  const name = params.get(INDENT_PARAMETER);
  return Object.hasOwn(INDENT_UNITS, name) ? name : DEFAULT_INDENT;
}

export function setIndent(params, name) {
  if (name === DEFAULT_INDENT || !Object.hasOwn(INDENT_UNITS, name))
    params.delete(INDENT_PARAMETER);
  else params.set(INDENT_PARAMETER, name);
}

export function loadIndent(storage) {
  const name = readStored(INDENT_STORAGE_KEY, storage);
  return Object.hasOwn(INDENT_UNITS, name) ? name : DEFAULT_INDENT;
}

// Only a value the readers accept is ever stored; anything else, like the
// default, clears the key.
export function saveIndent(name, storage) {
  const known = name !== DEFAULT_INDENT && Object.hasOwn(INDENT_UNITS, name);
  writeStored(INDENT_STORAGE_KEY, known ? name : "", storage);
}
