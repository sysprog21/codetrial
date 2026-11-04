import { COMPILED_LANGUAGES } from "./compiler-explorer.js";
import { resultsMarkup, runnerStatusMarkup } from "./render.js";
import { editorSnapshot } from "./editor-history.js";
import { highlight } from "./highlight.js";
import { indentGuides } from "./indent-guides.js";
import { tokenize } from "./tokenizer.js";
import {
  matchingBrackets,
  indentNewline,
  indentSelection,
  toggleLineComment,
  isToggleCommentKeystroke,
  insertBracketPair,
  isBracketOpenerKeystroke,
  isBracketCloserKeystroke,
  typeOverCloser,
  isBackspaceKeystroke,
  deleteEmptyPair,
  moveSelectedText,
} from "./editor.js";

export const EDITOR_FONT_SIZES = ["0.75", "0.875", "1", "1.125", "1.25", "1.5"];
export const DEFAULT_EDITOR_FONT_SIZE = "0.875";

export function applyWorkspaceFontSize(nodes, stored) {
  const size = EDITOR_FONT_SIZES.includes(stored)
    ? stored
    : DEFAULT_EDITOR_FONT_SIZE;
  nodes.editorStack.style.setProperty("--editor-font-size", `${size}rem`);
  const index = EDITOR_FONT_SIZES.indexOf(size);
  nodes.editorFontSmaller?.setAttribute("aria-disabled", String(index === 0));
  nodes.editorFontLarger?.setAttribute(
    "aria-disabled",
    String(index === EDITOR_FONT_SIZES.length - 1),
  );
}

export function resolveWorkspaceTheme(stored) {
  return stored === "light" || stored === "dark"
    ? stored
    : matchMedia("(prefers-color-scheme: light)").matches
      ? "light"
      : "dark";
}

export function applyWorkspaceTheme(nodes, stored) {
  if (stored === "light" || stored === "dark")
    nodes.editorStack.dataset.theme = stored;
  else delete nodes.editorStack.dataset.theme;
  nodes.editorThemeToggle?.setAttribute(
    "aria-pressed",
    String(resolveWorkspaceTheme(stored) === "light"),
  );
}

export function applyWorkspaceWrap(nodes, stored) {
  const on = stored === "1";
  nodes.editorStack.classList.toggle("wrap", on);
  nodes.editorWrapToggle?.setAttribute("aria-pressed", String(on));
}

export function syncWorkspaceScroll(nodes) {
  if (nodes.editorHighlight) {
    nodes.editorHighlight.scrollTop = nodes.editor.scrollTop;
    nodes.editorHighlight.scrollLeft = nodes.editor.scrollLeft;
  }
  if (nodes.editorLines) nodes.editorLines.scrollTop = nodes.editor.scrollTop;
}

