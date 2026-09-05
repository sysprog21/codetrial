// A document stub and a module hook, so `node --test` can drive `web/replay.js`.
//
// `web/replay.js` reads `document.querySelector` at module scope and imports by
// absolute URL, so it cannot be imported outside a browser and every check on it
// was a check on its source text. Reading source is how a dozen ways to put an
// accusation on this page were each closed one at a time while the class stayed
// open: the page renders through five files, and a guard that names files is a
// guard somebody adds another to.
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
// `importWithout` below re-imports the page with one id removed so a test can
// measure it, and "a missing id is caught, except for the ids nothing drives"
// in tests/browser/replay-render.test.js holds the answer.
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
    const wanted = selector.split(",").map((part) => part.trim().replace(/^\[|\]$/g, ""));
    for (const name of wanted) {
      if (name !== "data-moment" && name !== "data-window") {
        throw new Error(`the stub does not implement the selector ${selector}`);
      }
    }
    return [...this.descendants()].filter((node) =>
      wanted.some((name) =>
        name === "data-moment" ? "moment" in node.dataset : "window" in node.dataset,
      ),
    );
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
/// `web/replay.js` against it, and reports whether anything failed.
///
/// A second instance, because ESM caches by URL and the page reads its nodes
/// once at module scope. The query string is what makes the copy distinct;
/// `dom-hooks.js` carries it through.
export async function importWithout(markup, id, drive) {
  // Put back afterwards. This installs a second document for the copy it
  // imports, and a reader handed out by the first `installDocument` reads
  // `globalThis.document`, so leaving the replacement in place left every later
  // test in the file reading a page nobody rendered into.
  const previous = globalThis.document;
  installDocument(markup.replaceAll(` id="${id}"`, ""));
  // The list fetch never answers, so `loadList` waits forever instead of taking
  // a branch. What that measures is the import and the render, which are the two
  // things a test drives; the alternative was letting the refusal branch run and
  // reading an unhandled rejection out of it, which is a different thing to
  // catch and one the test runner also watches for.
  //
  // Undone with the document, and for a worse failure than the document's. A
  // stub that never settles is not a wrong answer a later test reads, it is a
  // test that never finishes, and the run reports a timeout somewhere else.
  const restoreFetch = failFetchWith(() => new Promise(() => {}));
  try {
    drive(await import(`/replay.js?without=${encodeURIComponent(id)}`));
  } catch (error) {
    return { caught: true, error };
  } finally {
    globalThis.document = previous;
    restoreFetch();
  }
  return { caught: false };
}

export function installDocument(markup) {
  // Only the ids the page actually declares, because that is what a browser
  // does. Where the page then fails, and which ids this reaches at all, is in
  // the header: a stub that mints an element for every selector renders happily
  // against a page that is broken in a browser, and this narrows that without
  // closing it.
  const present = new Set([...markup.matchAll(/id="([^"]+)"/g)].map((match) => match[1]));
  const nodes = new Map();
  globalThis.document = {
    querySelector(selector) {
      if (!present.has(selector.replace(/^#/, ""))) return null;
      if (!nodes.has(selector)) nodes.set(selector, new Element("div"));
      return nodes.get(selector);
    },
    createElement: (tag) => new Element(tag),
  };
  // The page calls `loadList` at import. Refused rather than answered, so a test
  // drives the render with the events it chose rather than with a list.
  failFetchWith(async () => ({ ok: false, status: 500, json: async () => ({}) }));
  return { node: (id) => document.querySelector(`#${id}`) };
}
