// The defense panel (docs/mycpu-defense.md): each line item with the change
// it asks for, a prediction recorded before its run, the run and its result,
// and the bug item's models, simulations and claim. The server holds the
// attempt; every view here is the one its last answer returned.

import { chiselLines, loadChisel } from "./chisel-highlight.js";

/// Builds an element with text, attributes and children.
function el(tag, props = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(props)) {
    if (key === "text") node.textContent = value;
    else if (key in node) node[key] = value;
    else node.setAttribute(key, value);
  }
  node.append(...children.filter((child) => child != null));
  return node;
}

/// `control` as the learner last left it: the panel is drawn again after
/// every answer, and what was typed or chosen must survive that.
function keep(drafts, key, control, prop = "value") {
  control.dataset.focus = key;
  if (drafts.has(key)) control[prop] = drafts.get(key);
  const save = () => drafts.set(key, control[prop]);
  control.addEventListener("input", save);
  control.addEventListener("change", save);
  return control;
}

/// A checkbox per spec, all of them ticked when `all` says so.
function specBoxes(name, specs, all, drafts, key) {
  return el(
    "fieldset",
    { className: "defense-specs" },
    el("legend", { text: name }),
    ...specs.map((spec) =>
      el(
        "label",
        {},
        keep(
          drafts,
          `${key}:${spec}`,
          el("input", { type: "checkbox", value: spec, checked: all }),
          "checked",
        ),
        spec.split(".").pop(),
      ),
    ),
  );
}

function ticked(fieldset) {
  return [...fieldset.querySelectorAll("input:checked")].map(
    (box) => box.value,
  );
}

/// Spec classes by their short names, comma-separated.
function shortSpecs(specs) {
  return specs.map((spec) => spec.split(".").pop()).join(", ");
}

/// The server's limit on a typed answer, in UTF-8 bytes: a character outside
/// ASCII takes two to four of them.
const TEXT_BYTES = 1024;

/// What the status line says when `text` is over the server's limit, else
/// null.
function overLimit(what, text) {
  return new TextEncoder().encode(text).length > TEXT_BYTES
    ? `${what} is too long: keep it within ${TEXT_BYTES} bytes (fewer characters outside ASCII).`
    : null;
}

function outcomeText(run) {
  const outcome = run.outcome;
  if (outcome.kind === "tests")
    return outcome.failed.length
      ? `failed: ${shortSpecs(outcome.failed)}`
      : "every spec passed";
  if (outcome.kind === "compileError") return "the change did not compile";
  return `no result (${outcome.reason})`;
}

/// An item before it is opened: what it asks, in order, and the button that
/// opens it, disabled once items may no longer be opened. Jim gives the same
/// overview aloud once it is opened.
function overview(heading, code, steps, begin, act) {
  const button = el("button", {
    type: "button",
    className: "primary",
    text: begin.open ? begin.label : "The items are over",
    "data-focus": `${begin.item}:begin`,
  });
  button.disabled = !begin.open;
  button.onclick = () => act("begin", { item: begin.item });
  return el(
    "article",
    { className: "defense-item defense-overview" },
    el("h3", { text: heading }),
    code,
    el("p", { className: "defense-overview-lead", text: "Overview" }),
    el("ol", {}, ...steps.map((step) => el("li", { text: step }))),
    button,
  );
}

/// What the overview says of hints, when the package allows any.
function hintStep(hints) {
  return hints?.allowed
    ? [
        `Jim may give a hint after a wrong or incomplete answer (${hints.allowed} in the whole defense); improve your answer from it. Each hint lowers this item's suggested mark to 2 at most.`,
      ]
    : [];
}

/// How many hints an opened item has had, for its heading.
function hintsGiven(hints, id) {
  const given = hints?.given?.[id] ?? 0;
  return given ? ` · ${given} hint${given === 1 ? "" : "s"}` : "";
}

/// The steps a line item asks for, in the order Jim takes them.
function lineSteps(item, hints) {
  return [
    "Explain to Jim what this line decides and in which clock cycle its value matters.",
    `Part b changes it: ${item.change}${item.deletes ? " (the line is deleted)" : ""}. Predict which specs will fail, or that the change will not compile, and say how, before any run.`,
    "Make the change in the one-line editor and run the specs you choose in a private copy of your checkout.",
    "Account to Jim for what the run showed, against your prediction.",
    ...hintStep(hints),
  ];
}