export function mountWorkspace({
  nodes,
  language = "javascript",
  readOnly = false,
  onEdit = () => {},
  onRunTests = () => {},
  controller,
}) {
  if (controller) {
    if (controller.testPanel)
      bindWorkspaceTestPanel(nodes, controller.testPanel);
    return mountControlledWorkspace(nodes, controller);
  }
  const cleanups = [];
  let tabLeaves = false;
  let running = false;
  const listen = (node, event, handler) => {
    if (!node) return;
    node.addEventListener(event, handler);
    cleanups.push(() => node.removeEventListener(event, handler));
  };
  const cache = { lines: 0, editor: null, frame: null };
  function paint() {
    if (nodes.editorHighlight && nodes.editorLines) {
      paintWorkspace({
        nodes,
        code: nodes.editor.value,
        language,
        options: { highlight: true },
        tokens: tokenize,
        cache,
      });
    } else {
      syncWorkspaceScroll(nodes);
    }
  }

  function edit(next) {
    if (readOnly || !next) return;
    nodes.editor.value = next.value;
    nodes.editor.setSelectionRange(next.start, next.end);
    paint();
    onEdit(nodes.editor.value);
  }
  function setReadOnly(value) {
    readOnly = Boolean(value);
    nodes.editor.readOnly = readOnly;
    if (nodes.run) nodes.run.disabled = readOnly || running;
  }
  listen(nodes.editor, "beforeinput", (event) => {
    if (readOnly) {
      event.preventDefault();
      return;
    }
    if (event.inputType === "insertLineBreak" && !event.isComposing) {
      event.preventDefault();
      edit(
        indentNewline(
          nodes.editor.value,
          nodes.editor.selectionStart,
          nodes.editor.selectionEnd,
          language,
        ),
      );
    }
  });
  listen(nodes.editor, "keydown", (event) => {
    if (readOnly || event.isComposing || event.keyCode === 229) return;
    if (event.key === "Escape") {
      tabLeaves = true;
      return;
    }
    if (["Shift", "Control", "Alt", "Meta"].includes(event.key)) return;
    const leaves = tabLeaves;
    tabLeaves = false;
    if (event.key === "Tab" && !leaves) {
      event.preventDefault();
      edit(
        indentSelection(
          nodes.editor.value,
          nodes.editor.selectionStart,
          nodes.editor.selectionEnd,
          event.shiftKey,
        ),
      );
    } else if (isToggleCommentKeystroke(event) && !event.repeat) {
      const toggled = toggleLineComment(
        nodes.editor.value,
        nodes.editor.selectionStart,
        nodes.editor.selectionEnd,
        language,
      );
      if (!toggled) return;
      event.preventDefault();
      edit(toggled);
    } else if (
      event.key === "Enter" &&
      (event.metaKey || event.ctrlKey) &&
      !event.altKey &&
      !event.shiftKey
    ) {
      event.preventDefault();
      if (!event.repeat) nodes.run?.click();
    }
  });
  listen(nodes.editor, "input", () => {
    if (!readOnly) {
      paint();
      onEdit(nodes.editor.value);
    }
  });
  listen(nodes.editor, "scroll", () => syncWorkspaceScroll(nodes));
  listen(nodes.editor, "blur", () => {
    tabLeaves = false;
  });
  listen(nodes.run, "click", async () => {
    if (readOnly || running) return;
    running = true;
    setReadOnly(readOnly);
    try {
      await onRunTests();
    } finally {
      running = false;
      setReadOnly(readOnly);
    }
  });
  setReadOnly(readOnly);
  paint();
  return {
    setValue(code) {
      nodes.editor.value = code;
      paint();
    },
    setLanguage(next) {
      language = next;
      paint();
    },
    setReadOnly,
    paint,
    destroy() {
      for (const cleanup of cleanups) cleanup();
    },
  };
}

