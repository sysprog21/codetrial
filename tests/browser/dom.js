// A document stub and a module hook, so `node --test` can drive a page module.
//
// Nothing here is about one page. The hook resolves any absolute `/x.js` the way
// the server serves it, and `installDocument` takes whatever markup it is given,
// so a page is drivable here as soon as somebody hands it its own markup. It was
// written for `web/replay.js`, which is the worked example throughout, and the
// naming was narrowed to match once a second caller arrived.
//
// The problem it solves is general too. A page module that reads
// `document.querySelector` at module scope and imports by absolute URL cannot be
// imported outside a browser, so every check on it is a check on its source
// text. Reading source is how a dozen ways to put an accusation on the replay
// page were each closed one at a time while the class stayed open: that page
// renders through five files, and a guard that names files is a guard somebody
// adds another to.
//
// This is the injected seam the check rules allow, not a browser, and the
// difference matters in one direction: a stub laxer than a browser passes things
// a browser would render. So the properties that carry words model what a
// browser does. `textContent` clears its children when set and concatenates them
// when read; `innerHTML` and its siblings are captured rather than silently
// stored; `dataset` is read back and stringified, because a stylesheet can draw
// it with `content: attr(...)`; `childElementCount` counts elements;
// `querySelectorAll` matches descendants and refuses a selector it does not
// implement; and `querySelector` answers `null` for an id the page does not
// declare, which is what a browser does.
//
// That last one catches less than it looks like, and how much less is asserted
// rather than described. Four attempts to write down which ids fail where
// produced four wrong sentences, because the answer depends on which function
// first dereferences each node and no reading of the source got it right.
// `importWithout` below re-imports a page with one id removed so a test can
// measure it, and "a missing id is caught, except for the ids nothing drives"
// in tests/browser/replay-render.test.js holds the answer for that page.
//
// What this stub is not is a browser, and a page that needs one is out of its
// reach rather than badly served by it: there is no layout, no event loop, and
// no network. `./scripts/browser-check.sh` is where runtime behavior is checked.
//
// The boundary that decides which pages fit is markup structure. Selectors are
// matched flat, against tag, id, class and attribute, in document order, and
// that is enough for every page whose modules read their nodes by name. What it
// does not model is nesting the markup declares: `firstElementChild` answers
// over children a page appended and `null` over children only the HTML has.
//
// `web/interview.js` is the measured case and the reason this is written down
// rather than guessed at. It imports here, which is new and worth having, since
// a module-scope throw in the largest browser module used to be visible only to
// the optional browser check. It cannot be driven past import, because
// `paintEditor` reaches for the `<code>` inside a `<pre>` that only the markup
// declares. Parsing that would mean a tree, and a tree parsed wrong renders a
// page no browser would, which is the one failure this file refuses.
import { register } from "node:module";
import { pathToFileURL } from "node:url";
import { failFetchWith } from "./source.js";

register("./dom-hooks.js", pathToFileURL(`${import.meta.dirname}/`));

/// Everything an element can say to a person that is not its text: the
/// attributes a browser speaks aloud or shows on hover, and the markup
/// properties that put a string on the page without going through `textContent`.
const SPOKEN_ATTRIBUTES = ["title", "ariaLabel", "alt", "placeholder", "value"];
const MARKUP = ["innerHTML", "innerText", "outerHTML"];

class Element {
  #text = "";

  constructor(tag) {
    this.tag = tag;
    this.children = [];
    this.dataset = {};
    // The one default the page reads back before setting it.
    this.hidden = false;
    this.listeners = {};
    /// What was assigned through a markup property. A browser parses it; this
    /// keeps the string, which is all a copy check needs and more than the
    /// silent own-property a plain object would have given it.
    this.markup = [];
    for (const name of MARKUP) {
      Object.defineProperty(this, name, {
        set: (value) => void this.markup.push(String(value)),
        get: () => this.markup.at(-1) ?? "",
      });
    }
    // `toggle` and a reader for it, which is all `web/replay.js` calls and all a
    // test needs to see the result. `add`, `remove` and `contains` on the class
    // list itself went unused; `current` below is what a test asks.
    const classes = new Set();
    this.classList = { toggle: (name, on) => void (on ? classes.add(name) : classes.delete(name)) };
    Object.defineProperty(this, "current", { get: () => classes.has("current") });
  }

  /// A browser's `textContent` is the concatenation of every descendant's text,
  /// and setting it removes the children. A plain property was neither: a page
  /// that set it above a list rendered a sentence a browser would show and this
  /// stub would drop.
  get textContent() {
    return this.children.length ? this.children.map((child) => child.textContent).join("") : this.#text;
  }

  set textContent(value) {
    this.children = [];
    this.#text = String(value);
  }