/// An item's lines with their numbers, colored once the grammar is loaded.
function nearLines(item) {
  return el(
    "pre",
    { className: "defense-near", "data-item": item.id },
    ...chiselLines(item.near).map((nodes, index) => {
      const number = item.nearFirst + index;
      return el(
        "span",
        { className: number === item.line ? "defense-asked" : "" },
        el("span", {
          className: "defense-lineno",
          text: `${String(number).padStart(4)}  `,
        }),
        ...nodes,
        "\n",
      );
    }),
  );
}

/// One line item: the code around the line, the change, the prediction and
/// the run; until it is opened, only its overview, so no control is built
/// for nothing.
function lineItem(item, runs, act, drafts, place) {
  const near = nearLines(item);
  const heading = `Item ${place.index} of ${place.total}: ${item.file}:${item.line}${item.exercise ? ` (Exercise ${item.exercise})` : ""}`;
  if (!item.begun)
    return overview(
      heading,
      near,
      lineSteps(item, place.hints),
      { item: item.id, label: `Begin item ${place.index}`, open: place.open },
      act,
    );
  const predicted = item.prediction;
  const fails = specBoxes(
    "Specs that will fail",
    item.specs,
    false,
    drafts,
    `${item.id}:fails`,
  );
  const noBuild = keep(
    drafts,
    `${item.id}:no-build`,
    el("input", { type: "checkbox" }),
    "checked",
  );
  const how = keep(
    drafts,
    `${item.id}:how`,
    el("textarea", {
      rows: 2,
      maxLength: 1024,
      placeholder: "How the change makes them fail, or why none will",
    }),
  );
  const edited = keep(
    drafts,
    `${item.id}:edited`,
    el("input", {
      className: "defense-edit",
      spellcheck: false,
      value: item.deletes ? "" : item.quote,
    }),
  );
  const runSpecs = specBoxes(
    "Specs to run",
    item.specs,
    true,
    drafts,
    `${item.id}:run`,
  );
  const record = el("button", {
    type: "button",
    text: "Record prediction",
    "data-focus": `${item.id}:record`,
  });
  record.onclick = () => {
    const long = overLimit("The explanation", how.value);
    if (long) return act.say(long);
    act("predict", {
      item: item.id,
      fails: noBuild.checked ? [] : ticked(fails),
      compiles: !noBuild.checked,
      how: how.value,
    });
  };
  const run = el("button", {
    type: "button",
    className: "primary",
    text: "Run with the change",
    disabled: !predicted,
    "data-focus": `${item.id}:run`,
  });
  run.onclick = () =>
    act("run", {
      item: item.id,
      edited: edited.value,
      specs: ticked(runSpecs),
    });
  return el(
    "article",
    { className: "defense-item" },
    el("h3", {
      tabIndex: -1,
      "data-focus": `${item.id}:opened`,
      text: heading + hintsGiven(place.hints, item.id),
    }),
    near,
    el("p", { text: `a. Explain this line to Jim.` }),
    el("p", {
      text: `b. Change: ${item.change}${item.deletes ? " (leave the line empty)" : ""}. Predict first, then run.`,
    }),
    predicted
      ? el("p", {
          className: "defense-recorded",
          text: `Prediction recorded: ${predicted.compiles === false ? "the change will not compile" : shortSpecs(predicted.fails) || "nothing fails"}; ${predicted.how}`,
        })
      : null,
    fails,
    el("label", {}, noBuild, " The change will not compile"),
    el("label", {}, "How", how),
    record,
    el("label", {}, `Line ${item.line} after your change`, edited),
    runSpecs,
    run,
    el(
      "ul",
      { className: "defense-runs", "aria-label": `Runs of ${item.id}` },
      ...runs.map((row) =>
        el(
          "li",
          {},
          `Predicted ${row.prediction.compiles === false ? "no build" : shortSpecs(row.prediction.fails) || "nothing"}; ${outcomeText(row)}`,
          row.summary
            ? el(
                "details",
                {},
                el("summary", { text: "Test output" }),
                el("pre", { className: "defense-retire", text: row.summary }),
              )
            : null,
        ),
      ),
    ),
  );
}

/// A whole number of at least 0 typed in a field, or null.
function wholeNumber(text) {
  return /^\d{1,12}$/.test(text.trim()) ? Number(text.trim()) : null;
}