function mountControlledWorkspace(nodes, c) {
  nodes.editor.addEventListener("dragstart", () => {
    c.finishEditorComposition();
    c.editorHistory.breakGroup();
    c.editorDrag = {
      before: editorSnapshot(nodes.editor),
      localDrop: false,
      moving: false,
    };
  });
  nodes.editor.addEventListener("drop", () => {
    if (c.editorDrag) c.editorDrag.localDrop = true;
  });
  nodes.editor.addEventListener("dragend", c.finishEditorDrag);
  nodes.editor.addEventListener("scroll", c.syncEditorScroll);
  document.addEventListener("selectionchange", () => {
    if (document.activeElement === nodes.editor) {
      // Keep input-only history repairs at the latest selection, without
      // accepting text that a native history action has already changed.
      if (c.observedEditor?.value === nodes.editor.value)
        c.observedEditor = editorSnapshot(nodes.editor);
      c.scheduleEditorPaint();
    }
  });
  nodes.editor.addEventListener("focus", () => c.paintEditor());
  nodes.editor.addEventListener("blur", () => {
    c.finishEditorComposition();
    c.editorHistory.breakGroup();
    c.paintEditor(false);
  });
  nodes.editor.addEventListener("pointerdown", () =>
    c.editorHistory.breakGroup(),
  );
  nodes.editor.addEventListener("compositionstart", () => {
    c.finishEditorComposition();
    c.editorHistory.breakGroup();
    c.editorComposition = editorSnapshot(nodes.editor);
  });
  nodes.editor.addEventListener("compositionend", () => {
    // Some engines send the committed input after compositionend. Include it
    // before closing the transaction, rather than recording its text twice.
    c.compositionTimer = setTimeout(c.finishEditorComposition, 0);
  });
  // Tab indents, so it cannot also move focus. Escape arms the next Tab to do
  // that instead, or a candidate driving the page from the keyboard is stuck
  // in the textarea with no way out.
  let tabLeavesEditor = false;
  // Escape arms only the keyboard's next move. A click in the editor is the
  // candidate editing again, and a blur means they left some other way, so
  // neither may leave the next Tab armed.
  for (const type of ["pointerdown", "blur"]) {
    nodes.editor.addEventListener(type, () => {
      tabLeavesEditor = false;
    });
  }
  nodes.editor.addEventListener("keydown", (event) => {
    const key = /^[\x20-\x7e]$/.test(event.key)
      ? event.key.toLowerCase()
      : event.key.length === 1
        ? event.code?.replace(/^Key/, "").toLowerCase()
        : event.key;
    const historyKey =
      (c.mac
        ? event.metaKey && !event.ctrlKey
        : event.ctrlKey && !event.metaKey) &&
      !event.altKey &&
      (key === "z" || (!c.mac && key === "y" && !event.shiftKey));
    if (historyKey) {
      // Let the input method handle its own candidate text until it commits.
      if (event.isComposing || event.keyCode === 229) return;
      event.preventDefault();
      if (!nodes.editor.disabled && !nodes.editor.readOnly)
        c.restoreEditorHistory(key === "y" || event.shiftKey);
      return;
    }
    if (
      [
        "ArrowLeft",
        "ArrowRight",
        "ArrowUp",
        "ArrowDown",
        "Home",
        "End",
        "PageUp",
        "PageDown",
      ].includes(event.key)
    )
      c.editorHistory.breakGroup();
    if (event.key === "Escape") {
      tabLeavesEditor = true;
      return;
    }
    // A modifier's own keydown must not disarm it. Shift fires one of its own
    // before Tab does, so without this Escape then Shift+Tab outdents instead
    // of moving focus backwards, and reverse tab order has no way out at all.
    if (["Shift", "Control", "Alt", "Meta"].includes(event.key)) return;
    const indents = event.key === "Tab" && !tabLeavesEditor;
    tabLeavesEditor = false;
    if (isToggleCommentKeystroke(event)) {
      const toggled = toggleLineComment(
        nodes.editor.value,
        nodes.editor.selectionStart,
        nodes.editor.selectionEnd,
        c.language,
      );
      if (!toggled) return;
      event.preventDefault();
      // A held chord repeats. Each repeat would flip the block back, and leave
      // an undo step of its own behind.
      if (!event.repeat) c.applyEditorEdit(toggled);
      return;
    }
    if (!indents) return;
    event.preventDefault();
    c.applyEditorEdit(
      indentSelection(
        nodes.editor.value,
        nodes.editor.selectionStart,
        nodes.editor.selectionEnd,
        event.shiftKey,
      ),
    );
  });
  nodes.editor.addEventListener("beforeinput", (event) => {
    if (["historyUndo", "historyRedo"].includes(event.inputType)) {
      if (event.isComposing) return;
      if (event.cancelable) {
        event.preventDefault();
        if (!nodes.editor.disabled && !nodes.editor.readOnly)
          c.restoreEditorHistory(event.inputType === "historyRedo");
      } else if (!nodes.editor.disabled && !nodes.editor.readOnly) {
        // selectionchange may still be queued when this action starts.
        c.observedEditor = editorSnapshot(nodes.editor);
      }
      return;
    }
    // Keep the buffer unchanged until the browser supplies the drop position.
    // Native deleteByDrag can also remove an unselected trailing newline.
    if (c.editorDrag?.localDrop && event.cancelable) {
      if (event.inputType === "deleteByDrag") {
        event.preventDefault();
        c.editorDrag.moving = true;
        return;
      }
      if (event.inputType === "insertFromDrop" && c.editorDrag.moving) {
        event.preventDefault();
        const before = c.editorDrag.before;
        const next = moveSelectedText(
          before.value,
          before.start,
          before.end,
          nodes.editor.selectionStart,
        );
        c.editorDrag = null;
        nodes.editor.setSelectionRange(
          before.start,
          before.end,
          before.direction,
        );
        c.applyEditorEdit(next);
        return;
      }
    }
    if (!c.applyingProgrammaticEdit && !c.editorComposition) {
      c.pendingEditorEdit = {
        before: editorSnapshot(nodes.editor),
        type: event.inputType,
        data: event.data,
      };
    }
    if (c.applyingProgrammaticEdit || event.isComposing || !event.cancelable)
      return;
    if (event.inputType === "insertLineBreak") {
      if (!c.editorOptions.autoIndent) return;
      event.preventDefault();
      c.applyEditorEdit(
        indentNewline(
          nodes.editor.value,
          nodes.editor.selectionStart,
          nodes.editor.selectionEnd,
          c.language,
        ),
      );
      return;
    }
    if (!c.editorOptions.autoClose) return;
    if (isBracketOpenerKeystroke(event)) {
      const pair = insertBracketPair(
        nodes.editor.value,
        nodes.editor.selectionStart,
        nodes.editor.selectionEnd,
        event.data,
        c.language,
      );
      if (!pair) return;
      event.preventDefault();
      c.applyEditorEdit(pair);
      return;
    }
    if (isBracketCloserKeystroke(event)) {
      const skip = typeOverCloser(
        nodes.editor.value,
        nodes.editor.selectionStart,
        nodes.editor.selectionEnd,
        event.data,
        c.language,
      );
      if (!skip) return;
      event.preventDefault();
      c.applyEditorEdit(skip);
      return;
    }
    if (isBackspaceKeystroke(event)) {
      const removal = deleteEmptyPair(
        nodes.editor.value,
        nodes.editor.selectionStart,
        nodes.editor.selectionEnd,
        c.language,
      );
      if (!removal) return;
      event.preventDefault();
      c.applyEditorEdit(removal);
    }
  });
  nodes.editor.addEventListener("input", (event) => {
    if (!c.replayingEditorHistory && !c.applyingProgrammaticEdit) {
      if (
        !event.isComposing &&
        ["historyUndo", "historyRedo"].includes(event.inputType)
      ) {
        // Non-cancelable native edits can notify only after writing. Discard
        // that result and replay the current language's edit.
        c.restoreEditorHistory(event.inputType === "historyRedo", true);
        return;
      }
      if (!c.editorComposition && !c.editorDrag) {
        const before = c.pendingEditorEdit?.before || c.observedEditor;
        const after = editorSnapshot(nodes.editor);
        const type = c.pendingEditorEdit?.type;
        const grouping =
          before?.start === before?.end &&
          ((type === "insertText" &&
            c.pendingEditorEdit.data?.length === 1 &&
            !/\s/.test(c.pendingEditorEdit.data)) ||
            ["deleteContentBackward", "deleteContentForward"].includes(type))
            ? type
            : null;
        if (before) c.editorHistory.record(before, after, grouping);
        c.observedEditor = after;
        c.pendingEditorEdit = null;
      }
    }
    c.onEdit(event);
  });
  return { paint: c.paintEditor, destroy() {} };
}