  /// The first child this page put here, which is structure the stub does have:
  /// `append` and `replaceChildren` built it. Markup nesting is the kind it does
  /// not have, and a page reaching for a child the markup declared gets `null`
  /// rather than an invention.
  get firstElementChild() {
    return this.children.find((child) => child.tag !== "#text") ?? null;
  }

  /// The text this element holds itself, which is what a copy check reads. The
  /// getter above would count a parent's words once per ancestor.
  get ownText() {
    return this.children.length ? "" : this.#text;
  }

  append(...children) {
    for (const child of children) {
      if (typeof child === "string") {
        const text = new Element("#text");
        text.textContent = child;
        this.children.push(text);
      } else {
        this.children.push(child);
      }
    }
  }

  replaceChildren(...children) {
    this.children = [];
    this.#text = "";
    this.append(...children);
  }

  /// The other way a page writes an attribute, and it has to reach the same
  /// reader as the property form: `setAttribute("aria-label", ...)` puts a word
  /// in front of somebody exactly as `ariaLabel = ...` does, and a copy check
  /// that saw one and not the other would be a guard with a hole in it.
  ///
  /// Stored under the property spelling, so `aria-label` and `ariaLabel` are one
  /// value rather than two the reader has to know to add up.
  setAttribute(name, value) {
    this[name.replace(/-([a-z])/g, (_, letter) => letter.toUpperCase())] = String(value);
  }

  getAttribute(name) {
    const property = name.replace(/-([a-z])/g, (_, letter) => letter.toUpperCase());
    return property in this ? String(this[property]) : null;
  }

  addEventListener(name, handler) {
    (this.listeners[name] ||= []).push(handler);
  }

  click() {
    for (const handler of this.listeners.click || []) handler();
  }

  /// Elements only, as a browser counts them: a text node is not a child
  /// element, and `loadList` decides whether to show "No recordings yet" on
  /// this number.
  get childElementCount() {
    return this.children.filter((child) => child.tag !== "#text").length;
  }

  *walk() {
    yield this;
    for (const child of this.children) yield* child.walk();
  }

  /// Descendants only, as a browser matches them: `querySelectorAll` never
  /// returns the element it was called on.
  *descendants() {
    for (const child of this.children) yield* child.walk();
  }

  querySelectorAll(selector) {
    // `[data-x]` only, and the dataset key is derived rather than enumerated.
    // Naming the two attributes this happened to be written for put the pair in
    // two places that had to agree, so the next page marking a node with a third
    // one had to patch the stub to be seen at all. Anything that is not a
    // `data-` attribute asks about structure an element does not model, and is
    // refused loudly rather than answered with an empty list.
    const keys = selector.split(",").map((part) => {
      const name = part.trim().replace(/^\[|\]$/g, "");
      if (!name.startsWith("data-")) {
        throw new Error(`the stub does not implement the selector ${selector}`);
      }
      return name.slice(5).replace(/-([a-z])/g, (_, letter) => letter.toUpperCase());
    });
    return [...this.descendants()].filter((node) => keys.some((key) => key in node.dataset));
  }

  /// Every string this element and its descendants would put in front of a
  /// person: their own text, the attributes a browser speaks, whatever was
  /// assigned through a markup property, and the dataset, which a stylesheet can
  /// draw with `content: attr(data-...)`.
  spoken() {
    return [...this.walk()]
      .flatMap((node) => [
        node.ownText,
        ...SPOKEN_ATTRIBUTES.map((key) => node[key]),
        ...node.markup,
        ...Object.values(node.dataset),
      ])
      // Stringified rather than filtered to strings. A browser stringifies every
      // dataset value and every reflected attribute on its way into the markup,
      // so `dataset.note = ["likely", "assisted"]` draws as text there and was
      // invisible here.
      .filter((value) => value !== undefined && value !== null)
      .map((value) => String(value))
      .filter((value) => value.trim());
  }
}

/// Installs the stub with one id removed from the page, imports a fresh copy of
/// `module` against it, and reports whether anything failed.
///
/// A second instance, because ESM caches by URL and a page reads its nodes once
/// at module scope. The query string is what makes the copy distinct;
/// `dom-hooks.js` carries it through.
export async function importWithout(markup, module, id, drive) {
  // Put back afterwards. This installs a second document for the copy it
  // imports, and a reader handed out by the first `installDocument` reads
  // `globalThis.document`, so leaving the replacement in place left every later
  // test in the file reading a page nobody rendered into.
  const previous = globalThis.document;
  installDocument(markup.replaceAll(` id="${id}"`, ""));
  // A fetch the page starts at import never answers, so it waits forever instead
  // of taking a branch. What that measures is the import and the render, which
  // are the two things a test drives; the alternative was letting the refusal
  // branch run and reading an unhandled rejection out of it, which is a
  // different thing to catch and one the test runner also watches for.
  //
  // Undone with the document, and for a worse failure than the document's. A
  // stub that never settles is not a wrong answer a later test reads, it is a
  // test that never finishes, and the run reports a timeout somewhere else.
  const restoreFetch = failFetchWith(() => new Promise(() => {}));
  try {
    drive(await import(`${module}?without=${encodeURIComponent(id)}`));
  } catch (error) {
    return { caught: true, error };
  } finally {
    globalThis.document = previous;
    restoreFetch();
  }
  return { caught: false };
}