/// The lanes a waveform draws from a window's table: the fields that change
/// in the window first, then the steady ones, each in table order, at most
/// `limit`. A field whose values are all 0 or 1 is a bit.
export function waveLanes(table, limit = 12) {
  const lanes = table.fields.map((name, column) => {
    const values = table.rows.map((row) => row[column]);
    return {
      name,
      values,
      moves: values.some(
        (value, index) => index > 0 && value !== values[index - 1],
      ),
      bit: values.every((value) => value === 0 || value === 1),
    };
  });
  return [
    ...lanes.filter((lane) => lane.moves),
    ...lanes.filter((lane) => !lane.moves),
  ].slice(0, limit);
}

const SVG = "http://www.w3.org/2000/svg";

/// An SVG element with attributes and children.
function svg(tag, attributes = {}, ...children) {
  const node = document.createElementNS(SVG, tag);
  for (const [key, value] of Object.entries(attributes))
    node.setAttribute(key, String(value));
  node.append(...children.filter((child) => child != null));
  return node;
}

/// The window drawn as a waveform: a bit as a square wave, a wider field as
/// a bus with its value in hex wherever there is room, cycles along the top.
function waveform(table) {
  const lanes = waveLanes(table);
  const cycles = table.rows.length;
  const label = 170;
  const cell = Math.max(6, Math.min(24, Math.floor(720 / Math.max(cycles, 1))));
  const lane = 22;
  const width = label + cell * cycles + 8;
  const height = 18 + lane * lanes.length;
  const x = (cycle) => label + cell * cycle;
  const ticks = [];
  for (
    let cycle = 0;
    cycle < cycles;
    cycle += Math.max(1, Math.ceil(40 / cell))
  )
    ticks.push(
      svg("text", { x: x(cycle) + 2, y: 11 }, String(table.first + cycle)),
      svg("line", {
        x1: x(cycle),
        y1: 14,
        x2: x(cycle),
        y2: height,
        class: "tick",
      }),
    );
  const rows = lanes.map((row, index) => {
    const top = 18 + lane * index + 4;
    const bottom = top + lane - 8;
    const name = svg("text", { x: 4, y: bottom - 2 }, row.name);
    if (row.bit) {
      const points = row.values
        .map((value, cycle) => {
          const y = value ? top : bottom;
          return `${x(cycle)},${y} ${x(cycle + 1)},${y}`;
        })
        .join(" ");
      return svg("g", {}, name, svg("polyline", { points, class: "bit" }));
    }
    const parts = [name];
    let start = 0;
    for (let cycle = 1; cycle <= cycles; cycle++) {
      if (cycle < cycles && row.values[cycle] === row.values[start]) continue;
      const [left, right] = [x(start), x(cycle)];
      const slant = Math.min(3, (right - left) / 2);
      const middle = (top + bottom) / 2;
      parts.push(
        svg("polygon", {
          points: `${left},${middle} ${left + slant},${top} ${right - slant},${top} ${right},${middle} ${right - slant},${bottom} ${left + slant},${bottom}`,
          class: "bus",
        }),
      );
      const text = row.values[start].toString(16);
      if ((text.length + 1) * 6 < right - left)
        parts.push(svg("text", { x: left + 4, y: bottom - 2 }, text));
      start = cycle;
    }
    return svg("g", {}, ...parts);
  });
  return svg(
    "svg",
    {
      class: "defense-waveform",
      width,
      height,
      viewBox: `0 0 ${width} ${height}`,
      role: "img",
      "aria-label": `Waveform of ${lanes.length} signals over ${cycles} cycles from cycle ${table.first}`,
    },
    ...ticks,
    ...rows,
  );
}

function simTable(sim) {
  // Shown whatever else the run left, as a run cut at the cycle limit still
  // has its window to show; the status line announced it once, when the run
  // finished, so a redraw does not say it again.
  const alert = sim.error
    ? el("p", { className: "defense-error", text: sim.error })
    : null;
  if (!sim.table && !sim.retire.length) return alert;
  const retire = el(
    "pre",
    { className: "defense-retire" },
    sim.retire.map(([cycle, line]) => `${cycle}: ${line}`).join("\n") ||
      "(no instruction retired in this window)",
  );
  if (!sim.table) return el("div", {}, alert, retire);
  const head = el(
    "tr",
    {},
    el("th", { scope: "col", text: "cycle" }),
    ...sim.table.fields.map((name) => el("th", { scope: "col", text: name })),
  );
  const rows = sim.table.rows.map((row, index) =>
    el(
      "tr",
      {},
      el("th", { scope: "row", text: String(sim.table.first + index) }),
      ...row.map((value) => el("td", { text: value.toString(16) })),
    ),
  );
  return el(
    "div",
    {},
    alert,
    retire,
    el("div", { className: "defense-wave" }, waveform(sim.table)),
    el(
      "div",
      {
        className: "defense-wave",
        tabIndex: 0,
        role: "region",
        "aria-label": `Cycles of the ${sim.model} model`,
      },
      el(
        "table",
        {},
        el("caption", {
          text: `${sim.model} model, ${sim.program}, cycles ${sim.table.first} to ${sim.table.first + sim.table.rows.length - 1} (hex values)`,
        }),
        el("thead", {}, head),
        el("tbody", {}, ...rows),
      ),
    ),
  );
}

