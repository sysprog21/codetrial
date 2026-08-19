// Minimal syntax highlighter for the languages the editor offers.
//
// The Next.js app this replaced used Monaco, which is megabytes and expects a
// bundler. The editor here is a plain textarea, so all that is needed is
// escaped HTML with spans behind it; the palette follows VS Code Dark+ so the
// result matches the captured visual goldens.
//
// Deliberately not a parser: it tokenizes comments, strings, numbers, and
// identifiers in that order and leaves everything else as plain text. That is
// enough to read code and cannot mangle it, because the textarea remains the
// source of truth.

import { escapeHtml } from "./lib.js";

const JS_KEYWORDS = [
  "async", "await", "break", "case", "catch", "class", "const", "continue", "debugger",
  "default", "delete", "do", "else", "export", "extends", "finally", "for", "function",
  "if", "import", "in", "instanceof", "let", "new", "of", "return", "static", "super",
  "switch", "this", "throw", "try", "typeof", "var", "void", "while", "yield",
];

const C_KEYWORDS = [
  "auto", "break", "case", "char", "const", "continue", "default", "do", "double",
  "else", "enum", "extern", "float", "for", "goto", "if", "inline", "int", "long",
  "register", "restrict", "return", "short", "signed", "sizeof", "static", "struct",
  "switch", "typedef", "union", "unsigned", "void", "volatile", "while",
];

const CPP_KEYWORDS = [
  ...C_KEYWORDS,
  "alignas", "alignof", "bool", "catch", "class", "constexpr", "decltype", "delete",
  "explicit", "false", "friend", "mutable", "namespace", "new", "noexcept", "nullptr",
  "operator", "private", "protected", "public", "template", "this", "throw", "true",
  "try", "typename", "using", "virtual",
];

const JAVA_KEYWORDS = [
  "abstract", "assert", "boolean", "break", "byte", "case", "catch", "char", "class",
  "const", "continue", "default", "do", "double", "else", "enum", "extends", "final",
  "finally", "float", "for", "if", "implements", "import", "instanceof", "int",
  "interface", "long", "native", "new", "package", "private", "protected", "public",
  "return", "short", "static", "strictfp", "super", "switch", "synchronized", "this",
  "throw", "throws", "transient", "try", "void", "volatile", "while",
];

const PY_KEYWORDS = [
  "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del",
  "elif", "else", "except", "finally", "for", "from", "global", "if", "import", "in",
  "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while",
  "with", "yield",
];

const JS_LITERALS = ["true", "false", "null", "undefined", "NaN", "Infinity"];
const C_LITERALS = ["NULL"];
const CPP_LITERALS = ["true", "false", "nullptr", "NULL"];
const JAVA_LITERALS = ["true", "false", "null"];
const PY_LITERALS = ["True", "False", "None"];
const C_LIKE_STRING = String.raw`\`(?:\\.|[^\`\\])*\`|"(?:\\.|[^"\\\n])*"|'(?:\\.|[^'\\\n])*'`;
const C_LIKE_COMMENT = String.raw`//[^\n]*|/\*[\s\S]*?\*/`;

const LANGUAGES = {
  python: {
    keywords: PY_KEYWORDS,
    literals: PY_LITERALS,
    // Triple-quoted first: a bare quote rule would end the string early.
    string: String.raw`"""[\s\S]*?"""|'''[\s\S]*?'''|"(?:\\.|[^"\\\n])*"|'(?:\\.|[^'\\\n])*'`,
    comment: String.raw`#[^\n]*`,
  },
  javascript: {
    keywords: JS_KEYWORDS,
    literals: JS_LITERALS,
    string: C_LIKE_STRING,
    comment: C_LIKE_COMMENT,
  },
  c: {
    keywords: C_KEYWORDS,
    literals: C_LITERALS,
    string: C_LIKE_STRING,
    comment: C_LIKE_COMMENT,
  },
  cpp: {
    keywords: CPP_KEYWORDS,
    literals: CPP_LITERALS,
    string: C_LIKE_STRING,
    comment: C_LIKE_COMMENT,
  },
  java: {
    keywords: JAVA_KEYWORDS,
    literals: JAVA_LITERALS,
    string: C_LIKE_STRING,
    comment: C_LIKE_COMMENT,
  },
};

const NUMBER = String.raw`\b\d(?:[\w.]*\w)?`;
const IDENTIFIER = String.raw`[A-Za-z_$][\w$]*`;

const patterns = new Map();

function pattern(spec) {
  if (!patterns.has(spec)) {
    patterns.set(
      spec,
      new RegExp(
        `(?<comment>${spec.comment})|(?<string>${spec.string})|(?<number>${NUMBER})|(?<identifier>${IDENTIFIER})`,
        "g",
      ),
    );
  }
  return patterns.get(spec);
}

export function languageSpec(language) {
  return LANGUAGES[language] ?? LANGUAGES.javascript;
}

/// Returns HTML for `code`. Every branch escapes, so candidate code can never
/// introduce markup into the page.
export function highlight(code, language) {
  const spec = languageSpec(language);
  const regex = pattern(spec);
  regex.lastIndex = 0;

  let html = "";
  let last = 0;
  for (const match of code.matchAll(regex)) {
    html += escapeHtml(code.slice(last, match.index));
    const text = escapeHtml(match[0]);
    const { comment, string, number, identifier } = match.groups;
    if (comment !== undefined) {
      html += `<span class="tok-comment">${text}</span>`;
    } else if (string !== undefined) {
      html += `<span class="tok-string">${text}</span>`;
    } else if (number !== undefined) {
      html += `<span class="tok-number">${text}</span>`;
    } else if (spec.keywords.includes(identifier)) {
      html += `<span class="tok-keyword">${text}</span>`;
    } else if (spec.literals.includes(identifier)) {
      html += `<span class="tok-literal">${text}</span>`;
    } else {
      html += text;
    }
    last = match.index + match[0].length;
  }
  html += escapeHtml(code.slice(last));

  // A trailing newline would otherwise collapse, leaving the last line of the
  // overlay one row above the textarea's caret.
  return html.endsWith("\n") ? `${html}\n` : html;
}
