const CONTROL_PARENS = new Set(["if", "while", "for", "with", "switch", "catch"]);
const EXPRESSION_KEYWORDS = new Set([
  "return", "throw", "case", "delete", "void", "typeof", "new", "yield", "await",
  "in", "of", "instanceof", "else", "do",
]);
const BLOCK_KEYWORDS = new Set(["else", "do", "try", "finally"]);
const CPP_RAW_START = /^(?:u8|u|U|L)?R"([^\s()\\]{0,16})\(/;
// Unicode names have different rules for the first and remaining characters.
const IDENTIFIER_START = /[$_\p{ID_Start}]/u;
const IDENTIFIER_PART = /[$_\u200c\u200d\p{ID_Continue}]/u;
const TEMPLATE = "`";

const C_LIKE = {
  lineComment: "//", quotes: "\"'", lineContinuation: true,
  scanners: [scanLineComment, scanBlockComment, scanQuotedString, scanPlainCode],
};
// Scanner order matters: more specific lexical constructs must be checked
// before the generic code scanner.
const LANGUAGES = {
  python: {
    lineComment: "#", quotes: "\"'", lineContinuation: true,
    scanners: [scanLineComment, scanPythonTripleString, scanQuotedString, scanPlainCode],
  },
  javascript: {
    ...C_LIKE,
    scanners: [
      scanInterpolationClose, scanLineComment, scanBlockComment, scanTemplateStart,
      scanQuotedString, scanJavaScriptRegexp, scanJavaScriptCode,
    ],
  },
  c: C_LIKE,
  cpp: {
    ...C_LIKE,
    scanners: [scanLineComment, scanBlockComment, scanCppRawString, scanCppNumber,
      scanQuotedString, scanPlainCode],
  },
  java: {
    ...C_LIKE, lineContinuation: false,
    scanners: [scanLineComment, scanBlockComment, scanJavaTextBlock, scanQuotedString,
      scanPlainCode],
  },
};

function newlineWidth(code, at) {
  if (code[at] === "\r") return code[at + 1] === "\n" ? 2 : 1;
  return code[at] === "\n" || code[at] === "\u2028" || code[at] === "\u2029" ? 1 : 0;
}

// Append a token range, merging it with the previous token when both ranges
// are adjacent and have the same kind.
function emit(tokens, start, end, kind) {
  if (start === end) return;
  const previous = tokens.at(-1);
  if (previous?.kind === kind && previous.end === start) previous.end = end;
  else tokens.push({ start, end, kind });
}

function scanLineComment(scan) {
  const { code, index, rules } = scan;
  if (!code.startsWith(rules.lineComment, index)) return null;
  let end = index + rules.lineComment.length;
  while (end < code.length && !newlineWidth(code, end)) end += 1;
  if (end === code.length) scan.endContext = "lineComment";
  return { end, kind: "comment" };
}

function scanBlockComment(scan) {
  const { code, index } = scan;
  if (!code.startsWith("/*", index)) return null;
  const closing = code.indexOf("*/", index + 2);
  if (closing === -1) scan.endContext = "blockComment";
  return { end: closing === -1 ? code.length : closing + 2, kind: "comment" };
}

function quotedEnd(scan, quote, width, continuation) {
  const { code, index } = scan;
  let at = index + width;
  while (at < code.length) {
    const newline = newlineWidth(code, at);
    if (width === 1 && newline) return at;
    if (code[at] === "\\" && at + 1 < code.length) {
      const escapedNewline = newlineWidth(code, at + 1);
      if (width === 1 && escapedNewline && !continuation) {
        at += 1;
        continue;
      }
      at += escapedNewline || 1;
      at += 1;
    } else if (code.startsWith(quote.repeat(width), at)) {
      return at + width;
    } else {
      at += 1;
    }
  }
  scan.endContext = "string";
  return at;
}

function scanQuotedString(scan) {
  const { code, index, rules } = scan;
  const quote = code[index];
  if (!rules.quotes.includes(quote)) return null;
  const end = quotedEnd(scan, quote, 1, rules.lineContinuation);
  if (scan.js) {
    scan.js.expectRegexp = false;
    scan.js.propertyName = false;
  }
  return { end, kind: "string" };
}