function bugItem(view, act, drafts) {
  const bug = view.bug;
  const heading = `The bug item: a model of your ${bug.task} core${bug.variant ? ` (${bug.variant})` : ""} with one bug`;
  if (!view.bugBegun)
    return overview(
      heading,
      null,
      [
        "Build two models of your own core: one as you wrote it, one with a single bug in it.",
        "Run one of your built programs on each model, and compare their retire records and waveforms.",
        "Find the first instruction whose effect differs, and look at the cycles around it.",
        "State once, in the panel: the cycle, the signal, the rule the bug model breaks and the exercise of yours that implements it. Then show Jim where it appears in the run.",
        ...hintStep(view.hints),
      ],
      {
        item: "bug",
        label: "Begin the bug item",
        open: view.itemsOpen !== false,
      },
      act,
    );
  const last = view.sims.at(-1);
  // Until the learner picks another, the program and the window are those of
  // the last run, so a claim names the program that was looked at.
  const program = el(
    "select",
    { "aria-label": "Program" },
    ...bug.programs.map((name) => new Option(name, name)),
  );
  if (last && bug.programs.includes(last.program)) program.value = last.program;
  keep(drafts, "bug:program", program);
  const model = keep(
    drafts,
    "bug:model",
    el(
      "select",
      { "aria-label": "Model" },
      new Option("bug model", "bug"),
      new Option("your core", "clean"),
    ),
  );
  const first = keep(
    drafts,
    "bug:first",
    el("input", { type: "number", min: 0, value: last?.first ?? 0 }),
  );
  const count = keep(
    drafts,
    "bug:count",
    el("input", { type: "number", min: 1, max: 256, value: last?.count ?? 32 }),
  );
  const build = el("button", {
    type: "button",
    text: bug.built ? "Models built" : "Build the two models",
    disabled: bug.built,
    "data-focus": "bug:build",
  });
  build.onclick = () => act("models", {});
  const simulate = el("button", {
    type: "button",
    className: "primary",
    text: "Run",
    disabled: !bug.built || !bug.programs.length,
    "data-focus": "bug:simulate",
  });
  simulate.onclick = () => {
    const span = [first, count].map((input) => wholeNumber(input.value));
    if (span.includes(null) || span[1] === 0)
      return act.say(
        "Give the first cycle and how many cycles as whole numbers.",
      );
    act("simulate", {
      model: model.value,
      program: program.value,
      first: span[0],
      count: span[1],
    });
  };
  const claim = {
    cycle: keep(drafts, "bug:cycle", el("input", { type: "number", min: 0 })),
    signal: keep(drafts, "bug:signal", el("input", { maxLength: 1024 })),
    rule: keep(drafts, "bug:rule", el("input", { maxLength: 1024 })),
    exercise: keep(
      drafts,
      "bug:exercise",
      el("input", { maxLength: 16, placeholder: "N.M" }),
    ),
  };
  // One claim per attempt: the server refuses a second.
  const state = el("button", {
    type: "button",
    text: view.claim ? "Bug stated" : "State the bug",
    disabled: Boolean(view.claim),
    "data-focus": "bug:claim",
  });
  // A claim is final, so one missing a part is never sent.
  state.onclick = () => {
    const cycle = wholeNumber(claim.cycle.value);
    const texts = [claim.signal, claim.rule, claim.exercise].map((input) =>
      input.value.trim(),
    );
    if (cycle === null || texts.includes(""))
      return act.say(
        "Give the cycle as a whole number, and the signal, the rule and the exercise.",
      );
    const long =
      overLimit("The signal", texts[0]) ?? overLimit("The rule", texts[1]);
    if (long) return act.say(long);
    act("claim", {
      program: program.value,
      cycle,
      signal: texts[0],
      rule: texts[1],
      exercise: texts[2],
    });
  };
  return el(
    "article",
    { className: "defense-item" },
    el("h3", {
      tabIndex: -1,
      "data-focus": "bug:opened",
      text: heading + hintsGiven(view.hints, "bug"),
    }),
    el("p", {
      text: "Run your programs on the bug model and on your core, find the first instruction whose effect differs, and look at the cycles around it.",
    }),
    bug.buildError
      ? el("p", { className: "defense-error", text: bug.buildError })
      : null,
    build,
    bug.programs.length
      ? null
      : el("p", {
          text: "No program is built for this task: build your programs before Start.",
        }),
    el(
      "div",
      { className: "defense-sim" },
      el("label", {}, "Program ", program),
      el("label", {}, "Model ", model),
      el("label", {}, "First cycle ", first),
      el("label", {}, "Cycles ", count),
      simulate,
    ),
    last ? simTable(last) : null,
    el(
      "div",
      { className: "defense-claim" },
      el("label", {}, "Cycle ", claim.cycle),
      el("label", {}, "Signal ", claim.signal),
      el("label", {}, "Rule it breaks ", claim.rule),
      el("label", {}, "Your exercise ", claim.exercise),
      state,
    ),
    view.claim
      ? el("p", {
          className: "defense-recorded",
          tabIndex: -1,
          "data-focus": "bug:stated",
          text: `Stated: cycle ${view.claim.cycle}, ${view.claim.signal}, ${view.claim.rule}, Exercise ${view.claim.exercise}`,
        })
      : null,
  );
}