export function paintWorkspace(
  { nodes, code, language, options, tokens, cache },
  showMatch = false,
) {
  // Focus, blur and language switches paint immediately and supersede queued
  // input, so a stale callback cannot repaint after focus leaves the editor.
  if (cache.frame !== null) {
    cancelAnimationFrame(cache.frame);
    cache.frame = null;
  }

  const currentActiveLine =
    showMatch && nodes.editor.selectionStart === nodes.editor.selectionEnd
      ? code.slice(0, nodes.editor.selectionStart).split("\n").length - 1
      : -1;
  if (currentActiveLine >= 0) {
    nodes.editorStack?.style.setProperty(
      "--active-line",
      String(currentActiveLine),
    );
  } else {
    nodes.editorStack?.style.removeProperty("--active-line");
  }

  const brackets =
    options.highlight && showMatch
      ? matchingBrackets(
          code,
          nodes.editor.selectionStart,
          nodes.editor.selectionEnd,
          language,
          tokens,
        )
      : null;
  if (
    cache.editor?.code === code &&
    cache.editor.language === language &&
    cache.editor.brackets?.[0] === brackets?.[0] &&
    cache.editor.brackets?.[1] === brackets?.[1]
  )
    return;
  cache.editor = { code, language: language, brackets };
  if (options.highlight) {
    const classified = tokens(code, language);
    // Kept with the tokens they are measured from, so a caret move that only
    // shifts the bracket match reuses the guides instead of measuring again.
    classified.guides ??= indentGuides(code, classified.tokens);
    nodes.editorHighlight.firstElementChild.innerHTML = highlight(
      code,
      language,
      brackets,
      classified.tokens,
      classified.guides,
    );
  }
  const lines = code.split("\n").length;
  if (lines !== cache.lines) {
    cache.lines = lines;
    nodes.editorLines.textContent = Array.from(
      { length: lines },
      (_, index) => index + 1,
    ).join("\n");
  }
  // The paint is deferred to the next frame, and a scroll event can run
  // before it: Enter at the bottom of a long buffer scrolls the textarea
  // while the layers still hold the old text, so their scrollTop clamps to the
  // old height and every row sits one above its code until the next scroll.
  syncWorkspaceScroll(nodes);
}

