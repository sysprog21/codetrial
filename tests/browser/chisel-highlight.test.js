import test from "node:test";
import assert from "node:assert/strict";
import { launchChromium, startStaticServer } from "./source.js";

// Chisel colored from the vendored tree-sitter-scala grammar, drawn as text.

test("Chisel lines are colored by construct and stay text", async (t) => {
  const browser = await launchChromium();
  if (!browser) return t.skip("playwright chromium unavailable");
  const { server, base } = await startStaticServer();
  try {
    const page = await browser.newPage();
    await page.goto(`${base}/task.html`, { waitUntil: "commit" });
    await page.setContent('<pre id="code"></pre>');
    const kinds = await page.evaluate(async () => {
      const { chiselLines, loadChisel } = await import("/chisel-highlight.js");
      const lines = [
        "val halted = RegInit(false.B) // kept",
        'when (io.x === 3.U) { r := Wire(UInt(32.W)) } // <b>"no"</b>',
      ];
      const plain = chiselLines(lines).flat();
      const before = plain.every((node) => node.nodeType === Node.TEXT_NODE);
      if (!(await loadChisel())) return null;
      const pre = document.querySelector("#code");
      for (const nodes of chiselLines(lines)) pre.append(...nodes, "\n");
      return {
        before,
        text: pre.textContent,
        bold: pre.querySelectorAll("b").length,
        spans: [...pre.querySelectorAll("span")].map(
          (span) => `${span.className}:${span.textContent}`,
        ),
      };
    });
    assert.ok(kinds, "the grammar loads");
    assert.ok(kinds.before, "plain text until the grammar loads");
    for (const expected of [
      "tok-chisel:RegInit",
      "tok-literal:false",
      "tok-number:B",
      "tok-keyword:val",
      "tok-hardware-op:===",
      "tok-hardware-op::=",
      "tok-chisel:Wire",
      "tok-chisel-type:UInt",
      "tok-number:32",
      "tok-comment:// kept",
    ])
      assert.ok(
        kinds.spans.includes(expected),
        `${expected} in ${kinds.spans.join(" ")}`,
      );
    assert.equal(kinds.bold, 0, "code is text, never markup");
    assert.equal(
      kinds.text,
      'val halted = RegInit(false.B) // kept\nwhen (io.x === 3.U) { r := Wire(UInt(32.W)) } // <b>"no"</b>\n',
    );
  } finally {
    await browser.close();
    await new Promise((resolve) => server.close(resolve));
  }
});