/// What the review shows of a defense: each item's suggested marks, and the
/// notice that they are suggestions. Empty for any other task.
export function defenseSections(defense) {
  if (!defense?.items) return [];
  // An attempt that did not complete has no marks, for want of completion
  // rather than of evidence.
  const mark = (part) =>
    defense.marksWithheld
      ? "withheld (the attempt did not complete)"
      : (part?.suggestedMark ??
        `none (${part?.insufficientReason ?? "no evidence"})`);
  return [
    ["Defense", defense.notice],
    ...defense.items.map((item) => [
      `${item.id}: ${item.file}:${item.line}`,
      `Explain: ${mark(item.explain)}. Predict and confirm: ${mark(item.predict)}.${item.explain?.reason ? ` ${item.explain.reason}` : ""}`,
    ]),
    ["Bug", `Suggested mark: ${mark(defense.bug)}.`],
  ];
}

/// What the status line says once an action's answer arrives.
function finished(action, view) {
  if (action === "run") {
    const run = view.runs?.at(-1);
    return run ? `The run finished: ${outcomeText(run)}.` : "The run finished.";
  }
  const sim = view.sims?.at(-1);
  if (action === "simulate" && sim?.error)
    return `The run is shown below. ${sim.error}`;
  return (
    {
      begin: "The item is open; Jim gives its overview.",
      predict: "Prediction recorded.",
      models: "Both models are built.",
      simulate: "The run is shown below.",
      claim: "The bug is stated.",
    }[action] ?? ""
  );
}

/// Where focus goes when the control the learner used comes back disabled or
/// gone: an opened item to its heading, a run to the next prediction, a build
/// to a simulation, a claim to what was stated.
function nextFocus(key) {
  if (key.endsWith(":begin")) return key.replace(/:begin$/, ":opened");
  if (key === "bug:build") return "bug:simulate";
  if (key === "bug:claim") return "bug:stated";
  return key.endsWith(":run") ? key.replace(/:run$/, ":record") : null;
}