function bindWorkspaceTestPanel(nodes, panel) {
  nodes.run.addEventListener("click", panel.run);
  // Cmd+Enter or Ctrl+Enter runs the tests from anywhere but another text
  // field. Those keep the chord, since on some platforms it types a line break
  // there, as in the report's STAR answers. The button's disabled state is the
  // guard, but the button stays enabled during the media preflight and after
  // the interview ends, so those are checked here. Everywhere else the chord
  // is taken, so it never types a newline into the code or presses the
  // focused button.
  document.addEventListener("keydown", (event) => {
    // Some input methods end composition before its confirming keydown. The
    // legacy IME code remains 229 even when isComposing is already false.
    if (
      event.key !== "Enter" ||
      !(event.metaKey || event.ctrlKey) ||
      event.shiftKey ||
      event.altKey ||
      event.isComposing ||
      event.keyCode === 229
    )
      return;
    if (
      event.target !== nodes.editor &&
      event.target.matches?.("textarea, input:not([type=checkbox])")
    )
      return;
    event.preventDefault();
    if (event.repeat || !panel.execution()) return;
    if (nodes.audioCheck.hidden && !panel.closed()) nodes.run.click();
  });
}

export function setWorkspaceResultsOpen(nodes, open) {
  if (nodes.resultsBody) nodes.resultsBody.hidden = !open;
  nodes.resultsToggle?.setAttribute("aria-expanded", String(open));
}

export function renderWorkspaceStatus(nodes, status) {
  if (nodes.resultsLabel) nodes.resultsLabel.textContent = "Test results";
  if (nodes.resultsFilter) nodes.resultsFilter.hidden = true;
  nodes.resultsContent.innerHTML = runnerStatusMarkup(status);
}

export function renderWorkspaceResults(
  nodes,
  summary,
  {
    status = null,
    hidePassed = false,
    diagnostics = null,
    contentOnly = false,
  } = {},
) {
  const { label, body } = resultsMarkup(
    summary,
    status,
    hidePassed,
    diagnostics,
  );
  if (nodes.resultsLabel && !contentOnly)
    nodes.resultsLabel.textContent = label;
  if (nodes.resultsFilter && !contentOnly)
    nodes.resultsFilter.hidden =
      Boolean(summary.setupError) || !summary.cases?.length;
  nodes.resultsContent.innerHTML = body;
}

export function firstWorkspaceRunnerStatus(language) {
  if (language === "python") return "booting";
  if (COMPILED_LANGUAGES.includes(language)) return "compiling";
  return "running";
}