function scanPythonTripleString(scan) {
  const quote = scan.code[scan.index];
  if ((quote !== '"' && quote !== "'") ||
      !scan.code.startsWith(quote.repeat(3), scan.index)) return null;
  return { end: quotedEnd(scan, quote, 3, true), kind: "string" };
}

function scanJavaTextBlock(scan) {
  const { code, index } = scan;
  if (!code.startsWith('"""', index)) return null;
  let at = index + 3;
  while (" \t\f".includes(code[at]) && at < code.length) at += 1;
  if (!newlineWidth(code, at) && at !== code.length) return null;
  return { end: quotedEnd(scan, '"', 3, false), kind: "string" };
}

function scanCppRawString(scan) {
  const { code, index } = scan;
  if (index > 0 && /[\w$]/.test(code[index - 1])) return null;
  if (!/[RuUL]/.test(code[index])) return null;
  const opening = CPP_RAW_START.exec(code.slice(index, index + 24));
  if (!opening) return null;
  const closing = code.indexOf(")" + opening[1] + '"', index + opening[0].length);
  if (closing === -1) scan.endContext = "string";
  return {
    end: closing === -1 ? code.length : closing + opening[1].length + 2,
    kind: "string",
  };
}

function scanCppNumber(scan) {
  const { code, index } = scan;
  if (!/[0-9]/.test(code[index])) return null;
  let at = index + 1;
  while (at < code.length) {
    if (/[\w.]/.test(code[at])) at += 1;
    else if (code[at] === "'" && /[0-9a-fA-F]/.test(code[at + 1] || "") &&
             /[0-9a-fA-F]/.test(code[at - 1])) at += 1;
    else break;
  }
  return { end: at, kind: "code" };
}

function scanTemplateStart(scan) {
  if (scan.code[scan.index] !== TEMPLATE) return null;
  scan.js.frames.push({ kind: "template" });
  return { end: scan.index + 1, kind: "string" };
}

function scanTemplateText(scan) {
  const { code, index } = scan;
  const { frames } = scan.js;
  let at = index;
  while (at < code.length) {
    if (code[at] === "\\" && at + 1 < code.length) {
      at += 2;
    } else if (code[at] === TEMPLATE) {
      at += 1;
      frames.pop();
      scan.js.expectRegexp = false;
      break;
    } else if (code.startsWith("${", at)) {
      emit(scan.tokens, index, at, "string");
      emit(scan.tokens, at, at + 2, "code");
      frames.push({ kind: "interpolation", depth: 0 });
      scan.js.expectRegexp = true;
      return { end: at + 2 };
    } else at += 1;
  }
  if (at === code.length && frames.at(-1)?.kind === "template") scan.endContext = "string";
  return { end: at, kind: "string" };
}

function scanInterpolationClose(scan) {
  const { frames } = scan.js;
  const frame = frames.at(-1);
  if (frame?.kind !== "interpolation" || frame.depth || scan.code[scan.index] !== "}") return null;
  frames.pop();
  return { end: scan.index + 1, kind: "code" };
}

function scanJavaScriptRegexp(scan) {
  const { code, index, js } = scan;
  if (code[index] !== "/" || !js.expectRegexp) return null;
  let at = index + 1;
  let characterClass = false;
  let closed = false;
  while (at < code.length && !newlineWidth(code, at)) {
    const char = code[at];
    if (char === "\\") {
      at += newlineWidth(code, at + 1) ? 1 : Math.min(2, code.length - at);
    } else if (char === "[") {
      characterClass = true;
      at += 1;
    } else if (char === "]" && characterClass) {
      characterClass = false;
      at += 1;
    } else if (char === "/" && !characterClass) {
      at += 1;
      while (at < code.length && /[A-Za-z]/.test(code[at])) at += 1;
      closed = true;
      break;
    } else at += 1;
  }
  js.expectRegexp = false;
  js.propertyName = false;
  if (!closed && at === code.length) scan.endContext = "regexp";
  return { end: at, kind: "regexp" };
}

function codePointAt(code, index) {
  return String.fromCodePoint(code.codePointAt(index));
}