export function installDocument(markup) {
  // The page's own elements, scanned out of the markup flat: tag, id, classes
  // and attributes, in document order. No nesting, no parent, no sibling.
  //
  // That boundary is the point rather than a shortcut. A tree parsed wrong is
  // the one failure mode this file refuses, because it renders a page no
  // browser would; existence and order need no tree, and reading them out of
  // the markup is the same thing the id scan here always did. So a selector
  // that asks about structure is refused loudly below rather than answered
  // with a guess.
  const declared = [...markup.matchAll(/<([a-zA-Z][\w-]*)((?:\s+[\w:-]+(?:="[^"]*")?)*)\s*\/?>/g)].map(
    (match) => {
      const attributes = Object.fromEntries(
        [...match[2].matchAll(/([\w:-]+)(?:="([^"]*)")?/g)].map((it) => [it[1], it[2] ?? ""]),
      );
      return {
        tag: match[1].toLowerCase(),
        id: attributes.id,
        classes: new Set((attributes.class || "").split(/\s+/).filter(Boolean)),
        attributes,
      };
    },
  );
  const nodes = new Map();
  /// One selector against one scanned element. Supported: `#id`, `.class`,
  /// `tag`, `[attr]` and `[attr="value"]`, and a concatenation of those on one
  /// element. Anything with a combinator asks about structure this does not
  /// model, and throwing is what stops a test quietly asserting over nothing.
  const matches = (selector, element) => {
    if (/[\s>+~,]/.test(selector.trim())) {
      throw new Error(`the document stub models no structure, so it cannot match: ${selector}`);
    }
    const parts = selector.trim().match(/^[a-zA-Z][\w-]*|#[\w:-]+|\.[\w-]+|\[[^\]]+\]/g);
    if (!parts || parts.join("") !== selector.trim()) {
      throw new Error(`the document stub does not implement the selector: ${selector}`);
    }
    return parts.every((part) => {
      if (part.startsWith("#")) return element.id === part.slice(1);
      if (part.startsWith(".")) return element.classes.has(part.slice(1));
      if (part.startsWith("[")) {
        const [, name, value] = part.slice(1, -1).match(/^([\w:-]+)(?:="?([^"]*)"?)?$/) || [];
        if (!name) throw new Error(`the document stub cannot read the attribute selector: ${part}`);
        if (!(name in element.attributes)) return false;
        return value === undefined || element.attributes[name] === value;
      }
      return element.tag === part.toLowerCase();
    });
  };
  // Keyed on the element rather than on the selector, so two selectors that
  // name the same element hand back the same node. Keying on the selector let
  // `#start` and `button#start` render into two different places, which is a
  // page a browser cannot produce.
  const node = (element) => {
    if (!nodes.has(element)) nodes.set(element, new Element(element.tag));
    return nodes.get(element);
  };
  globalThis.document = {
    querySelector: (selector) => {
      const found = declared.find((element) => matches(selector, element));
      return found ? node(found) : null;
    },
    querySelectorAll: (selector) =>
      declared.filter((element) => matches(selector, element)).map(node),
    createElement: (tag) => new Element(tag),
    // A page that registers a listener while it loads gets to. Nothing here
    // dispatches, so what this buys is the import rather than the behaviour,
    // and a test that wants the behaviour drives the handler it was given.
    addEventListener() {},
  };

  // Enough `window` for a module that registers a listener or reads the origin
  // while it loads, and no more. Navigation is recorded rather than performed,
  // because there is nowhere to go: a test that wants to know where a page sent
  // somebody reads `window.location.href` back, and one that does not is not
  // made to care.
  globalThis.window = {
    location: { origin: "https://codetrial.test", href: "", search: "", reload() {} },
    addEventListener() {},
  };
  // Whatever the page fetches at import is refused rather than answered, so a
  // test drives the render with the state it chose rather than with a response.
  failFetchWith(async () => ({ ok: false, status: 500, json: async () => ({}) }));
  return { node: (id) => document.querySelector(`#${id}`) };
}