/// Mounts the panel in `root`. `api(path, body)` is the page's request
/// helper; `fail(error)` shows a refusal. Returns `refresh`, `setLocked` and
/// `destroy`.
export function mountDefense(root, api, fail) {
  let locked = false;
  let busy = false;
  let destroyed = false;
  // Whether the last view said another request holds the attempt.
  let waiting = false;
  // The refresh waiting to ask again while the attempt is held.
  let poll = null;
  // Refreshes asked for, so that only the latest is drawn.
  let asks = 0;
  // What the learner typed or chose, by control, for the panel's lifetime.
  const drafts = new Map();
  const status = el("p", { role: "status", "aria-live": "polite" });
  // The control the learner last used, given focus again after a redraw.
  let focusKey = null;
  // The last view drawn, drawn again once the Chisel grammar has loaded.
  let shown = null;
  // Once the grammar loads, only the code is drawn again, colored: the rest
  // of the panel, and whatever the learner is typing in it, stays as it is.
  loadChisel().then((loaded) => {
    if (!loaded || !shown || destroyed) return;
    for (const pre of root.querySelectorAll("pre.defense-near[data-item]")) {
      const item = shown.lines?.find((line) => line.id === pre.dataset.item);
      if (item) pre.replaceWith(nearLines(item));
    }
  });
  /// Enables or disables every control at once, as the lock and a running
  /// request say.
  const syncControls = () => {
    const controls = root.querySelector(".defense-controls");
    if (controls) controls.disabled = locked || busy;
  };

  /// Asks for the view again shortly, one poll at a time however many
  /// refreshes asked for one.
  const later = () => {
    poll ??= setTimeout(() => {
      poll = null;
      if (!destroyed) refresh();
    }, 5000);
  };

  /// Shows the status line alone, in place of the items.
  const statusOnly = (text) => {
    status.textContent = text;
    if (!root.contains(status)) root.replaceChildren(status);
  };

  function render(view) {
    if (destroyed) return;
    // Another request holds the attempt, as one from a second tab can: say
    // so, and ask again shortly.
    waiting = Boolean(view.busy);
    if (view.busy) {
      statusOnly("A run is still going; the panel waits for it.");
      later();
      return;
    }
    shown = view;
    const runs = view.runs ?? [];
    root.replaceChildren(
      el(
        "fieldset",
        { className: "defense-controls", disabled: locked || busy },
        el("legend", { text: "Defense" }),
        el("p", {
          text: `Work under defense: ${view.stamp.studentId} at ${view.stamp.head.slice(0, 12)}${view.stamp.auditor ? " (auditor, not scored)" : ""}.`,
        }),
        status,
        ...view.lines.map((item, index) =>
          lineItem(
            item,
            runs.filter((run) => run.item === item.id),
            act,
            drafts,
            {
              index: index + 1,
              total: view.lines.length,
              hints: view.hints,
              open: view.itemsOpen !== false,
            },
          ),
        ),
        bugItem(view, act, drafts),
      ),
    );
    const find = (key) =>
      key && root.querySelector(`[data-focus="${CSS.escape(key)}"]`);
    let again = find(focusKey);
    if (focusKey && (!again || again.disabled))
      again = find(nextFocus(focusKey));
    if (again && !again.disabled) again.focus();
  }

  async function act(action, body) {
    if (locked || busy) return;
    busy = true;
    // A refresh already on its way read the attempt before this action, so
    // its answer is not drawn over this one's.
    asks += 1;
    focusKey = document.activeElement?.dataset?.focus ?? null;
    syncControls();
    status.textContent =
      action === "run" || action === "models"
        ? "Running in a private copy of your checkout… this can take minutes."
        : action === "simulate"
          ? "Running the program on the model…"
          : "Recording…";
    try {
      const view = await api(`/api/defense/${action}`, body);
      busy = false;
      render(view);
      status.textContent = finished(action, view);
    } catch (error) {
      busy = false;
      if (destroyed) return;
      fail(error);
      await refresh();
      if (destroyed) return;
      syncControls();
      // The refusal is said here too, beside the control that was used,
      // unless the refresh found the attempt held and said so instead.
      if (!waiting)
        status.textContent =
          error?.message ?? error?.error ?? "The request was refused.";
    }
  }
  /// Says why a request was not sent.
  act.say = (text) => {
    status.textContent = text;
  };

  /// Draws the current view. A failed request is said and asked again
  /// shortly, so a panel never stays empty or waiting for good.
  async function refresh() {
    // Only the latest request is drawn: an older answer, a stale busy one
    // included, never replaces a newer view.
    const asked = ++asks;
    try {
      const view = await api("/api/defense");
      if (asked === asks) render(view);
    } catch (error) {
      if (destroyed || asked !== asks) return;
      fail(error);
      // A refusal (a closed defense, no attempt after a restart) stays as it
      // is: the panel stays as drawn. Only a request that failed is retried.
      if (error?.code) return;
      if (!shown)
        statusOnly("The panel could not load; it tries again shortly.");
      later();
    }
  }

  return {
    refresh,
    setLocked(value) {
      locked = value;
      syncControls();
    },
    destroy() {
      destroyed = true;
      clearTimeout(poll);
      root.replaceChildren();
      root.hidden = true;
    },
  };
}