// Infer whether / starts a regexp from preceding code. This is a heuristic;
// function and class declaration bodies are not fully tracked.
function scanJavaScriptCode(scan) {
  const { code, index, js } = scan;
  const char = code[index];
  const point = codePointAt(code, index);
  if (IDENTIFIER_START.test(point)) {
    let at = index + point.length;
    while (at < code.length) {
      const next = codePointAt(code, at);
      if (!IDENTIFIER_PART.test(next)) break;
      at += next.length;
    }
    const word = code.slice(index, at);
    // In obj.return / 2, return is a property name, not a keyword.
    js.controlParen = !js.propertyName && CONTROL_PARENS.has(word);
    js.blockNext = !js.propertyName && BLOCK_KEYWORDS.has(word);
    js.expectRegexp = !js.propertyName && EXPRESSION_KEYWORDS.has(word);
    js.propertyName = false;
    return { end: at, kind: "code" };
  }
  if (/[0-9]/.test(char)) {
    let at = index + 1;
    while (at < code.length && /[\w.]/.test(code[at])) at += 1;
    js.expectRegexp = false;
    js.controlParen = false;
    js.propertyName = false;
    return { end: at, kind: "code" };
  }
  if ((char === "+" || char === "-") && code[index + 1] === char) {
    js.controlParen = false;
    js.propertyName = false;
    return { end: index + 2, kind: "code" };
  }
  if (/\s/.test(char)) return { end: index + 1, kind: "code" };
  const blockNext = js.blockNext;
  js.blockNext = false;
  // if (...) may precede a regexp statement; / after call(...) is division.
  if (char === "(") {
    js.parens.push(js.controlParen);
    js.expectRegexp = true;
  } else if (char === ")") {
    const control = js.parens.pop() === true;
    js.expectRegexp = control;
    js.blockNext = control;
  } else if (char === "{") {
    const kind = blockNext ? "block" : "object";
    js.braces.push(kind);
    if (js.frames.at(-1)?.kind === "interpolation") js.frames.at(-1).depth += 1;
    js.expectRegexp = true;
  } else if (char === "}") {
    if (js.frames.at(-1)?.kind === "interpolation") js.frames.at(-1).depth -= 1;
    js.expectRegexp = js.braces.pop() === "block";
  } else if (char === "]") {
    js.expectRegexp = false;
  } else if (char === ".") {
    js.expectRegexp = false;
    js.propertyName = true;
  } else if (char === "=" && code[index + 1] === ">") {
    js.blockNext = true;
    js.expectRegexp = true;
    js.controlParen = false;
    return { end: index + 2, kind: "code" };
  } else {
    js.expectRegexp = true;
  }
  if (char !== "." && char !== "?") js.propertyName = false;
  js.controlParen = false;
  return { end: index + 1, kind: "code" };
}

function scanPlainCode(scan) {
  return { end: scan.index + 1, kind: "code" };
}

export function tokenize(code, language) {
  if (!Object.hasOwn(LANGUAGES, language))
    throw new RangeError("Unsupported language: " + language);
  const rules = LANGUAGES[language];
  const scan = {
    // index is the next source position; tokens are ranges; endContext records
    // an unfinished comment or literal.
    code, rules, index: 0, tokens: [], endContext: "code",
    // Distinguish division from regexps and template text from interpolation code.
    js: language === "javascript" ? {
      // Frames track templates; interpolation depth counts inner braces.
      frames: [],
      expectRegexp: true,
      // True after if/while/etc.; at (, parens keeps it for the matching ).
      controlParen: false,
      // True after . so obj.return does not treat return as a keyword.
      propertyName: false,
      // True after if (...), else, or =>; { then opens a block, not an object.
      blockNext: false,
      parens: [], braces: [],
    } : null,
  };
  while (scan.index < code.length) {
    const scanners = scan.js?.frames.at(-1)?.kind === "template" ?
      [scanTemplateText] : rules.scanners;
    let result;
    for (const scanner of scanners) {
      // A scanner examines the source at scan.index.
      // It returns null if it does not match, otherwise { end, kind }.
      result = scanner(scan);
      if (result) break;
    }
    if (result.kind) emit(scan.tokens, scan.index, result.end, result.kind);
    scan.index = result.end;
  }
  return { tokens: scan.tokens, endContext: scan.endContext };
}
