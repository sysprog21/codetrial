import { test } from "node:test";
import assert from "node:assert/strict";

import { tokenize } from "../../web/tokenizer.js";

const parts = (code, language) => {
  const { tokens, endContext } = tokenize(code, language);
  assert.equal(tokens[0]?.start ?? 0, 0);
  assert.equal(tokens.at(-1)?.end ?? 0, code.length);
  for (let index = 1; index < tokens.length; index += 1) {
    assert.equal(tokens[index].start, tokens[index - 1].end, "token ranges must meet");
  }
  assert.equal(tokens.map(({ start, end }) => code.slice(start, end)).join(""), code);
  return { parts: tokens.map(({ start, end, kind }) => [kind, code.slice(start, end)]), endContext };
};

test("block comments carry across lines and return to code after the closer", () => {
  for (const language of ["javascript", "c", "cpp", "java"]) {
    assert.deepEqual(parts("/*\n * setup {\n*/ if (ready) {", language), {
      parts: [["comment", "/*\n * setup {\n*/"], ["code", " if (ready) {"]],
      endContext: "code",
    });
    assert.equal(parts("/*\n * setup {", language).endContext, "blockComment");
  }
  assert.deepEqual(parts("/*\nif ready:", "python"), {
    parts: [["code", "/*\nif ready:"]], endContext: "code",
  });
});

test("comment openers inside multiline strings do not start comments", () => {
  const cases = [
    ["javascript", "const text = `\n/*\n`;\nif (ready) {", "`\n/*\n`"],
    ["python", 'text = """\n/*\n"""\nif ready:', '"""\n/*\n"""'],
    ["cpp", 'auto text = R"tag(\n/*\n)tag";\nif (ready) {', 'R"tag(\n/*\n)tag"'],
    ["java", 'String text = """\n/*\n""";\nif (ready) {', '"""\n/*\n"""'],
  ];
  for (const [language, code, literal] of cases) {
    const result = parts(code, language);
    assert.ok(result.parts.some(([kind, text]) => kind === "string" && text === literal), language);
    assert.ok(result.parts.some(([kind, text]) => kind === "code" && text.includes("if ")), language);
    assert.equal(result.parts.some(([kind]) => kind === "comment"), false, language);
    assert.equal(result.endContext, "code", language);
  }
});

test("unterminated multiline strings remain strings through the buffer", () => {
  for (const [language, code] of [
    ["javascript", "const text = `\n/*\nif (ready) {"],
    ["python", 'text = """\n/*\nif ready:'],
    ["cpp", 'auto text = R"tag(\n/*\nif (ready) {'],
    ["java", 'String text = """\n/*\nif (ready) {'],
  ]) {
    const result = parts(code, language);
    assert.equal(result.endContext, "string", language);
    assert.equal(result.parts.some(([kind]) => kind === "comment"), false, language);
  }
  assert.deepEqual(parts('text = "oops\nif ready:', "python"), {
    parts: [["code", "text = "], ["string", '"oops'], ["code", "\nif ready:"]],
    endContext: "code",
  });
});

test("JavaScript regex literals do not turn their contents into comments", () => {
  const code = String.raw`const re = /\/*/;` + "\nif (ready) {";
  const result = parts(code, "javascript");
  assert.ok(result.parts.some(([kind, text]) => kind === "regexp" && text === String.raw`/\/*/`));
  assert.equal(result.parts.some(([kind]) => kind === "comment"), false);
  assert.equal(result.endContext, "code");
  assert.ok(parts("const re = /[/*]/;", "javascript").parts.some(([kind, text]) =>
    kind === "regexp" && text === "/[/*]/"));
  assert.ok(parts("const n = total / count;", "javascript").parts.every(([kind]) => kind === "code"));
  assert.ok(parts("const n = total++ / count;", "javascript").parts.every(([kind]) => kind === "code"));
  assert.ok(parts("return /[/*]/.test(input);", "javascript").parts.some(([kind]) => kind === "regexp"));
  assert.ok(parts("if (ready) /[/*]/.test(input);", "javascript").parts.some(([kind]) => kind === "regexp"));
  const afterBlock = parts(String.raw`if (ready) {} /\/*/.test(input);`, "javascript");
  assert.ok(afterBlock.parts.some(([kind]) => kind === "regexp"));
  assert.equal(afterBlock.parts.some(([kind]) => kind === "comment"), false);
  assert.ok(parts("const object = {} / 2;", "javascript").parts.every(([kind]) => kind === "code"));
});

test("property names and Unicode identifiers do not turn division into a regexp", () => {
  for (const code of [
    "const n = object.return / 2;",
    "const n = object.if(ready) / 2;",
    "const n = object?.return / 2;",
    "const n = π / 2;",
    "const n = 𝛑 / 2;",
  ]) {
    assert.deepEqual(parts(code, "javascript"), {
      parts: [["code", code]], endContext: "code",
    }, code);
  }
});

test("escaped CRLF stays inside a string and Java quotes stop at a newline", () => {
  const continued = 'const s = "a\\\r\n/*\\\r\n";\r\nif (ready) {';
  assert.equal(parts(continued, "javascript").parts.some(([kind]) => kind === "comment"), false);
  assert.equal(parts(continued, "javascript").endContext, "code");
  const java = 'String s = "a\\\n/* comment */\nif (ready) {';
  assert.ok(parts(java, "java").parts.some(([kind]) => kind === "comment"));
  assert.equal(parts(java, "java").endContext, "code");
});

test("Java text blocks require a line break after the opening quotes", () => {
  const invalid = parts('String s = """abc\nif (ready) {', "java");
  assert.equal(invalid.endContext, "code");
  assert.ok(invalid.parts.some(([kind, text]) => kind === "code" && text.includes("if (ready) {")));
  assert.equal(parts('String s = """ \nbody\n""";', "java").endContext, "code");
});

test("C++ digit separators do not start a quoted string", () => {
  const code = "auto n = 1'000; if (ready) {";
  assert.deepEqual(parts(code, "cpp"), { parts: [["code", code]], endContext: "code" });
});

test("template interpolation returns to code and handles nested templates", () => {
  const code = 'const text = `head ${ready ? `nested ${value}` : "/*"} tail`;';
  const result = parts(code, "javascript");
  assert.ok(result.parts.some(([kind, text]) => kind === "code" && text.includes("ready ? ")));
  assert.ok(result.parts.some(([kind, text]) => kind === "code" && text.includes("value")));
  assert.ok(result.parts.some(([kind, text]) => kind === "string" && text === '"/*"'));
  assert.equal(result.parts.some(([kind]) => kind === "comment"), false);
  assert.equal(result.endContext, "code");
  assert.equal(parts("const text = `head ${ready", "javascript").endContext, "code");
  assert.equal(parts("const text = `head ${ready} /* tail`;", "javascript").parts
    .some(([kind]) => kind === "comment"), false);
});

test("line comments end at a newline and escaped quotes hide comment markers", () => {
  assert.deepEqual(parts("# /*\nif ready:", "python"), {
    parts: [["comment", "# /*"], ["code", "\nif ready:"]], endContext: "code",
  });
  assert.equal(parts("// note", "cpp").endContext, "lineComment");
  const code = String.raw`const text = "\"/*"; // note`;
  assert.deepEqual(parts(code, "javascript").parts.map(([kind]) => kind),
    ["code", "string", "code", "comment"]);
});

test("existing highlighted quotes and comments are classified in every language", () => {
  for (const [language, code] of [
    ["python", 'value = "return" # note'],
    ["javascript", 'const value = "return"; // note'],
    ["c", 'char *value = "return"; // note'],
    ["cpp", 'auto value = "return"; // note'],
    ["java", 'String value = "return"; // note'],
  ]) {
    const result = parts(code, language);
    assert.ok(result.parts.some(([kind, text]) => kind === "string" && text === '"return"'), language);
    assert.ok(result.parts.some(([kind, text]) => kind === "comment" && text.endsWith("note")), language);
  }
});

test("constructs from another language stay ordinary code", () => {
  for (const [language, code] of [
    ["python", "/* marker */"],
    ["python", "// marker"],
    ["c", "# marker"],
    ["c", "value = `template`"],
    ["java", "value = `template`"],
  ]) {
    assert.deepEqual(parts(code, language), {
      parts: [["code", code]], endContext: "code",
    }, language);
  }
});

test("an unsupported language throws before scanning", () => {
  for (const language of ["unknown", "constructor", "toString", ""]) {
    assert.throws(() => tokenize("/*", language), RangeError);
  }
});
